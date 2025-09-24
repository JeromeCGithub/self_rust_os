//! A simple heap allocator using the `linked_list_allocator` crate.

use linked_list_allocator::LockedHeap;
use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};

/// Start address of the heap.
pub const HEART_START: usize = 0x_4444_4444_0000;
/// Size of the heap in bytes.
pub const HEAP_SIZE: usize = 1000 * 1024; // 1 MiB

/// Initialize the heap.
///
/// # Errors
/// Might fail if the physical memory frame allocator runs out of memory.
#[expect(clippy::impl_trait_in_params)]
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEART_START as u64);
        let head_end = heap_start + HEAP_SIZE - 1_u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(head_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        // SAFETY:
        //
        // The caller must guarantee that the `page` is not already mapped.
        // As we just allocated a new frame, it is guaranteed that the frame is unused.
        // Also, we have to call init_heap only once.
        unsafe { mapper.map_to(page, frame, flags, frame_allocator)?.flush() }
    }

    // SAFETY:
    //
    // Init the allocator is unsafe because the caller must guarantee that the
    // heap memory is not used for other purposes.
    unsafe {
        ALLOCATOR.lock().init(HEART_START, HEAP_SIZE);
    }

    Ok(())
}

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();
