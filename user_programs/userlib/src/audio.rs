//! Exclusive bounded PCM output. Decode/mix/resample in userspace before writing.
use crate::sys;
use rustos_audio_abi as abi;
pub use rustos_audio_abi::{Error, Status, CHANNELS, FRAME_BYTES, MAX_WRITE, RATE};
/// Owned native stream; dropping it stops DMA and releases device ownership.
pub struct Stream {
    handle: u64,
}
impl Stream {
    /// Open stereo signed16-bit little-endian48kHz output, or return Busy/NoDevice.
    pub fn open() -> Result<Self, Error> {
        // SAFETY: OPEN takes only zero scalar arguments and returns an owned lease.
        let handle = abi::decode(unsafe { sys::syscall(abi::SYS_OPEN, 0, 0, 0) })?;
        Ok(Self { handle })
    }
    /// Write at most4096 bytes without blocking; every write is whole PCM frames.
    /// WouldBlock consumes no data. The caller retains its buffer after return.
    pub fn write(&mut self, pcm: &[u8]) -> Result<usize, Error> {
        if pcm.is_empty() || pcm.len() > MAX_WRITE || pcm.len() % FRAME_BYTES != 0 {
            return Err(Error::Invalid);
        }
        // SAFETY: The entire immutable slice lives through this synchronous
        // syscall; kernel copies it before returning and retains no user pointer.
        abi::decode(unsafe {
            sys::syscall(
                abi::SYS_WRITE,
                self.handle,
                pcm.as_ptr() as u64,
                pcm.len() as u64,
            )
        })
        .map(|n| n as usize)
    }
    /// Read hardware-consumption counters and bounded queue capacity.
    pub fn status(&self) -> Result<Status, Error> {
        let mut status = Status::default();
        // SAFETY: Status is fully initialized, aligned, padding-free writable
        // storage of the exact ABI size; no reference is retained by the kernel.
        abi::decode(unsafe {
            sys::syscall(
                abi::SYS_STATUS,
                self.handle,
                core::ptr::addr_of_mut!(status) as u64,
                core::mem::size_of::<Status>() as u64,
            )
        })?;
        if status.version != abi::VERSION {
            return Err(Error::Failed);
        }
        Ok(status)
    }
    fn control(&mut self, operation: u64) -> Result<(), Error> {
        // SAFETY: Opaque owned handle and a documented scalar operation only.
        abi::decode(unsafe { sys::syscall(abi::SYS_CONTROL, self.handle, operation, 0) })
            .map(|_| ())
    }
    /// Pause without dropping queued audio.
    pub fn pause(&mut self) -> Result<(), Error> {
        self.control(abi::PAUSE)
    }
    /// Resume queued audio.
    pub fn resume(&mut self) -> Result<(), Error> {
        self.control(abi::RESUME)
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.control(abi::CLOSE);
    }
}
