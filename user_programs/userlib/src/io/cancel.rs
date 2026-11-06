//! Owned, monotonic cancellation shared between trusted application workers.
extern crate alloc;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
/// Each new token owns a distinct flag; clones share its lifetime, not task data.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    /// Create a fresh uncancelled task token.
    pub fn new() -> Self {
        Self::default()
    }
    /// Cancel this task and all clones; the flag never resets.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release)
    }
    /// Acquire the cancellation published by another application thread.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clones_observe_cancellation_across_threads_and_new_tasks_remain_independent() {
        let original = Cancellation::new();
        let clone = original.clone();
        let other = Cancellation::new();
        std::thread::spawn(move || clone.cancel()).join().unwrap();
        assert!(original.is_cancelled());
        assert!(!other.is_cancelled());
        original.cancel();
        assert!(original.is_cancelled());
    }
}
