//! Reusable process-private heap grown on demand through the program-break ABI.
use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::NonNull,
};
use linked_list_allocator::Heap;
struct Allocator(spin::Mutex<Heap>);
// SAFETY: One mutex serializes the free list and growth. The kernel returns
// disjoint zero-filled writable private pages at a stable virtual base; growth
// is contiguous and fallible. Returned allocations remain live until matching
// deallocation, and this allocator exclusively controls the program break.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.0.lock();
        if let Ok(pointer) = heap.allocate_first_fit(layout) {
            return pointer.as_ptr();
        }
        let Some(needed) = layout
            .size()
            .checked_add(layout.align())
            .and_then(|n| n.checked_add(4095))
            .map(|n| n & !4095)
        else {
            return core::ptr::null_mut();
        };
        let growth = needed.max(64 * 1024).max(heap.size().min(16 * 1024 * 1024));
        // SAFETY: The allocator holds its exclusive growth/free-list lock. A
        // query retains no pointers; no other safe SDK code changes this break.
        let base = if heap.size() == 0 {
            unsafe { crate::sys::brk(0) }
        } else {
            heap.bottom()
        };
        let Some(end) = base
            .checked_add(heap.size())
            .and_then(|n| n.checked_add(growth))
        else {
            return core::ptr::null_mut();
        };
        // SAFETY: Only append to this allocator's region; no live allocation is
        // moved or unmapped. Failure leaves the old break and heap unchanged.
        if unsafe { crate::sys::brk(end) } != end {
            return core::ptr::null_mut();
        }
        // SAFETY: Successful growth maps the entire contiguous byte range, page
        // aligned and private for process lifetime. No Rust references exist to
        // the new pages. The same mutex owns initial setup and every extension.
        unsafe {
            if heap.size() == 0 {
                heap.init(base, growth);
            } else {
                heap.extend(growth);
            }
        }
        heap.allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), |p| p.as_ptr())
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if let Some(pointer) = NonNull::new(pointer) {
            // SAFETY: GlobalAlloc's caller supplies the same live block/layout;
            // serialized deallocation makes it reusable without kernel I/O.
            unsafe {
                self.0.lock().deallocate(pointer, layout);
            }
        }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator(spin::Mutex::new(Heap::empty()));
#[cfg(feature = "tls")]
pub fn used() -> usize {
    ALLOCATOR.0.lock().used()
}
#[alloc_error_handler]
fn exhausted(_layout: Layout) -> ! {
    crate::sys::console_write(b"userspace allocation failed\n");
    crate::sys::exit(12)
}
