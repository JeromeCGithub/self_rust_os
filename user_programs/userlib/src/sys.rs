//! Syscall interface for user programs.
//!
//! Wraps the kernel's `int 0x80` ABI:
//!
//! | Register | Purpose        |
//! |----------|----------------|
//! | `rax`    | syscall number |
//! | `rdi`    | argument 1     |
//! | `rsi`    | argument 2     |
//! | `rdx`    | argument 3     |
//!
//! The return value is placed in `rax`. On failure the kernel returns
//! [`SYSCALL_ERROR`].

use core::arch::asm;

/// Syscall numbers. Must match the kernel's `src/userspace/syscall.rs`.
pub const SYS_EXIT: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_READ: u64 = 2;
pub const SYS_TICKS: u64 = 3;
pub const SYS_READ_NB: u64 = 4;
pub const SYS_LS: u64 = 5;
pub const SYS_READ_FILE: u64 = 6;
pub const SYS_WRITE_FILE: u64 = 7;
pub const SYS_SPAWN: u64 = 8;
pub const SYS_KILL: u64 = 9;
pub const SYS_WAITPID: u64 = 10;
pub const SYS_MOUSE: u64 = 11;
pub const SYS_MKDIR: u64 = 12;
pub const SYS_FB_INFO: u64 = 13;

/// Blocks the caller for a duration in PIT ticks.
pub const SYS_SLEEP_TICKS: u64 = 17;
/// Publishes a completed update to the caller's private drawing surface.
pub const SYS_PRESENT_SURFACE: u64 = 18;
/// Queries surface revision and exit status (display owner only).
pub const SYS_SURFACE_STATUS: u64 = 19;
/// Bounded messages between an application and the display owner.
pub const SYS_SEND_MESSAGE: u64 = 20;
/// Nonblocking receive from the caller's inbox.
pub const SYS_RECEIVE_MESSAGE: u64 = 21;
/// Private connected-child standard-I/O operations.
pub const SYS_STDIO: u64 = 22;
/// Open a regular file (Linux-style flags, native syscall number).
pub const SYS_OPEN: u64 = 23;
/// Close a regular-file descriptor.
pub const SYS_CLOSE: u64 = 24;
/// Write bytes through a descriptor.
pub const SYS_WRITE_FD: u64 = 25;
/// Reposition a regular-file offset.
pub const SYS_LSEEK: u64 = 26;
/// Change the process working directory.
pub const SYS_CHDIR: u64 = 27;
/// Copy the canonical current directory, including its NUL terminator.
pub const SYS_GETCWD: u64 = 28;

/// Return value indicating a failed syscall.
pub const SYSCALL_ERROR: u64 = u64::MAX - 1;

/// File descriptor for keyboard input.
pub const STDIN_FD: u64 = 0;

/// Invokes a raw syscall via `int 0x80`.
///
/// Prefer the typed wrappers below. This is an escape hatch for syscalls
/// without a wrapper yet.
///
/// # Safety
///
/// The caller must ensure the syscall number and arguments form a valid
/// request according to the kernel's syscall ABI.
#[inline(always)]
pub unsafe fn syscall(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let result: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => result,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack),
    );
    result
}

/// Terminates the program with the given exit code. Never returns.
pub fn exit(code: u64) -> ! {
    unsafe { syscall(SYS_EXIT, code, 0, 0) };
    // The kernel never returns from SYS_EXIT; this is just to satisfy the
    // type checker in case it ever does.
    unsafe { core::hint::unreachable_unchecked() }
}

/// Writes bytes to the console or the connected host on a child endpoint.
/// Returns the number of bytes written; connected output retries backpressure.
pub fn console_write(buf: &[u8]) -> usize {
    if crate::process::redirected() {
        return crate::process::write(buf);
    }
    unsafe { syscall(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0) as usize }
}

/// Blocking stdin read: keyboard normally, connected input on a hosted child.
/// Returns the number of bytes read.
/// A keyboard wake without copied bytes returns zero; callers should retry.
pub fn read(fd: u64, buf: &mut [u8]) -> usize {
    if fd == STDIN_FD && crate::process::redirected() {
        return crate::process::read(buf, true);
    }
    unsafe { syscall(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) as usize }
}

