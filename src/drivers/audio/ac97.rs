//! Intel82801AA PCM output. No audio IRQ handler or userspace DMA mapping.
use crate::{drivers::pci, memory::RegionFrameAllocator};
use core::sync::atomic::{fence, Ordering};
use rustos_audio_abi::{Error, Status};
use rustos_audio_queue::{Ring, SLOTS};
use x86_64::VirtAddr;
#[derive(Clone, Copy)]
#[repr(C)]
struct Descriptor {
    address: u32,
    samples: u16,
    flags: u16,
}
const _: () = assert!(core::mem::size_of::<Descriptor>() == 8);
/// One controller with boot-owned33 DMA frames, reused across exclusive leases.
/// Integer addresses avoid Rust aliases to device-visible memory. Service lock
/// serializes every operation; no IRQ touches registers or these buffers.
pub struct Device {
    function: pci::Function,
    nam: u16,
    bm: u16,
    descriptors: usize,
    descriptor_phys: u32,
    buffers: [usize; SLOTS],
    physical: [u32; SLOTS],
    offsets: [u32; SLOTS],
    ring: Ring,
    paused: bool,
    started: bool,
    failed: bool,
    samples_left: u16,
    underruns: u64,
}
fn io_bar(raw: u32, bytes: u16) -> Result<u16, Error> {
    super::caps::io_bar(raw, bytes).ok_or(Error::NoDevice)
}
impl Device {
    /// Initialize once during single-CPU boot, before scheduler frame ownership.
    pub fn init(frames: &mut RegionFrameAllocator, offset: VirtAddr) -> Result<Self, Error> {
        let function = pci::find_device(0x8086, 0x2415, pci::read).ok_or(Error::NoDevice)?;
        let nam = io_bar(pci::read(function, 0x10), 0x100)?;
        let bm = io_bar(pci::read(function, 0x14), 0x40)?;
        set_command(function, false);
        let mut device = Self {
            function,
            nam,
            bm,
            descriptors: 0,
            descriptor_phys: 0,
            buffers: [0; SLOTS],
            physical: [0; SLOTS],
            offsets: [0; SLOTS],
            ring: Ring::default(),
            paused: false,
            started: false,
            failed: false,
            samples_left: 0,
            underruns: 0,
        };
        device.w32(0x2c, 2); // Cold-reset deasserted; stereo/16-bit profile.
        let mut ready = false;
        for _ in 0..10000 {
            if device.r32(0x30) & (1 << 8) != 0 {
                ready = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !ready {
            return Err(Error::NoDevice);
        }
        device.codec(0, 0);
        device.codec(0x02, 0);
        device.codec(0x18, 0);
        device.codec(0x26, 0);
        device.codec(0x2a, 0);
        device.reset()?;
        let dma = frames.allocate_batch::<33>().ok_or(Error::Failed)?;
        if dma
            .iter()
            .any(|frame| frame.start_address().as_u64() > u64::from(u32::MAX) - 4095)
        {
            for frame in dma {
                // SAFETY: The unique batch has never been published to DMA or
                // mappings. Bus mastering is disabled; returning frames cannot
                // race a device/CPU owner. The allocator owns the return storage.
                unsafe {
                    frames.recycle(frame);
                }
            }
            return Err(Error::Failed);
        }
        for frame in &dma {
            // SAFETY: Each unique reserved4KiB RAM frame is directly mapped by
            // boot's physical offset, exclusively owned and not yet DMA-visible.
            unsafe {
                core::ptr::write_bytes(
                    (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
                    0,
                    4096,
                );
            }
        }
        device.descriptors = (offset + dma[0].start_address().as_u64()).as_u64() as usize;
        device.descriptor_phys = dma[0].start_address().as_u64() as u32;
        for n in 0..SLOTS {
            device.buffers[n] = (offset + dma[n + 1].start_address().as_u64()).as_u64() as usize;
            device.physical[n] = dma[n + 1].start_address().as_u64() as u32;
        }
        device.w32(0x10, device.descriptor_phys);
        fence(Ordering::SeqCst);
        set_command(function, true);
        Ok(device)
    }
    /// Stop/reset DMA with bounded polling before changing lease ownership.
    pub fn reset(&mut self) -> Result<(), Error> {
        self.reset_hardware()?;
        self.ring = Ring::default();
        self.paused = false;
        self.samples_left = 0;
        self.underruns = 0;
        self.offsets = [0; SLOTS];
        Ok(())
    }
    fn reset_hardware(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        self.w8(0x1b, 0);
        let mut stopped = false;
        for _ in 0..10000 {
            if self.r16(0x16) & 1 != 0 {
                stopped = true;
                break;
            }
            core::hint::spin_loop();
        }
        if stopped {
            self.w8(0x1b, 2);
            for _ in 0..10000 {
                if self.r8(0x1b) & 2 == 0 {
                    self.w32(0x10, self.descriptor_phys);
                    self.started = false;
                    return Ok(());
                }
                core::hint::spin_loop();
            }
        }
        // Quarantine: prevent any future buffer reuse after an uncertain stop.
        // Boot-owned frames remain reserved even if bus mastering cannot drain.
        set_command(self.function, false);
        self.failed = true;
        Err(Error::Failed)
    }
    fn poll(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        if !self.started || self.ring.queued() == 0 {
            return Ok(());
        }
        for _ in 0..4 {
            let current = self.r8(0x14);
            let status = self.r16(0x16);
            let remaining = self.r16(0x18);
            if self.r8(0x14) != current {
                continue;
            }
            self.ring.observe(current, status & 1 != 0, remaining)?;
            self.samples_left = remaining;
            if status & 16 != 0 {
                self.underruns = self.underruns.saturating_add(1);
            }
            self.w16(0x16, status & 0x1c);
            return Ok(());
        }
        Err(Error::WouldBlock)
    }
    /// Copy into one free controller-owned slot before publishing its descriptor.
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        if bytes.is_empty() || bytes.len() > 4096 || bytes.len() % 4 != 0 {
            return Err(Error::Invalid);
        }
        self.poll()?;
        let slot = self.ring.push((bytes.len() / 4) as u32)?;
        self.offsets[slot] = 0;
        // SAFETY: Ring reserves only a free slot, never the DMA's live/prefetched
        // range through LVI. The complete disjoint source is kernel-owned; this
        // slot is one reserved aligned4KiB RAM frame and len<=4096. The aligned
        // descriptor slot fits in its separate owned frame. No Rust DMA reference
        // is retained. Service mutex excludes CPU writers and cleanup.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.buffers[slot] as *mut u8,
                bytes.len(),
            );
            (self.descriptors as *mut Descriptor)
                .add(slot)
                .write_volatile(Descriptor {
                    address: self.physical[slot],
                    samples: (bytes.len() / 2) as u16,
                    flags: 0,
                });
        }
        // x86 coherent DMA: order data/descriptor stores before the LVI doorbell.
        // The mutex, not this publication fence, owns multi-field CPU state.
        fence(Ordering::SeqCst);
        self.w8(0x15, slot as u8);
        if !self.paused && !self.started {
            self.w8(0x1b, 1);
            self.started = true;
        }
        Ok(bytes.len())
    }
    /// Pause/resume DMA without discarding accepted samples.
    pub fn pause(&mut self, paused: bool) -> Result<(), Error> {
        if self.paused == paused {
            return Ok(());
        }
        self.poll()?;
        if paused {
            self.w8(0x1b, 0);
            let mut stopped = false;
            for _ in 0..10000 {
                if self.r16(0x16) & 1 != 0 {
                    stopped = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !stopped {
                set_command(self.function, false);
                self.failed = true;
                return Err(Error::Failed);
            }
            self.poll()?;
            if self.started {
                // A fresh descriptor0 start also avoids controllers/emulators
                // resuming at prefetched PIV and discarding the partial current
                // buffer. Rebase only after DMA is quiescent; preserve counters.
                let (head, consumed) = self.ring.rebase(self.samples_left)?;
                self.reset_hardware()?;
                self.buffers.rotate_left(head);
                self.physical.rotate_left(head);
                self.offsets.rotate_left(head);
                if self.ring.queued() > 0 {
                    self.offsets[0] += consumed * 4;
                }
                for slot in 0..self.ring.queued() {
                    // SAFETY: DMA has been stopped/reset. Every descriptor is
                    // aligned/in the owned frame; its checked partial offset
                    // plus remaining length stays within its own4KiB buffer.
                    // No other CPU/IRQ can access these arrays under the mutex.
                    unsafe {
                        (self.descriptors as *mut Descriptor)
                            .add(slot)
                            .write_volatile(Descriptor {
                                address: self.physical[slot] + self.offsets[slot],
                                samples: (self.ring.frames(slot) * 2) as u16,
                                flags: 0,
                            });
                    }
                }
                if self.ring.queued() > 0 {
                    fence(Ordering::SeqCst);
                    self.w8(0x15, (self.ring.queued() - 1) as u8);
                }
            }
        } else if self.ring.queued() > 0 {
            self.w8(0x1b, 1);
            self.started = true;
        }
        self.paused = paused;
        Ok(())
    }
    /// Snapshot consumed frames and backpressure after bounded progress polling.
    pub fn status(&mut self) -> Result<Status, Error> {
        self.poll()?;
        let played = if self.started {
            self.ring.played(self.samples_left)
        } else {
            self.ring.completed()
        };
        Ok(Status {
            version: 1,
            flags: if self.paused {
                2
            } else if self.ring.queued() > 0 {
                1
            } else {
                0
            },
            submitted_frames: self.ring.submitted(),
            played_frames: played,
            queued_frames: self.ring.submitted().saturating_sub(played) as u32,
            free_slots: (SLOTS - self.ring.queued()) as u32,
            underruns: self.underruns,
        })
    }
    fn r8(&self, register: u16) -> u8 {
        // SAFETY: Fixed offsets fit the validated64-byte bus-master I/O BAR;
        // boot or the service lock exclusively owns register access. No IRQ uses it.
        unsafe { x86_64::instructions::port::Port::<u8>::new(self.bm + register).read() }
    }
    fn r16(&self, register: u16) -> u16 {
        // SAFETY: Aligned fixed16-bit register within the validated bus-master BAR.
        unsafe { x86_64::instructions::port::Port::<u16>::new(self.bm + register).read() }
    }
    fn r32(&self, register: u16) -> u32 {
        // SAFETY: Aligned fixed32-bit register within the validated bus-master BAR.
        unsafe { x86_64::instructions::port::Port::<u32>::new(self.bm + register).read() }
    }
    fn w8(&self, register: u16, value: u8) {
        // SAFETY: Fixed offset within the validated bus-master BAR, exclusively
        // accessed at boot/under service mutex, never by an IRQ.
        unsafe {
            x86_64::instructions::port::Port::<u8>::new(self.bm + register).write(value);
        }
    }
    fn w16(&self, register: u16, value: u16) {
        // SAFETY: Aligned fixed16-bit register within validated bus-master BAR.
        unsafe {
            x86_64::instructions::port::Port::<u16>::new(self.bm + register).write(value);
        }
    }
    fn w32(&self, register: u16, value: u32) {
        // SAFETY: Aligned fixed32-bit register within validated bus-master BAR.
        unsafe {
            x86_64::instructions::port::Port::<u32>::new(self.bm + register).write(value);
        }
    }
    fn codec(&self, register: u16, value: u16) {
        // SAFETY: Fixed aligned codec register fits the validated256-byte NAM
        // aperture. Boot owns codec initialization; no IRQ accesses this BAR.
        unsafe {
            x86_64::instructions::port::Port::<u16>::new(self.nam + register).write(value);
        }
    }
}
fn set_command(function: pci::Function, dma: bool) {
    let command = pci::read(function, 4) as u16;
    // SAFETY: Boot is single-CPU; runtime cleanup holds kernel entry and audio
    // mutex. No other runtime caller/IRQ accesses PCI configuration pairs.
    // Halfword command writes preserve the adjacent W1C status register.
    unsafe {
        x86_64::instructions::port::Port::<u32>::new(0xcf8).write(function.address(4));
        x86_64::instructions::port::Port::<u16>::new(0xcfc).write(if dma {
            command | 5
        } else {
            (command | 1) & !4
        });
    }
}
