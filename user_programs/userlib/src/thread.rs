//! Native kernel-scheduled threads. Panics terminate the process (panic=abort).
use crate::sys::{self, SYSCALL_ERROR};
use alloc::{boxed::Box, sync::Arc};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use rustos_thread_abi as abi;

/// Creation/join failure. Allocation exhaustion in the user heap remains fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error;
fn call(number: u64, a: u64, b: u64, c: u64) -> Result<u64, Error> {
    // SAFETY: Callers provide the documented scalar arguments or a complete
    // borrowed Create record. The kernel never retains Rust references.
    match unsafe { sys::syscall(number, a, b, c) } {
        SYSCALL_ERROR => Err(Error),
        n => Ok(n),
    }
}
/// Available logical CPUs, from one through 32.
pub fn available_parallelism() -> usize {
    call(abi::SYS_CPU_COUNT, 0, 0, 0).unwrap_or(1) as usize
}
/// Dense CPU index at this instant; the thread may migrate after the call.
pub fn current_cpu() -> usize {
    call(abi::SYS_CURRENT_CPU, 0, 0, 0).unwrap_or(0) as usize
}
/// Generation-checked identifier for the calling native thread.
pub fn current() -> u64 {
    call(abi::SYS_THREAD_CURRENT, 0, 0, 0).unwrap_or(0)
}
/// Allow another runnable thread to execute, including with only one CPU.
pub fn yield_now() {
    let _ = call(abi::SYS_THREAD_YIELD, 0, 0, 0);
}
/// Sleep this thread in existing PIT tick units (approximately 55 ms each).
pub fn sleep_ticks(ticks: u64) -> Result<(), Error> {
    sys::sleep_ticks(ticks).map_err(|_| Error)
}