/// Non-blocking read. Returns bytes read, or 0 when no input is available.
pub fn read_nb(fd: u64, buf: &mut [u8]) -> usize {
    if fd == STDIN_FD && crate::process::redirected() {
        return crate::process::read(buf, false);
    }
    unsafe { syscall(SYS_READ_NB, fd, buf.as_mut_ptr() as u64, buf.len() as u64) as usize }
}

/// Monotonic timer ticks since boot.
pub fn ticks() -> u64 {
    unsafe { syscall(SYS_TICKS, 0, 0, 0) }
}

/// Copies a UTF-8 path into owned NUL-terminated storage (fewer than 4096 bytes).
#[cfg(feature = "alloc")]
pub(crate) fn path_buf(path: &str) -> Option<alloc::vec::Vec<u8>> {
    if path.len() >= 4096 || path.as_bytes().contains(&0) {
        return None;
    }
    let mut bytes = alloc::vec::Vec::new();
    bytes.try_reserve_exact(path.len() + 1).ok()?;
    bytes.extend_from_slice(path.as_bytes());
    bytes.push(0);
    Some(bytes)
}
#[cfg(not(feature = "alloc"))]
pub(crate) fn path_buf(path: &str) -> Option<[u8; 4096]> {
    if path.len() >= 4096 || path.as_bytes().contains(&0) {
        return None;
    }
    let mut bytes = [0; 4096];
    bytes[..path.len()].copy_from_slice(path.as_bytes());
    Some(bytes)
}

/// Lists directory entries into `buf` (12 bytes per entry: 11-byte 8.3 name
/// + 1 byte attribute, `0x10` = directory).
/// `path` selects the directory ("" = cwd, "/" = root).
/// Returns the number of entries, or `None` on error.
pub fn ls(path: &str, buf: &mut [u8], max_entries: usize) -> Option<usize> {
    if max_entries.checked_mul(12)? > buf.len() {
        return None;
    }
    let path_buf = path_buf(path)?;
    let n = unsafe {
        syscall(
            SYS_LS,
            path_buf.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            max_entries as u64,
        )
    };
    if n == SYSCALL_ERROR {
        None
    } else {
        Some(n as usize)
    }
}

