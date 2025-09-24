//! Memory management module for setting up paging and frame allocation.

use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use x86_64::{
    registers::control::Cr3,
    structures::paging::{FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

/// Initialize a new `OffsetPageTable`.
///
/// # Safety
/// Unsafe because the caller must guarantee that the physical memory is mapped
/// to virtual memory at the passed `physical_memory_offset`.
/// This function must be only called once to avoid aliasing `&mut` references.
#[must_use]
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

/// Returns a mutable reference to the active level 4 table.
///
/// # Safety
/// Unsafe because the caller must guarantee that the physical memory is mapped
/// to virtual memory at the passed `physical_memory_offset`.
/// This function must be only called once to avoid aliasing `&mut` references.
#[must_use]
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    // Cr3 register holds the physical address of the level 4 page table.
    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}

/// A frame allocator that returns usable frames from the bootloader's memory map.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
}

impl BootInfoFrameAllocator {
    /// Create a `BootInfoFrameAllocator` from the passed memory map.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the passed memory map is valid.
    #[must_use]
    pub const unsafe fn new(memory_map: &'static MemoryMap) -> Self {
        Self {
            memory_map,
            next: 0,
        }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        let regions = self.memory_map.iter();
        let usable_regions = regions.filter(|r| r.region_type == MemoryRegionType::Usable);
        let addr_ranges = usable_regions.map(|r| r.range.start_addr()..r.range.end_addr());
        let frame_addresses = addr_ranges.flat_map(|r| r.step_by(4096));
        frame_addresses.map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

/// Implement the `FrameAllocator` trait for `BootInfoFrameAllocator`.
///
/// SAFETY:
///
/// Unsafe because the caller must guarantee that the memory map is valid.
unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}
