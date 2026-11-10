//! Bounded lifetime pins for filesystem operations owned by a process.
use core::sync::atomic::{AtomicU64, Ordering};

const CANCELLED: u64 = 1 << 63;
const RELEASED: u64 = 1 << 62;
const COUNT: u64 = RELEASED - 1;

#[derive(Debug)]
struct Cell {
    owner: AtomicU64,
    state: AtomicU64,
}
impl Cell {
    const fn new() -> Self {
        Self {
            owner: AtomicU64::new(0),
            state: AtomicU64::new(0),
        }
    }
    fn release(&self, generation: u64) {
        // Only the final lease or its owner can release this generation. A
        // successful release publishes all previous state before reuse.
        let _ = self
            .owner
            .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}
static CELLS: [Cell; 128] = [const { Cell::new() }; 128];
static NEXT: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Owner {
    cell: &'static Cell,
    generation: u64,
}
impl Owner {
    pub(crate) fn new() -> Option<Self> {
        let generation = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .ok()?;
        for cell in &CELLS {
            if cell
                .owner
                .compare_exchange(0, generation, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                cell.state.store(0, Ordering::Release);
                return Some(Self { cell, generation });
            }
        }
        None
    }
    pub(crate) fn begin(&self) -> Option<FilesystemOperation> {
        let mut state = self.cell.state.load(Ordering::Acquire);
        loop {
            if state & (CANCELLED | RELEASED) != 0 || state & COUNT == COUNT {
                return None;
            }
            match self.cell.state.compare_exchange_weak(
                state,
                state + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(FilesystemOperation {
                        cell: self.cell,
                        generation: self.generation,
                    })
                }
                Err(next) => state = next,
            }
        }
    }
    pub(crate) fn cancel(&self) {
        self.cell.state.fetch_or(CANCELLED, Ordering::AcqRel);
    }
    pub(crate) fn active(&self) -> bool {
        self.cell.state.load(Ordering::Acquire) & COUNT != 0
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let old = self
            .cell
            .state
            .fetch_or(CANCELLED | RELEASED, Ordering::AcqRel);
        if old & COUNT == 0 {
            self.cell.release(self.generation);
        }
    }
}
/// Copyable, generation-checked cancellation observation for locked I/O.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cancellation {
    cell: &'static Cell,
    generation: u64,
}
impl Cancellation {
    pub(crate) fn cancelled(self) -> bool {
        self.cell.owner.load(Ordering::Acquire) != self.generation
            || self.cell.state.load(Ordering::Acquire) & CANCELLED != 0
    }
    #[cfg(test)]
    pub(crate) fn cancel_for_test(self) {
        if self.cell.owner.load(Ordering::Acquire) == self.generation {
            self.cell.state.fetch_or(CANCELLED, Ordering::AcqRel);
        }
    }
}
/// Pins the issuing process until an admitted filesystem operation completes.
pub struct FilesystemOperation {
    cell: &'static Cell,
    generation: u64,
}
impl FilesystemOperation {
    /// Whether the owning process has been stopped.
    pub fn cancelled(&self) -> bool {
        self.cancellation().cancelled()
    }
    pub(crate) fn cancellation(&self) -> Cancellation {
        Cancellation {
            cell: self.cell,
            generation: self.generation,
        }
    }
}
impl Drop for FilesystemOperation {
    fn drop(&mut self) {
        let old = self.cell.state.fetch_sub(1, Ordering::AcqRel);
        if old & COUNT == 1 && old & RELEASED != 0 {
            self.cell.release(self.generation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Owner;

    #[test_case]
    fn cancelled_owner_keeps_slot_until_last_operation_and_reuses_new_generation() {
        let owner = Owner::new().unwrap();
        let address = core::ptr::from_ref(owner.cell);
        let generation = owner.generation;
        let first = owner.begin().unwrap();
        let stale = first.cancellation();
        let second = owner.begin().unwrap();
        owner.cancel();
        assert!(owner.begin().is_none());
        assert!(first.cancelled() && second.cancelled());
        drop(owner);
        let other = Owner::new().unwrap();
        assert_ne!(core::ptr::from_ref(other.cell), address);
        drop(first);
        let still_other = Owner::new().unwrap();
        assert_ne!(core::ptr::from_ref(still_other.cell), address);
        drop(second);
        let reused = Owner::new().unwrap();
        assert_eq!(core::ptr::from_ref(reused.cell), address);
        assert_ne!(reused.generation, generation);
        assert!(
            stale.cancelled(),
            "a stale token must fail closed after reuse"
        );
        assert!(!reused.begin().unwrap().cancelled());
    }
}