/// Reads a whole file into `buf`. Returns bytes read, or `None` on error.
pub fn read_file(path: &str, buf: &mut [u8]) -> Option<usize> {
    let path_buf = path_buf(path)?;
    let n = unsafe {
        syscall(
            SYS_READ_FILE,
            path_buf.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if n == SYSCALL_ERROR {
        None
    } else {
        Some(n as usize)
    }
}

/// Creates or overwrites a file with `data`. Returns bytes written,
/// or `None` on error.
pub fn write_file(path: &str, data: &[u8]) -> Option<usize> {
    let path_buf = path_buf(path)?;
    let n = unsafe {
        syscall(
            SYS_WRITE_FILE,
            path_buf.as_ptr() as u64,
            data.as_ptr() as u64,
            data.len() as u64,
        )
    };
    if n == SYSCALL_ERROR {
        None
    } else {
        Some(n as usize)
    }
}

/// Open a regular file; returns the lowest available descriptor (3..=1026).
pub fn open(path: &str, flags: u64) -> Result<u64, ()> {
    let path = path_buf(path).ok_or(())?;
    // SAFETY: The complete fixed-size path remains readable for this call;
    // flags are scalar and the kernel validates them before disk changes.
    result(unsafe { syscall(SYS_OPEN, path.as_ptr() as u64, flags, 0) })
}
/// Close a descriptor; already closed or foreign descriptors fail.
pub fn close(fd: u64) -> Result<(), ()> {
    // SAFETY: Only a process-local descriptor integer crosses the boundary.
    result(unsafe { syscall(SYS_CLOSE, fd, 0, 0) }).map(|_| ())
}
/// Write at the current descriptor offset. Returns a possibly short count.
/// Descriptors 1/2 use the same output connection as console_write.
pub fn write(fd: u64, bytes: &[u8]) -> Result<usize, ()> {
    if fd == 1 || fd == 2 {
        return Ok(console_write(bytes));
    }
    // SAFETY: bytes is readable for its complete length during this call. No
    // reference survives the syscall; the kernel copies before disk access.
    result(unsafe { syscall(SYS_WRITE_FD, fd, bytes.as_ptr() as u64, bytes.len() as u64) })
        .map(|n| n as usize)
}
/// Seek with Linux SEEK_SET/CUR/END values (0/1/2), and signed byte offset.
pub fn lseek(fd: u64, offset: i64, origin: u64) -> Result<u64, ()> {
    // SAFETY: Descriptor, signed offset bits and origin are scalar arguments.
    result(unsafe { syscall(SYS_LSEEK, fd, offset as u64, origin) })
}
/// Change working directory. Failure preserves the previous directory.
pub fn chdir(path: &str) -> Result<(), ()> {
    let path = path_buf(path).ok_or(())?;
    // SAFETY: The complete fixed-size path is readable and not retained.
    result(unsafe { syscall(SYS_CHDIR, path.as_ptr() as u64, 0, 0) }).map(|_| ())
}
/// Return the canonical directory as UTF-8 within the caller's buffer.
pub fn getcwd(bytes: &mut [u8]) -> Result<&str, ()> {
    // SAFETY: The destination is exclusively borrowed and writable for the
    // advertised capacity; the kernel validates its mappings before copying.
    let n =
        result(unsafe { syscall(SYS_GETCWD, bytes.as_mut_ptr() as u64, bytes.len() as u64, 0) })?
            as usize;
    if n == 0 || n > bytes.len() || bytes[n - 1] != 0 {
        return Err(());
    }
    core::str::from_utf8(&bytes[..n - 1]).map_err(|_| ())
}
fn result(value: u64) -> Result<u64, ()> {
    if value == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(value)
    }
}

/// Creates a directory at `path`, creating missing parents (`mkdir -p`).
/// Returns `Ok` on success, `Err` on error (bad path, already a file, etc.).
pub fn mkdir(path: &str) -> Result<(), ()> {
    let path_buf = path_buf(path).ok_or(())?;
    let n = unsafe { syscall(SYS_MKDIR, path_buf.as_ptr() as u64, 0, 0) };
    if n == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(())
    }
}

/// Base address of the display owner's MMIO or a client's private drawing surface.
pub const FRAMEBUFFER_ADDR: usize = rustos_thread_abi::FRAMEBUFFER as usize;

/// Framebuffer geometry returned by [`fb_info`].
#[derive(Debug, Clone, Copy)]
pub struct FbInfo {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Bytes per scanline.
    pub pitch: usize,
    /// Bits per pixel (32).
    pub bpp: usize,
}

/// Returns the framebuffer geometry (width, height, pitch, bpp).
/// User programs must use this instead of hardcoding dimensions.
pub fn fb_info() -> Result<FbInfo, ()> {
    let mut buf = [0u64; 4];
    let n = unsafe { syscall(SYS_FB_INFO, buf.as_mut_ptr() as u64, 0, 0) };
    if n == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(FbInfo {
            width: buf[0] as usize,
            height: buf[1] as usize,
            pitch: buf[2] as usize,
            bpp: buf[3] as usize,
        })
    }
}

