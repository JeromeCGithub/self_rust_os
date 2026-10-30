//! Intel 82540EM Ethernet transport. Single CPU, polled legacy descriptors.
//! MMIO is supervisor-only UC; DMA buffers are permanently device-owned frames.
//! Only bounded service in ordinary syscall context accesses descriptor rings.
use crate::drivers::pci;
use super::e1000_caps::{self, COUNT, FRAME_BYTES};
use crate::memory::RegionFrameAllocator;
use core::sync::atomic::{fence, Ordering};
use rustos_net_abi::Error;
use smoltcp::{
    phy::{self, Device, DeviceCapabilities},
    time::Instant,
};
use x86_64::{
    structures::paging::{
        Mapper, OffsetPageTable, Page, PageTableFlags, PhysFrame, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

const MMIO: u64 = 0xffff_a000_0000_0000;
const MMIO_BYTES: u64 = 0x20000;

#[derive(Clone, Copy)]
#[repr(C)]
struct RxDesc {
    address: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}
#[derive(Clone, Copy)]
#[repr(C)]
struct TxDesc {
    address: u64,
    length: u16,
    cso: u8,
    command: u8,
    status: u8,
    css: u8,
    special: u16,
}
const _: () = assert!(core::mem::size_of::<RxDesc>() == 16 && core::mem::size_of::<TxDesc>() == 16);

/// A single polled NIC. Addresses are integers to avoid references to DMA-owned
/// memory. CPU access is serialized by the outer network lock.
pub struct E1000 {
    mmio: usize,
    rx_ring: usize,
    tx_ring: usize,
    rx_buffers: [usize; COUNT],
    tx_buffers: [usize; COUNT],
    rx_next: usize,
    tx_next: usize,
    mac: [u8; 6],
}

impl E1000 {
    /// Discover/map/reset the supported NIC, then reserve the entire DMA batch.
    /// Called once during boot before allocator ownership passes to the scheduler.
    pub fn init(
        mapper: &mut OffsetPageTable<'_>,
        frames: &mut RegionFrameAllocator,
        offset: VirtAddr,
    ) -> Result<Self, Error> {
        let function = pci::find_device(0x8086, 0x100e, pci::read).ok_or(Error::NoDevice)?;
        let physical = pci::memory_bar(pci::read(function, 0x10)).ok_or(Error::NoDevice)?;
        // The supported BAR has a fixed 128 KiB aperture. Reject firmware layouts
        // with a conflicting direct-map alias, or an unaligned/out-of-range BAR.
        if physical % MMIO_BYTES != 0 || physical + MMIO_BYTES > 1u64 << 32 {
            return Err(Error::Invalid);
        }
        for index in 0..MMIO_BYTES / 4096 {
            let phys = physical + index * 4096;
            if mapper.translate_addr(offset + phys).is_some()
                || mapper
                    .translate_addr(VirtAddr::new(MMIO + index * 4096))
                    .is_some()
            {
                return Err(Error::Invalid);
            }
        }
        for index in 0..MMIO_BYTES / 4096 {
            let page: Page<Size4KiB> = Page::from_start_address(VirtAddr::new(MMIO + index * 4096))
                .map_err(|_| Error::Invalid)?;
            let frame = PhysFrame::from_start_address(PhysAddr::new(physical + index * 4096))
                .map_err(|_| Error::Invalid)?;
            // SAFETY: Boot has exclusive mapper access. Each page is unmapped,
            // aligned and maps only this validated device BAR; UC flags avoid
            // speculative/cached MMIO. No USER bit or cached physical alias exists.
            unsafe {
                mapper
                    .map_to(
                        page,
                        frame,
                        PageTableFlags::PRESENT
                            | PageTableFlags::WRITABLE
                            | PageTableFlags::NO_CACHE
                            | PageTableFlags::WRITE_THROUGH,
                        frames,
                    )
                    .map_err(|_| Error::NoResources)?
                    .flush();
            }
        }
        // Memory decoding must be on for MMIO. DMA is still disabled in RCTL/TCTL
        // until every owned frame and descriptor is initialized below.
        pci::enable_memory(function);
        let mut nic = Self {
            mmio: MMIO as usize,
            rx_ring: 0,
            tx_ring: 0,
            rx_buffers: [0; COUNT],
            tx_buffers: [0; COUNT],
            rx_next: 0,
            tx_next: 0,
            mac: [0; 6],
        };
        nic.write(0xd8, u32::MAX); // IMC: no NIC IRQ until routing is implemented.
        nic.write(0, nic.read(0) | (1 << 26));
        let mut ready = false;
        for _ in 0..100_000 {
            if nic.read(0) & (1 << 26) == 0 {
                ready = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !ready {
            return Err(Error::NoDevice);
        }
        nic.write(0xd8, u32::MAX);
        let _ = nic.read(0xc0);
        nic.write(0x100, 0);
        nic.write(0x400, 0);
        let low = nic.read(0x5400).to_le_bytes();
        let high = nic.read(0x5404).to_le_bytes();
        nic.mac = [low[0], low[1], low[2], low[3], high[0], high[1]];
        if nic.mac == [0; 6] || nic.mac[0] & 1 != 0 {
            return Err(Error::NoDevice);
        }

        // A failed batch restores the allocator cursor. Once this succeeds,
        // initialization has no fallible allocations. Storage is reserved for
        // the device's lifetime and reused for every packet, never per-process.
        let dma = frames.allocate_batch::<66>().ok_or(Error::NoResources)?;
        for frame in &dma {
            let ptr = (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
            // SAFETY: Each unique, newly reserved 4 KiB frame is mapped RAM and
            // has not been published to DMA or any other subsystem.
            unsafe {
                core::ptr::write_bytes(ptr, 0, 4096);
            }
        }
        nic.rx_ring = (offset + dma[0].start_address().as_u64()).as_u64() as usize;
        nic.tx_ring = (offset + dma[1].start_address().as_u64()).as_u64() as usize;
        for index in 0..COUNT {
            nic.rx_buffers[index] =
                (offset + dma[2 + index].start_address().as_u64()).as_u64() as usize;
            nic.tx_buffers[index] =
                (offset + dma[34 + index].start_address().as_u64()).as_u64() as usize;
            // SAFETY: Rings each fit within one owned aligned frame; all indices
            // are below COUNT. Device RX/TX are disabled and no CPU alias exists.
            unsafe {
                (nic.rx_ring as *mut RxDesc).add(index).write(RxDesc {
                    address: dma[2 + index].start_address().as_u64(),
                    length: 0,
                    checksum: 0,
                    status: 0,
                    errors: 0,
                    special: 0,
                });
                (nic.tx_ring as *mut TxDesc).add(index).write(TxDesc {
                    address: dma[34 + index].start_address().as_u64(),
                    length: 0,
                    cso: 0,
                    command: 0,
                    status: 1,
                    css: 0,
                    special: 0,
                });
            }
        }
        nic.write(0x2800, dma[0].start_address().as_u64() as u32);
        nic.write(0x2804, (dma[0].start_address().as_u64() >> 32) as u32);
        nic.write(0x2808, (COUNT * 16) as u32);
        nic.write(0x2810, 0);
        nic.write(0x2818, (COUNT - 1) as u32);
        nic.write(0x3800, dma[1].start_address().as_u64() as u32);
        nic.write(0x3804, (dma[1].start_address().as_u64() >> 32) as u32);
        nic.write(0x3808, (COUNT * 16) as u32);
        nic.write(0x3810, 0);
        nic.write(0x3818, 0);
        nic.write(0x410, 10 | (8 << 10) | (6 << 20));
        // x86 coherent DMA: publish initialized buffers before enabling the NIC.
        // This fence orders CPU stores/device doorbells, not SMP ownership.
        fence(Ordering::SeqCst);
        pci::enable_bus_master(function);
        nic.write(0, nic.read(0) | (1 << 6)); // SLU
        nic.write(0x400, 2 | 8 | (0x10 << 4) | (0x40 << 12));
        nic.write(0x100, 2 | (1 << 15) | (1 << 26)); // EN, broadcast, strip FCS, 2048-byte buffers
        Ok(nic)
    }

    /// Factory MAC address.
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }
    /// Link status as reported by the device (no polling wait).
    pub fn link_up(&self) -> bool {
        self.read(8) & 2 != 0
    }
    fn read(&self, register: usize) -> u32 {
        debug_assert!(register < MMIO_BYTES as usize && register % 4 == 0);
        // SAFETY: init mapped the entire aligned BAR supervisor-only UC. All
        // call sites use aligned in-range registers; volatile is required for MMIO.
        unsafe { ((self.mmio + register) as *const u32).read_volatile() }
    }
    fn write(&self, register: usize, value: u32) {
        debug_assert!(register < MMIO_BYTES as usize && register % 4 == 0);
        // SAFETY: Same valid UC BAR and serialization as read; writing a device
        // register does not create a Rust reference to hardware-owned storage.
        unsafe {
            ((self.mmio + register) as *mut u32).write_volatile(value);
        }
    }
    fn tx_available(&self) -> bool {
        // SAFETY: tx_next is modulo COUNT, descriptor frame is permanently owned.
        // Only the status byte is inspected while DMA may own this descriptor.
        let status = unsafe {
            core::ptr::addr_of!((*(self.tx_ring as *const TxDesc).add(self.tx_next)).status)
                .read_volatile()
        };
        status & 1 != 0
    }
}

/// CPU-owned received bytes, copied before returning the DMA frame to hardware.
pub struct Rx {
    bytes: [u8; FRAME_BYTES],
    len: usize,
}
impl phy::RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.bytes[..self.len])
    }
}
/// Exclusive access to one available TX descriptor during a stack callback.
pub struct Tx<'a>(&'a mut E1000);
impl phy::TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        assert!(len <= FRAME_BYTES, "stack respects Ethernet device MTU");
        let nic = self.0;
        let index = nic.tx_next;
        fence(Ordering::SeqCst);
        // SAFETY: transmit/receive checked DD before lending exclusive access;
        // hardware no longer owns this buffer, len fits its unique 4 KiB frame.
        // The temporary reference ends before the DMA descriptor is published.
        let result =
            f(unsafe { core::slice::from_raw_parts_mut(nic.tx_buffers[index] as *mut u8, len) });
        // SAFETY: index is bounded and descriptor is CPU-owned until TDT moves.
        // Byte fields are naturally aligned; no references alias hardware writes.
        unsafe {
            let desc = (nic.tx_ring as *mut TxDesc).add(index);
            core::ptr::addr_of_mut!((*desc).length).write_volatile(len as u16);
            core::ptr::addr_of_mut!((*desc).command).write_volatile(0x0b); // EOP, IFCS, RS
            core::ptr::addr_of_mut!((*desc).status).write_volatile(0);
        }
        fence(Ordering::SeqCst);
        nic.tx_next = (index + 1) % COUNT;
        nic.write(0x3818, nic.tx_next as u32);
        result
    }
}
impl Device for E1000 {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        if !self.tx_available() {
            return None;
        }
        let index = self.rx_next;
        let desc = (self.rx_ring as *mut RxDesc).wrapping_add(index);
        // SAFETY: RX index is modulo COUNT. Inspect only hardware-owned status
        // until DD indicates ownership returned to the CPU.
        let status = unsafe { core::ptr::addr_of!((*desc).status).read_volatile() };
        if status & 1 == 0 {
            return None;
        }
        fence(Ordering::SeqCst);
        // SAFETY: DD transfers ownership; these fields fit the aligned descriptor.
        let (len, errors) = unsafe {
            (
                core::ptr::addr_of!((*desc).length).read_volatile() as usize,
                core::ptr::addr_of!((*desc).errors).read_volatile(),
            )
        };
        let mut rx = Rx {
            bytes: [0; FRAME_BYTES],
            len: 0,
        };
        if status & 2 != 0 && errors == 0 && (14..=FRAME_BYTES).contains(&len) {
            // SAFETY: Completed RX frame, valid length within its own 4 KiB
            // buffer; copy to distinct CPU-owned storage before rearming DMA.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.rx_buffers[index] as *const u8,
                    rx.bytes.as_mut_ptr(),
                    len,
                );
            }
            rx.len = len;
        }
        // SAFETY: Descriptor is CPU-owned and no reference into its payload is
        // retained. Clearing status then advancing RDT returns ownership to DMA.
        unsafe {
            core::ptr::addr_of_mut!((*desc).status).write_volatile(0);
        }
        fence(Ordering::SeqCst);
        self.write(0x2818, index as u32);
        self.rx_next = (index + 1) % COUNT;
        // An invalid frame becomes an empty token: the stack rejects it while
        // consuming one ingress-budget unit, preventing an unbounded drain loop.
        Some((rx, Tx(self)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        if self.tx_available() {
            Some(Tx(self))
        } else {
            None
        }
    }
    fn capabilities(&self) -> DeviceCapabilities {
        e1000_caps::capabilities()
    }
}
