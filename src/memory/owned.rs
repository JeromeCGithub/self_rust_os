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
    maximum: usize,
}
impl<'a> FrameTransaction<'a> {
    /// Borrow the serialized pool for one unpublished mapping operation.
    pub fn new(pool: &'a mut RegionFrameAllocator) -> Self {
        Self::bounded(pool, usize::MAX)
    }
    /// Cap all allocations in this transaction, including page-table parents.
    /// Zero refuses allocation without touching the pool or allocating a ledger.
    pub fn bounded(pool: &'a mut RegionFrameAllocator, maximum: usize) -> Self {
        Self {
            pool,
            frames: Vec::new(),
            maximum,
        }
    }
    /// Number of owned frames, available before commit for atomic budget charging.
    pub fn allocated_frames(&self) -> usize {
        self.frames.len()
    }
    /// Selected persistent heap storage used by this frame ownership ledger.
    pub fn metadata_bytes(&self) -> usize {
        self.frames.capacity() * core::mem::size_of::<PhysFrame>()
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
        if self.frames.len() >= self.maximum {
            return None;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemRegion;
    static REGIONS: [MemRegion; 1] = [MemRegion {
        start: 0x1000,
        end: 0x9000,
    }];
    #[test_case]
    fn transaction_frame_ceiling_precedes_allocation_and_rolls_back_repeatedly() {
        // SAFETY: A local ledger-only fixture: frame addresses are never mapped,
        // dereferenced or transferred to a real allocator. No physical alias or
        // memory reference is created, and the fixture owns its complete state.
        let mut pool = unsafe { RegionFrameAllocator::new(&REGIONS) };
        pool.enable_recycling();
        for _ in 0..3 {
            for maximum in [0, 1, 3, 8, 9] {
                {
                    let mut transaction = FrameTransaction::bounded(&mut pool, maximum);
                    for _ in 0..maximum.min(8) {
                        assert!(transaction.allocate_frame().is_some());
                    }
                    assert!(
                        transaction.allocate_frame().is_none(),
                        "frame ceiling {maximum}"
                    );
                }
                assert_eq!(
                    pool.allocated(),
                    0,
                    "bounded rollback must return every frame"
                );
            }
        }
    }
    #[test_case]
    fn committed_bounded_frames_keep_ownership_and_default_remains_unbounded() {
        // SAFETY: This disjoint local allocator tracks scalar frame identities
        // only; the test never maps/dereferences them or shares the allocator.
        let mut pool = unsafe { RegionFrameAllocator::new(&REGIONS) };
        pool.enable_recycling();
        let frames = {
            let mut transaction = FrameTransaction::bounded(&mut pool, 2);
            assert!(transaction.allocate_frame().is_some());
            assert!(transaction.allocate_frame().is_some());
            assert!(transaction.allocate_frame().is_none());
            transaction.commit()
        };
        assert_eq!(frames.len(), 2);
        assert_ne!(frames[0], frames[1]);
        assert_eq!(pool.allocated(), 2);
        for frame in frames {
            // SAFETY: The committed ledger is the sole owner, no mappings or
            // references exist, and each frame is returned exactly once.
            unsafe {
                pool.recycle(frame);
            }
        }
        {
            let mut transaction = FrameTransaction::new(&mut pool);
            for _ in 0..8 {
                assert!(transaction.allocate_frame().is_some());
            }
            assert!(transaction.allocate_frame().is_none());
        }
        assert_eq!(pool.allocated(), 0);
    }
}