/// Creates a new process running the program at the given path.
///
/// Takes a path like `/SNAKE.BIN`. Returns the new process's PID.
/// The caller keeps running; the new process is scheduled preemptively.
pub fn spawn(path: &str) -> Result<u32, ()> {
    spawn_arguments(path, &[], false)
}
/// Spawn with inherited environment; argv[0] is the supplied executable path.
pub fn spawn_with_args(path: &str, arguments: &[&str]) -> Result<u32, ()> {
    spawn_arguments(path, arguments, false)
}
#[cfg(feature = "alloc")]
pub(crate) fn spawn_arguments(path: &str, arguments: &[&str], connected: bool) -> Result<u32, ()> {
    let environment = crate::env::snapshot();
    let environment: alloc::vec::Vec<&str> =
        environment.iter().map(|string| string.as_str()).collect();
    spawn_environment(path, arguments, &environment, connected)
}
#[cfg(not(feature = "alloc"))]
pub(crate) fn spawn_arguments(path: &str, arguments: &[&str], connected: bool) -> Result<u32, ()> {
    if arguments.len() >= rustos_process_abi::MAX_ARGS {
        return Err(());
    }
    let mut argv = [""; rustos_process_abi::MAX_ARGS];
    argv[0] = path;
    argv[1..arguments.len() + 1].copy_from_slice(arguments);
    let request =
        rustos_process_abi::Arguments::new(&argv[..arguments.len() + 1]).map_err(|_| ())?;
    let path = path_buf(path).ok_or(())?;
    // SAFETY: Complete fixed path and request stay readable for the call.
    result(unsafe {
        syscall(
            rustos_process_abi::SYS_SPAWN_ARGS,
            path.as_ptr() as u64,
            request.as_bytes().as_ptr() as u64,
            u64::from(connected),
        )
    })
    .map(|pid| pid as u32)
}
/// Spawn with an explicit environment (KEY=value strings); argv[0] is path.
#[cfg(feature = "alloc")]
pub fn spawn_with_env(path: &str, arguments: &[&str], environment: &[&str]) -> Result<u32, ()> {
    spawn_environment(path, arguments, environment, false)
}
#[cfg(feature = "alloc")]
pub(crate) fn spawn_environment(
    path: &str,
    arguments: &[&str],
    environment: &[&str],
    connected: bool,
) -> Result<u32, ()> {
    use rustos_process_abi::{SpawnRequest, StringRef, MAX_STARTUP_BYTES, MAX_VECTOR};
    if arguments.len() >= MAX_VECTOR || environment.len() > MAX_VECTOR {
        return Err(());
    }
    let mut budget = path.len() + 1;
    for string in arguments.iter().chain(environment.iter()) {
        budget = budget
            .checked_add(string.len() + 1)
            .filter(|&n| n <= MAX_STARTUP_BYTES)
            .ok_or(())?;
        if string.as_bytes().contains(&0) {
            return Err(());
        }
    }
    let mut argv = alloc::vec::Vec::new();
    argv.try_reserve_exact(arguments.len() + 1)
        .map_err(|_| ())?;
    for string in core::iter::once(path).chain(arguments.iter().copied()) {
        argv.push(StringRef {
            pointer: string.as_ptr() as u64,
            length: string.len() as u64,
        });
    }
    let mut envp = alloc::vec::Vec::new();
    envp.try_reserve_exact(environment.len()).map_err(|_| ())?;
    for string in environment {
        envp.push(StringRef {
            pointer: string.as_ptr() as u64,
            length: string.len() as u64,
        });
    }
    let request = SpawnRequest {
        version: 1,
        flags: u64::from(connected),
        argv: argv.as_ptr() as u64,
        argc: argv.len() as u64,
        envp: envp.as_ptr() as u64,
        envc: envp.len() as u64,
    };
    let path = path_buf(path).ok_or(())?;
    // SAFETY: The fixed path, repr(C) padding-free request, both complete record
    // vectors and every referenced UTF-8 string remain readable through the
    // synchronous syscall. The kernel validates/copies before retaining data.
    result(unsafe {
        syscall(
            rustos_process_abi::SYS_SPAWN_ENV,
            path.as_ptr() as u64,
            core::ptr::addr_of!(request) as u64,
            core::mem::size_of::<SpawnRequest>() as u64,
        )
    })
    .map(|pid| pid as u32)
}

/// Terminates the process with the given PID.
///
/// Returns 0 on success, or an error if the PID is invalid.
pub fn kill(pid: u32) -> Result<(), ()> {
    let n = unsafe { syscall(SYS_KILL, pid as u64, 0, 0) };
    if n == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(())
    }
}

/// Waits for the process with the given PID to exit.
///
/// Blocks until the target exits, then reaps it and returns its exit code.
pub fn waitpid(pid: u32) -> Result<u64, ()> {
    let n = unsafe { syscall(SYS_WAITPID, pid as u64, 0, 0) };
    if n == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(n)
    }
}

