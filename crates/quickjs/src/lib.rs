//! Restricted upstream engine feasibility adapter. No browser or OS capabilities.
#![no_std]
extern crate alloc;
use alloc::{ffi::CString, rc::Rc, string::String, vec};
use core::{
    ffi::{c_char, c_void},
    marker::PhantomData,
    ptr::NonNull,
};
#[cfg(any(target_os = "none", feature = "freestanding"))]
mod platform;
/// Failure classes intentionally omit page source and exception text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    InvalidInput,
    Exception,
    Interrupted,
    Memory,
    OutputLimit,
}
/// Per-realm engine budgets. Reallocation conservatively counts both live blocks.
#[derive(Clone, Copy)]
pub struct Limits {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub interrupt_polls: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 4 * 1024 * 1024,
            stack_bytes: 64 * 1024,
            interrupt_polls: 100,
        }
    }
}
/// Engine allocator metrics, excluding Rust result/source buffers and bridge state.
#[derive(Default, Debug)]
#[repr(C)]
pub struct Stats {
    pub allocated_bytes: usize,
    pub peak_bytes: usize,
    pub allocations: usize,
    pub interrupt_polls: u64,
}
extern "C" {
    fn rjs_create(memory: usize, stack: usize, polls: u64) -> *mut c_void;
    fn rjs_destroy(runtime: *mut c_void);
    fn rjs_eval(
        runtime: *mut c_void,
        source: *const c_char,
        length: usize,
        output: *mut u8,
        capacity: usize,
        written: *mut usize,
    ) -> i32;
    fn rjs_stats(runtime: *const c_void, stats: *mut Stats);
}
/// Owns one context/runtime on its creating thread; it is neither Send nor Sync.
pub struct Runtime {
    raw: NonNull<c_void>,
    _thread: PhantomData<Rc<()>>,
}
impl Runtime {
    /// Construct the restricted profile. Date, Atomics, host I/O and DOM are absent.
    pub fn new(limits: Limits) -> Result<Self, Error> {
        if limits.memory_bytes == 0
            || limits.memory_bytes > 64 * 1024 * 1024
            || !(16384..=131072).contains(&limits.stack_bytes)
            || limits.interrupt_polls == 0
            || limits.interrupt_polls > 1_000_000
        {
            return Err(Error::InvalidInput);
        }
        // SAFETY: Scalar budgets are validated above. C creates and returns sole owned
        // opaque state or null; no borrowed Rust address is retained by the engine.
        let raw = NonNull::new(unsafe {
            rjs_create(
                limits.memory_bytes,
                limits.stack_bytes,
                limits.interrupt_polls,
            )
        })
        .ok_or(Error::Memory)?;
        Ok(Self {
            raw,
            _thread: PhantomData,
        })
    }
    /// Evaluate a bounded UTF-8 classic script and copy its string result.
    /// Poll limits bound engine callbacks, not exact instruction or wall-clock counts.
    pub fn eval(&mut self, source: &str) -> Result<String, Error> {
        if source.len() > 1024 * 1024 {
            return Err(Error::InvalidInput);
        }
        let source = CString::new(source).map_err(|_| Error::InvalidInput)?;
        let mut output = vec![0u8; 8192];
        let mut written = 0;
        // SAFETY: Exclusive self owns a live C runtime on the creator thread. CString
        // supplies complete input plus the terminator required by JS_Eval. Output and
        // length are exclusive initialized buffers with exactly the declared capacity;
        // C copies at most capacity bytes and retains neither buffer after this call.
        let status = unsafe {
            rjs_eval(
                self.raw.as_ptr(),
                source.as_ptr(),
                source.as_bytes().len(),
                output.as_mut_ptr(),
                output.len(),
                &mut written,
            )
        };
        match status {
            0 if written <= output.len() => {
                output.truncate(written);
                String::from_utf8(output).map_err(|_| Error::Exception)
            }
            1 => Err(Error::Exception),
            2 => Err(Error::Interrupted),
            3 => Err(Error::Memory),
            4 => Err(Error::OutputLimit),
            _ => Err(Error::Unavailable),
        }
    }
    /// Snapshot engine counters without publishing any foreign pointers.
    pub fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        // SAFETY: self keeps C state live and thread-confined. C writes exactly this
        // repr(C) record synchronously; no aliases or references escape the call.
        unsafe { rjs_stats(self.raw.as_ptr(), &mut stats) };
        stats
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: This unique owner destroys the context before its runtime exactly
        // once, on the creator thread. No C value or pointer escaped the adapter.
        unsafe { rjs_destroy(self.raw.as_ptr()) };
    }
}
/// Native platform allocation blocks still live (host libc is not instrumented).
pub fn native_allocations() -> Option<usize> {
    #[cfg(any(target_os = "none", feature = "freestanding"))]
    {
        Some(platform::live_blocks())
    }
    #[cfg(not(any(target_os = "none", feature = "freestanding")))]
    {
        None
    }
}

/// Validate the native allocation/format adapter; unavailable on the libc profile.
pub fn native_platform_check() -> Option<bool> {
    #[cfg(any(target_os = "none", feature = "freestanding"))]
    {
        extern "C" {
            fn rjs_platform_check() -> i32;
        }
        // SAFETY: The C self-test owns all temporary buffers/allocations, exposes no
        // pointers and reports only whether the native ABI contracts held.
        Some(unsafe { rjs_platform_check() } == 1)
    }
    #[cfg(not(any(target_os = "none", feature = "freestanding")))]
    {
        None
    }
}
