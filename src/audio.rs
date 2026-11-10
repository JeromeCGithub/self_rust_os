//! Native exclusive PCM service. Lock order: scheduler metadata -> audio; audio
//! never calls the scheduler while locked. No IRQ acquires this lock. Entry gate
//! stabilizes user mappings during owned copies; device storage stays kernel-only.
use crate::{drivers::audio::ac97::Device, userspace::usercopy};
use rustos_audio_abi::{self as abi, Error};
use spin::Mutex;
use x86_64::VirtAddr;
struct Service {
    device: Option<Device>,
    owner: Option<(u32, u64)>,
    next: u64,
}
static AUDIO: Mutex<Service> = Mutex::new(Service {
    device: None,
    owner: None,
    next: 1,
});
/// Bring up optional audio once, before scheduler ownership of the frame allocator.
pub fn init(frames: &mut crate::memory::RegionFrameAllocator, offset: VirtAddr) {
    match Device::init(frames, offset) {
        Ok(device) => {
            AUDIO.lock().device = Some(device);
            crate::serial_println!("[audio] AC97 stereo S16LE 48000Hz; 32 bounded DMA slots");
        }
        Err(error) => {
            crate::serial_println!("[audio] PCM unavailable: {:?}", error);
        }
    }
}
/// Stop a dead owner's DMA before dropping its stream lease. Boot-reserved DMA
/// frames are device-lifetime resources, never returned while hardware may own them.
pub fn cleanup(pid: u32) {
    let mut audio = AUDIO.lock();
    if audio.owner.is_some_and(|(owner, _)| owner == pid) {
        if let Some(device) = audio.device.as_mut() {
            let _ = device.reset();
        }
        audio.owner = None;
    }
}
/// Terminal halt only, under kernel entry after parking remote CPUs. A failure
/// is diagnostic; boot-owned device storage remains retained forever regardless.
pub(crate) fn disable_dma_terminal() -> bool {
    let Some(mut service) = AUDIO.try_lock() else {
        return false;
    };
    service
        .device
        .as_mut()
        .map_or(true, Device::disable_dma_terminal)
}
/// Validate ABI/user buffers and dispatch without retaining any user reference.
pub fn syscall(number: u64, first: u64, second: u64, third: u64) -> u64 {
    dispatch(number, first, second, third).unwrap_or_else(Error::encode)
}
fn dispatch(number: u64, first: u64, second: u64, third: u64) -> Result<u64, Error> {
    let pid = crate::scheduler::current_pid().ok_or(Error::Invalid)?;
    // Validate/copy complete buffers before locking any device. A valid handle
    // never authorizes access through an unmapped or supervisor-only pointer.
    let mut pcm = [0u8; abi::MAX_WRITE];
    if number == abi::SYS_WRITE {
        if third == 0
            || third > abi::MAX_WRITE as u64
            || third % 4 != 0
            || !usercopy::from_user(second, &mut pcm[..third as usize])
        {
            return Err(Error::Invalid);
        }
    } else if number == abi::SYS_STATUS {
        if third != core::mem::size_of::<abi::Status>() as u64
            || !usercopy::validate(second, third, true)
        {
            return Err(Error::Invalid);
        }
    } else if third != 0
        || (number == abi::SYS_OPEN && (first != 0 || second != 0))
        || (number == abi::SYS_CONTROL && second > 2)
    {
        return Err(Error::Invalid);
    }
    let mut audio = AUDIO.lock();
    if audio.device.is_none() {
        return Err(Error::NoDevice);
    }
    if number == abi::SYS_OPEN {
        if audio.owner.is_some() {
            return Err(Error::Busy);
        }
        let handle = audio.next;
        audio.next = handle
            .checked_add(1)
            .filter(|n| *n < u64::MAX - 1023)
            .ok_or(Error::Failed)?;
        audio.device.as_mut().ok_or(Error::NoDevice)?.reset()?;
        audio.owner = Some((pid, handle));
        return Ok(handle);
    }
    if audio.owner != Some((pid, first)) {
        return Err(Error::NotOwner);
    }
    let device = audio.device.as_mut().ok_or(Error::NoDevice)?;
    match number {
        abi::SYS_WRITE => device.write(&pcm[..third as usize]).map(|n| n as u64),
        abi::SYS_CONTROL => {
            match second {
                abi::CLOSE => {
                    device.reset()?;
                    audio.owner = None;
                }
                abi::PAUSE => device.pause(true)?,
                abi::RESUME => device.pause(false)?,
                _ => return Err(Error::Invalid),
            }
            Ok(0)
        }
        abi::SYS_STATUS => {
            let status = device.status()?;
            // SAFETY: Status is padding-free initialized integers. The entire
            // destination was checked writable/user above; entry gate prevents
            // unmapping through this copy. Assembly creates no shared user borrow.
            unsafe {
                usercopy::copy_raw(
                    second as *mut u8,
                    core::ptr::addr_of!(status).cast(),
                    core::mem::size_of::<abi::Status>(),
                );
            }
            Ok(0)
        }
        _ => Err(Error::Invalid),
    }
}