/// Returns the current mouse state as `(x, y, buttons)`.
///
/// Coordinates span the dimensions returned by fb_info; buttons: bit 0 = left,
/// bit 1 = right, bit 2 = middle.
pub fn mouse() -> (i32, i32, u8) {
    let packed = unsafe { syscall(SYS_MOUSE, 0, 0, 0) };
    let x = (packed & 0xFFFF) as i32;
    let y = ((packed >> 16) & 0xFFFF) as i32;
    let buttons = ((packed >> 32) & 0xFF) as u8;
    (x, y, buttons)
}

/// Copies a nonempty prefix of an application's private drawing surface.
/// The buffer must fit within pitch * height bytes; full snapshots remain supported.
/// Only PID 0 may call this. Returns whether the application has exited.
pub fn snapshot(pid: u32, pixels: &mut [u32]) -> Result<bool, ()> {
    let result = unsafe {
        syscall(
            14,
            u64::from(pid),
            pixels.as_mut_ptr() as u64,
            core::mem::size_of_val(pixels) as u64,
        )
    };
    if result == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(result == 1)
    }
}

/// Gives a desktop application keyboard focus. Only PID 0 may call this.
pub fn focus(pid: u32) -> Result<(), ()> {
    if unsafe { syscall(15, u64::from(pid), 0, 0) } == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(())
    }
}

/// Uses client-side rendering for this application's surface; console writes
/// continue to reach the serial log without drawing over graphical content.
pub fn graphics_mode() {
    unsafe {
        syscall(16, 0, 0, 0);
    }
}

/// Deschedules the calling thread for a duration in PIT ticks (~55 ms each).
/// Zero returns immediately. Durations above i64::MAX or overflowing the
/// absolute tick deadline return Err. No CPU time is consumed while waiting.
pub fn sleep_ticks(duration: u64) -> Result<(), ()> {
    // SAFETY: Syscall 17 takes only an integer duration and no pointers.
    if unsafe { syscall(SYS_SLEEP_TICKS, duration, 0, 0) } == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(())
    }
}

/// Publishes a completed update to this application's private drawing surface.
/// Graphical apps call this after rendering, so the compositor can reuse cached
/// content while it is unchanged. Console output publishes automatically.
pub fn present_surface() -> Result<(), ()> {
    // SAFETY: Syscall 18 has no arguments and publishes only the caller's surface.
    if unsafe { syscall(SYS_PRESENT_SURFACE, 0, 0, 0) } == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(())
    }
}

/// Queries a private surface's (revision, exited) metadata. PID 0 only.
pub fn surface_status(pid: u32) -> Result<(u64, bool), ()> {
    // SAFETY: Syscall 19 takes a PID and returns integer metadata, no pointers.
    let result = unsafe { syscall(SYS_SURFACE_STATUS, u64::from(pid), 0, 0) };
    if result == SYSCALL_ERROR {
        Err(())
    } else {
        Ok((result >> 1, result & 1 != 0))
    }
}

/// Failure to send/receive a scalar service message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageError {
    /// The target's bounded queue is full. Retry after yielding.
    Full,
    /// Missing/dead target, forbidden client-to-client delivery, or no caller.
    Unavailable,
}

/// Sends an opaque scalar to PID 0, or from PID 0 to a live client.
pub fn send_message(target: u32, payload: u32) -> Result<(), MessageError> {
    // SAFETY: Only integers cross the boundary; no user memory is borrowed.
    match unsafe { syscall(SYS_SEND_MESSAGE, u64::from(target), u64::from(payload), 0) } {
        0 => Ok(()),
        1 => Err(MessageError::Full),
        _ => Err(MessageError::Unavailable),
    }
}

/// Returns a kernel-authenticated `(sender, payload)`, or None for an empty inbox.
pub fn receive_message() -> Result<Option<(u32, u32)>, MessageError> {
    // SAFETY: No arguments or pointers; only the caller's inbox is accessed.
    let value = unsafe { syscall(SYS_RECEIVE_MESSAGE, 0, 0, 0) };
    if value == SYSCALL_ERROR {
        return Err(MessageError::Unavailable);
    }
    if value == 0 {
        return Ok(None);
    }
    Ok(Some((((value >> 32) - 1) as u32, value as u32)))
}

