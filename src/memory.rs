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

/// A physical memory region: `[start, end)` in bytes.
///
/// Used by boot paths that do not go through the `bootloader` crate (e.g.
/// GRUB Multiboot2) to describe RAM discovered from the firmware memory map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemRegion {
    /// Inclusive start address.
    pub start: u64,
    /// Exclusive end address.
    pub end: u64,
}

impl MemRegion {
    /// Creates a region, returning `None` when empty or inverted.
    #[must_use]
    pub const fn new(start: u64, end: u64) -> Option<Self> {
        if start < end {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// Size of the region in bytes.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end - self.start
    }
}

/// A frame allocator that hands out frames from a static list of physical
/// memory regions.
///
/// Unlike [`BootInfoFrameAllocator`], the regions come from the caller (for
/// example parsed from a Multiboot2 memory map) instead of the `bootloader`
/// crate's `BootInfo`. Regions must be sorted, non-overlapping, and must not
/// cover memory owned by the kernel image itself.
pub struct RegionFrameAllocator {
    regions: &'static [MemRegion],
    next: usize,
}

impl RegionFrameAllocator {
    /// Creates an allocator over the given regions.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the regions describe usable RAM that
    /// the kernel is free to hand out.
    #[must_use]
    pub const unsafe fn new(regions: &'static [MemRegion]) -> Self {
        Self { regions, next: 0 }
    }

    /// Reserve a fixed batch atomically, rolling back on insufficient memory.
    pub fn allocate_batch<const N: usize>(&mut self) -> Option<[PhysFrame; N]> {
        let checkpoint = self.next;
        let mut result = [PhysFrame::containing_address(PhysAddr::new(0)); N];
        for slot in &mut result {
            match self.allocate_frame() {
                Some(frame) => *slot = frame,
                None => {
                    self.next = checkpoint;
                    return None;
                }
            }
        }
        Some(result)
    }
}

/// Implement the `FrameAllocator` trait for `RegionFrameAllocator`.
///
/// # Safety
///
/// Unsafe because the caller must guarantee that the regions are valid.
unsafe impl FrameAllocator<Size4KiB> for RegionFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        // Skip whole regions arithmetically instead of walking all previous
        // frames on every allocation (desktop surfaces require thousands).
        let mut index = self.next as u64;
        for region in self.regions {
            let frames = region.len().div_ceil(4096);
            if index < frames {
                self.next += 1;
                return Some(PhysFrame::containing_address(PhysAddr::new(
                    region.start + index * 4096,
                )));
            }
            index -= frames;
        }
        None
    }
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
}

/// Implement the `FrameAllocator` trait for `BootInfoFrameAllocator`.
///
/// SAFETY:
///
/// Unsafe because the caller must guarantee that the memory map is valid.
unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        // Bootloader ranges are page-aligned. Skip whole usable regions rather
        // than rescanning every previously allocated frame: a 64 MiB heap maps
        // over 16,000 pages, making nth(next) quadratic during debug boot.
        let mut index = self.next as u64;
        for region in self
            .memory_map
            .iter()
            .filter(|r| r.region_type == MemoryRegionType::Usable)
        {
            let count = (region.range.end_addr() - region.range.start_addr()) / 4096;
            if index < count {
                self.next += 1;
                return Some(PhysFrame::containing_address(PhysAddr::new(
                    region.range.start_addr() + index * 4096,
                )));
            }
            index -= count;
        }
        None
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test_case]
    fn batch_failure_restores_allocator_cursor() {
        static REGIONS: [MemRegion; 1] = [MemRegion {
            start: 0x2000,
            end: 0x4000,
        }];
        // SAFETY: No memory is dereferenced; this allocator is local to the test.
        let mut frames = unsafe { RegionFrameAllocator::new(&REGIONS) };
        assert!(frames.allocate_batch::<3>().is_none());
        let batch = frames
            .allocate_batch::<2>()
            .expect("failed reservation must preserve frames");
        assert_eq!(batch[0].start_address().as_u64(), 0x2000);
        assert_eq!(batch[1].start_address().as_u64(), 0x3000);
        assert!(frames.allocate_batch::<1>().is_none());
        assert!(frames.allocate_batch::<0>().is_some());
    }
}

#[cfg(test)]
mod boot_frame_tests {
    use super::*;
    use alloc::boxed::Box;
    use bootloader::bootinfo::{FrameRange, MemoryRegion};

    #[test_case]
    fn usable_frames_skip_reserved_regions_and_never_repeat_after_exhaustion() {
        let mut map = Box::new(MemoryMap::new());
        for (start, end, region_type) in [
            (0x1000, 0x4000, MemoryRegionType::Usable),
            (0x4000, 0x8000, MemoryRegionType::Reserved),
            (0x8000, 0xa000, MemoryRegionType::Usable),
        ] {
            map.add_region(MemoryRegion {
                range: FrameRange::new(start, end),
                region_type,
            });
        }
        // SAFETY: This test never maps/dereferences the returned frames. The map
        // is immutable and leaked for its required static lifetime; allocator is local.
        let mut frames = unsafe { BootInfoFrameAllocator::new(Box::leak(map)) };
        for expected in [0x1000, 0x2000, 0x3000, 0x8000, 0x9000] {
            assert_eq!(
                frames.allocate_frame().unwrap().start_address().as_u64(),
                expected
            );
        }
        for _ in 0..3 {
            assert!(frames.allocate_frame().is_none());
        }
    }

    #[test_case]
    fn empty_memory_map_returns_no_frames() {
        let map = Box::leak(Box::new(MemoryMap::new()));
        // SAFETY: Empty immutable map has no frames to hand out or dereference.
        let mut frames = unsafe { BootInfoFrameAllocator::new(map) };
        assert!(frames.allocate_frame().is_none());
        assert!(frames.allocate_frame().is_none());
    }
}