/// Requested worker stack and optional CPU affinity. Initial threads use the
/// existing larger main stack. Worker stacks default to 256 KiB plus a guard.
pub struct Builder {
    stack_size: u64,
    cpu: Option<usize>,
}
impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}
impl Builder {
    /// Default guarded stack and unrestricted scheduling.
    pub const fn new() -> Self {
        Self {
            stack_size: abi::DEFAULT_STACK,
            cpu: None,
        }
    }
    /// Usable stack bytes, from 16 KiB through 256 KiB, rounded up to pages.
    pub fn stack_size(mut self, bytes: usize) -> Self {
        self.stack_size = bytes as u64;
        self
    }
    /// Restrict this worker to an online logical CPU.
    pub fn on_cpu(mut self, cpu: usize) -> Self {
        self.cpu = Some(cpu);
        self
    }
    /// Transfer a closure to a new Ring 3 execution context.
    pub fn spawn<F, T>(self, f: F) -> Result<JoinHandle<T>, Error>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let cpus = available_parallelism();
        let affinity = match self.cpu {
            Some(cpu) if cpu < cpus => 1u64 << cpu,
            Some(_) => return Err(Error),
            None => 0,
        };
        if !(16384..=abi::MAX_STACK).contains(&self.stack_size) {
            return Err(Error);
        }
        let packet = Arc::new(Packet {
            value: UnsafeCell::new(None),
            done: AtomicBool::new(false),
        });
        let start = Box::into_raw(Box::new(Start {
            f,
            packet: Arc::clone(&packet),
        }));
        let request = abi::Create {
            version: abi::VERSION,
            flags: 0,
            entry: trampoline::<F, T> as *const () as u64,
            argument: start as u64,
            stack_size: self.stack_size,
            affinity,
        };
        let result = call(
            abi::SYS_THREAD_CREATE,
            core::ptr::addr_of!(request) as u64,
            core::mem::size_of::<abi::Create>() as u64,
            0,
        );
        match result {
            Ok(tid) => Ok(JoinHandle {
                tid: Some(tid),
                packet,
            }),
            Err(error) => {
                // SAFETY: Failed create publishes no worker; ownership of the
                // original exact Box remains here and is reclaimed once.
                unsafe {
                    drop(Box::from_raw(start));
                }
                Err(error)
            }
        }
    }
}
struct Packet<T> {
    value: UnsafeCell<Option<T>>,
    done: AtomicBool,
}
// SAFETY: Exactly one worker writes value. Exactly one consumed JoinHandle
// reads/takes it after kernel join and the release/acquire completion flag.
// Arc keeps storage alive through detach, failed spawn and early worker exit.
unsafe impl<T: Send> Sync for Packet<T> {}
struct Start<F, T> {
    f: F,
    packet: Arc<Packet<T>>,
}
extern "C" fn trampoline<F, T>(argument: usize) -> !
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // SAFETY: spawn transfers this exact, aligned, uniquely owned Box only on
    // successful creation. This trampoline is entered exactly once with it.
    let start = unsafe { Box::from_raw(argument as *mut Start<F, T>) };
    let Start { f, packet } = *start;
    let result = f();
    // SAFETY: The worker is the sole writer, and no joiner can take value until
    // the release below and successful kernel completion/join.
    unsafe {
        *packet.value.get() = Some(result);
    }
    packet.done.store(true, Ordering::Release);
    drop(packet);
    let _ = call(abi::SYS_THREAD_EXIT, 0, 0, 0);
    // If a broken kernel returns, abort the process without an invalid return.
    sys::exit(1)
}
/// Unique join ownership. Dropping the handle detaches without blocking.
pub struct JoinHandle<T: Send + 'static> {
    tid: Option<u64>,
    packet: Arc<Packet<T>>,
}
impl<T: Send + 'static> JoinHandle<T> {
    /// Native identifier, useful for diagnostics.
    pub fn id(&self) -> u64 {
        self.tid.expect("live join handle")
    }
    /// Whether the closure has published its result; the exit may still finish.
    pub fn is_finished(&self) -> bool {
        self.packet.done.load(Ordering::Acquire)
    }
    /// Wait for normal return and transfer its result to this thread.
    pub fn join(mut self) -> Result<T, Error> {
        call(abi::SYS_THREAD_JOIN, self.id(), 0, 0)?;
        self.tid = None;
        if !self.packet.done.load(Ordering::Acquire) {
            return Err(Error);
        }
        // SAFETY: This consumed handle is the sole reader. Successful kernel
        // join proves the worker has finished; acquire publishes the value.
        unsafe { (*self.packet.value.get()).take().ok_or(Error) }
    }
}
impl<T: Send + 'static> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if let Some(tid) = self.tid.take() {
            let _ = call(abi::SYS_THREAD_DETACH, tid, 0, 0);
        }
    }
}
/// Spawn with default stack size. Captures and result must safely cross threads.
pub fn spawn<F, T>(f: F) -> Result<JoinHandle<T>, Error>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(f)
}

/// Outcome of an atomic compare-and-park request. Always recheck the predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    Woken,
    Mismatch,
    TimedOut,
}
/// Park only while the atomic still equals expected. None waits indefinitely.
pub fn wait(word: &AtomicU32, expected: u32, timeout_ticks: Option<u64>) -> Result<Wait, Error> {
    if timeout_ticks == Some(0) {
        return Ok(Wait::TimedOut);
    }
    match call(
        abi::SYS_THREAD_WAIT,
        word.as_ptr() as u64,
        u64::from(expected),
        timeout_ticks.unwrap_or(0),
    )? {
        0 => Ok(Wait::Woken),
        1 => Ok(Wait::Mismatch),
        2 => Ok(Wait::TimedOut),
        _ => Err(Error),
    }
}
/// Wake at most count threads parked on this process-private atomic word.
pub fn wake(word: &AtomicU32, count: usize) -> Result<usize, Error> {
    call(abi::SYS_THREAD_WAKE, word.as_ptr() as u64, count as u64, 0).map(|n| n as usize)
}
/// Diagnostic count of this process's allocated worker-stack pages.
pub fn stack_pages() -> usize {
    call(abi::SYS_THREAD_PAGES, 0, 0, 0).unwrap_or(0) as usize
}
/// Diagnostic physical-frame ownership count; use with a controlled workload.
pub fn allocated_frames() -> usize {
    call(abi::SYS_FRAME_COUNT, 0, 0, 0).unwrap_or(0) as usize
}