/// Park only this thread until its process mailbox has data (true) or the PIT
/// deadline expires (false). Zero waits indefinitely. This does not consume a
/// message: other same-process readers can race, so recheck receive_message.
pub fn wait_message(timeout_ticks: u64) -> Result<bool, MessageError> {
    // SAFETY: Only a scalar timeout crosses the boundary; no user references
    // are retained while the kernel blocks this native execution context.
    match unsafe { syscall(rustos_thread_abi::SYS_MESSAGE_WAIT, timeout_ticks, 0, 0) } {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(MessageError::Unavailable),
    }
}

/// Process-local readiness conditions. Waiting never consumes input or messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Events(u64);
impl Events {
    /// An owned child has retained completion; consume through the child API.
    pub const CHILD: Self = Self(rustos_thread_abi::EVENT_CHILD);
    /// Native channel message, closure or requested writable notification.
    pub const PORT: Self = Self(rustos_thread_abi::EVENT_PORT);
    pub const MAILBOX: Self = Self(rustos_thread_abi::EVENT_MAILBOX);
    pub const STDIO_READ: Self = Self(rustos_thread_abi::EVENT_STDIO_READ);
    pub const STDIO_WRITE: Self = Self(rustos_thread_abi::EVENT_STDIO_WRITE);
    pub const KEYBOARD: Self = Self(rustos_thread_abi::EVENT_KEYBOARD);
    /// Display-owner pointer changes since its last `mouse()` sample.
    pub const POINTER: Self = Self(rustos_thread_abi::EVENT_POINTER);
    /// Whether all requested conditions were included in this readiness hint.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
impl core::ops::BitOr for Events {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Park this native thread until any selected condition is ready, or timeout.
/// Zero ticks means no deadline. An empty returned mask means timeout; I/O EOF
/// also counts as readiness. Recheck the operation because readers may race.
pub fn wait_events(events: Events, timeout_ticks: u64) -> Result<Events, MessageError> {
    // SAFETY: Only scalar flags/deadline cross this blocking syscall. The kernel
    // retains no user buffers; process ownership determines accessible queues.
    let value = unsafe {
        syscall(
            rustos_thread_abi::SYS_EVENT_WAIT,
            events.0,
            timeout_ticks,
            0,
        )
    };
    if value == SYSCALL_ERROR {
        Err(MessageError::Unavailable)
    } else {
        Ok(Events(value))
    }
}

/// Wait for this process's standard input, including redirected child input.
pub fn wait_stdin(timeout_ticks: u64) -> Result<Events, MessageError> {
    wait_events(
        if crate::process::redirected() {
            Events::STDIO_READ
        } else {
            Events::KEYBOARD
        },
        timeout_ticks,
    )
}

/// Query (zero) or set the process break. Failure returns its old value.
/// # Safety
/// The caller must exclusively own the heap region and never shrink/unmap live
/// allocations or race the SDK allocator. Ordinary apps should use alloc APIs.
pub unsafe fn brk(address: usize) -> usize {
    // SAFETY: Scalar request only; the kernel validates its arena and mapping
    // ownership. The caller guarantees allocation lifetimes described above.
    unsafe { syscall(31, address as u64, 0, 0) as usize }
}

/// Return this native thread's FS base (static compiler TLS thread pointer).
pub fn fs_base() -> Result<u64, ()> {
    let mut base = 0u64;
    // SAFETY: GET_FS writes one initialized, writable, exclusively borrowed u64.
    result(unsafe { syscall(32, 0x1003, core::ptr::addr_of_mut!(base) as u64, 0) }).map(|_| base)
}
/// Replace this native thread's FS base with a private mapped user pointer.
/// # Safety
/// Caller must own a valid compatible TCB/static-TLS layout and preserve every
/// existing compiler TLS object's lifetime, alignment, initialization and alias
/// rules. Do not redirect compiler TLS while live references to it exist.
pub unsafe fn set_fs_base(base: u64) -> Result<(), ()> {
    // SAFETY: Scalar request; kernel checks canonical mapped user memory. Caller
    // guarantees the stronger compiler-TLS layout/lifetime contract above.
    result(unsafe { syscall(32, 0x1002, base, 0) }).map(|_| ())
}
