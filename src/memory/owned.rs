//! Fallible frame-allocation transactions and explicit process/frame ownership.
use super::RegionFrameAllocator;
use alloc::vec::Vec;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};

/// Own every newly allocated frame until commit. Failed construction rolls back.
/// Callers remove any partially published live mappings and complete shootdown
/// before dropping; an entirely unpublished address-space tree can be discarded.
pub struct FrameTransaction<'a> {
    pool: &'a mut RegionFrameAllocator,
    frames: Vec<PhysFrame>,
}
impl<'a> FrameTransaction<'a> {
    /// Borrow the serialized pool for one unpublished mapping operation.
    pub fn new(pool: &'a mut RegionFrameAllocator) -> Self {
        Self {
            pool,
            frames: Vec::new(),
        }
    }
    /// Transfer frame ownership to the completed address space.
    pub fn commit(mut self) -> Vec<PhysFrame> {
        core::mem::take(&mut self.frames)
    }
}
// SAFETY: The uniquely borrowed pool hands out disjoint frames; the ledger
// reserves before allocation and records each issued frame exactly once.
unsafe impl FrameAllocator<Size4KiB> for FrameTransaction<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        self.frames.try_reserve(1).ok()?;
        let frame = self.pool.allocate_frame()?;
        self.frames.push(frame);
        Some(frame)
    }
}
impl Drop for FrameTransaction<'_> {
    fn drop(&mut self) {
        for frame in self.frames.drain(..) {
            // SAFETY: Process construction discards an unpublished private tree.
            // Live stack construction removes its partial leaves and completes
            // shootdown before drop. Both callers retain exclusive frame ownership.
            unsafe {
                self.pool.recycle(frame);
            }
        }
    }
}
