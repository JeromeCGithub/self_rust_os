//! Process-private synchronization with atomic fast paths and kernel parking.
use crate::thread;
/// Shared ownership for values transferred between native threads.
pub use alloc::sync::Arc;
use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

/// Blocking mutex. Panics abort the process, so poisoning is not exposed.
pub struct Mutex<T: ?Sized> {
    state: AtomicU32,
    value: UnsafeCell<T>,
}
// SAFETY: Moving the mutex transfers its owned value. Access is granted only
// to the one successful acquire owner, which releases before another enters.
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
// SAFETY: The atomic state serializes all access; shared ownership of the mutex
// permits moving T between owners, requiring Send rather than unrestricted Sync.
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
impl<T> Mutex<T> {
    /// An unlocked mutex owning value.
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            value: UnsafeCell::new(value),
        }
    }
    /// Consume exclusive mutex ownership and return the protected value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}
impl<T: ?Sized> Mutex<T> {
    /// Acquire, parking the current thread only when contended.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.state.swap(2, Ordering::Acquire) != 0 {
                let _ = thread::wait(&self.state, 2, None);
            }
        }
        MutexGuard {
            mutex: self,
            owner: PhantomData,
        }
    }
    /// Exclusive access requires no locking when the mutex itself is unique.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}
/// Exclusive access guard. It must be dropped on the acquiring thread.
pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
    owner: PhantomData<*mut ()>,
}
impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: The live guard owns the acquire state until drop.
        unsafe { &*self.mutex.value.get() }
    }
}
impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Unique guard borrow and exclusive acquired mutex ownership.
        unsafe { &mut *self.mutex.value.get() }
    }
}
impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        if self.mutex.state.swap(0, Ordering::Release) == 2 {
            let _ = thread::wake(&self.mutex.state, 1);
        }
    }
}
/// Predicate notification. Always wait in a loop under the associated mutex.
pub struct Condvar {
    sequence: AtomicU32,
}
impl Default for Condvar {
    fn default() -> Self {
        Self::new()
    }
}
impl Condvar {
    /// Empty notification sequence.
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
        }
    }
    /// Release the guard, park unless already notified, and reacquire its mutex.
    pub fn wait<'a, T: ?Sized>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        let generation = self.sequence.load(Ordering::Relaxed);
        let mutex = guard.mutex;
        drop(guard);
        let _ = thread::wait(&self.sequence, generation, None);
        mutex.lock()
    }
    /// Notify one waiter.
    pub fn notify_one(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
        let _ = thread::wake(&self.sequence, 1);
    }
    /// Notify every current waiter.
    pub fn notify_all(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
        let _ = thread::wake(&self.sequence, 1024);
    }
}
/// One-time initialization. A panic aborts the process rather than leaving users.
pub struct Once {
    state: AtomicU32,
}
impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}
impl Once {
    /// Uninitialized state.
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
        }
    }
    /// Run exactly one initializer and publish its writes to every caller.
    pub fn call_once(&self, initialize: impl FnOnce()) {
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
            .is_ok()
        {
            initialize();
            self.state.store(2, Ordering::Release);
            let _ = thread::wake(&self.state, 1024);
        } else {
            while self.state.load(Ordering::Acquire) != 2 {
                let _ = thread::wait(&self.state, 1, None);
            }
        }
    }
}
