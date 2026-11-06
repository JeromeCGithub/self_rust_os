//! Process startup arguments. Safe strings plus raw C-style argc/argv access.
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use rustos_process_abi::{MAX_STARTUP_BYTES as DATA_SIZE, MAX_VECTOR as MAX_ARGS};

// Initialized once before main and thread creation, then never modified. The
// release/acquire stores publish immutable startup metadata, not a mutable
// multi-field protocol. Initial stack frames belong to the whole process even
// if its initial thread later exits while other threads continue.
static COUNT: AtomicUsize = AtomicUsize::new(0);
static VECTOR: AtomicPtr<*const u8> = AtomicPtr::new(core::ptr::null_mut());

/// Install the kernel entry registers before any application code executes.
/// # Safety
/// The private initial stack holds an aligned argv table with argc+1 entries,
/// a null final pointer and valid immutable UTF-8 C strings totalling at most
/// DATA_SIZE bytes. Storage remains mapped throughout this process's lifetime.
pub(crate) unsafe fn initialize(count: usize, vector: *const *const u8) {
    if count > MAX_ARGS || (count != 0 && vector.is_null()) {
        crate::sys::exit(1);
    }
    VECTOR.store(vector as *mut *const u8, Ordering::Release);
    COUNT.store(count, Ordering::Release);
}
/// Number of startup arguments, including argv[0] when present.
pub fn argc() -> usize {
    COUNT.load(Ordering::Acquire)
}
/// Raw, null-terminated C-style pointer vector. Dereferencing requires unsafe;
/// callers must not mutate the table or strings while safe references exist.
pub fn argv() -> *const *const u8 {
    VECTOR.load(Ordering::Acquire)
}
/// Return one startup argument without allocation.
pub fn arg(index: usize) -> Option<&'static str> {
    if index >= argc() {
        return None;
    }
    // SAFETY: Runtime initialization receives this bounded, aligned table from
    // the kernel. The index is below argc, and all strings are private owned
    // initial-stack storage. No safe SDK operation mutates or unmaps it. Unsafe
    // callers of raw argv must uphold the shared-reference immutability rule.
    unsafe {
        let pointer = argv().add(index).read();
        for length in 0..DATA_SIZE {
            if pointer.add(length).read() == 0 {
                return core::str::from_utf8(core::slice::from_raw_parts(pointer, length)).ok();
            }
        }
    }
    None
}
/// Allocation-free iterator over startup argument strings.
pub struct Args {
    next: usize,
    count: usize,
}
/// Iterate argv[0] and the remaining arguments in order.
pub fn args() -> Args {
    Args {
        next: 0,
        count: argc(),
    }
}
impl Iterator for Args {
    type Item = &'static str;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.count {
            return None;
        }
        let argument = arg(self.next);
        self.next += 1;
        argument
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.count - self.next;
        (remaining, Some(remaining))
    }
}
impl ExactSizeIterator for Args {}

/// Shared application-root dispatch for supported main signatures.
#[doc(hidden)]
pub trait Entry<A> {
    fn call(self);
}
impl<F: FnOnce()> Entry<()> for F {
    fn call(self) {
        self();
    }
}
impl<F: FnOnce(usize, &[&str])> Entry<(usize,)> for F {
    fn call(self) {
        let mut strings = [""; MAX_ARGS];
        let count = argc();
        for (slot, argument) in strings.iter_mut().zip(args()) {
            *slot = argument;
        }
        self(count, &strings[..count]);
    }
}
/// Called by the shared application root; keeps fn main() source compatible.
#[doc(hidden)]
pub fn invoke<A>(entry: impl Entry<A>) {
    entry.call();
}
