//! Reclaimed private process heap for TLS allocations. No kernel heap pointers.
use core::{
    alloc::{GlobalAlloc, Layout},
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr::NonNull,
};
use linked_list_allocator::Heap;
const SIZE: usize = 16 * 1024 * 1024;
#[repr(align(16))]
struct Storage(UnsafeCell<[MaybeUninit<u8>; SIZE]>);
// SAFETY: Only the allocator initializes/partitions this permanently mapped BSS
// region, under its mutex. Returned allocations are disjoint and exclusively
// owned by callers until deallocation. No IRQ or other process shares this heap.
unsafe impl Sync for Storage {}
static STORAGE: Storage = Storage(UnsafeCell::new([MaybeUninit::uninit(); SIZE]));
struct Allocator(spin::Mutex<Heap>);
// SAFETY: The mutex serializes all free-list operations and one-time setup.
// Layouts are forwarded unchanged. The backing BSS is aligned, stable, writable
// private process memory; each freed block was returned by this same allocator.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.0.lock();
        if heap.size() == 0 {
            // SAFETY: First locked access, before any allocation exists. This static
            // region lives for the process lifetime and has no references/other owners.
            unsafe {
                heap.init(STORAGE.0.get().cast::<u8>() as usize, SIZE);
            }
        }
        heap.allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), |p| p.as_ptr())
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if let Some(pointer) = NonNull::new(pointer) {
            // SAFETY: GlobalAlloc's caller guarantees matching live block and layout;
            // serialized metadata mutation returns that unique block to this heap.
            unsafe {
                self.0.lock().deallocate(pointer, layout);
            }
        }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator(spin::Mutex::new(Heap::empty()));
pub fn used() -> usize {
    ALLOCATOR.0.lock().used()
}
#[alloc_error_handler]
fn exhausted(_layout: Layout) -> ! {
    crate::sys::console_write(b"TLS userspace heap exhausted\n");
    crate::sys::exit(12)
}
