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

/// Writes bytes to the console. Returns the number of bytes written.
pub fn console_write(buf: &[u8]) -> usize {
    unsafe { syscall(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0) as usize }
}

/// Blocking read from a file descriptor (`STDIN_FD` = keyboard).
/// Returns the number of bytes read.
pub fn read(fd: u64, buf: &mut [u8]) -> usize {
    unsafe {
        syscall(
            SYS_READ,
            fd,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        ) as usize
    }
}

/// Non-blocking read. Returns bytes read, or 0 when no input is available.
pub fn read_nb(fd: u64, buf: &mut [u8]) -> usize {
    unsafe {
        syscall(
            SYS_READ_NB,
            fd,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        ) as usize
    }
}

/// Monotonic timer ticks since boot.
pub fn ticks() -> u64 {
    unsafe { syscall(SYS_TICKS, 0, 0, 0) }
}

/// Copies a path into a null-terminated 256-byte buffer for the file syscalls.
fn path_buf(path: &str) -> Option<[u8; 256]> {
    let bytes = path.as_bytes();
    if bytes.len() >= 255 {
        return None;
    }
    let mut buf = [0u8; 256];
    buf[..bytes.len()].copy_from_slice(bytes);
    Some(buf)
}

/// Lists directory entries into `buf` (12 bytes per entry: 11-byte 8.3 name
/// + 1 byte attribute, `0x10` = directory).
/// `path` selects the directory ("" or "/" = root).
/// Returns the number of entries, or `None` on error.
pub fn ls(path: &str, buf: &mut [u8], max_entries: usize) -> Option<usize> {
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
pub const FRAMEBUFFER_ADDR: usize = 0x210_0000;

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
    let path_buf = path_buf(path).ok_or(())?;
    let n = unsafe {
        syscall(
            SYS_SPAWN,
            path_buf.as_ptr() as u64,
            0,
            0,
        )
    };
    if n == SYSCALL_ERROR {
        Err(())
    } else {
        Ok(n as u32)
    }
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
    let result = unsafe { syscall(14, u64::from(pid), pixels.as_mut_ptr() as u64, core::mem::size_of_val(pixels) as u64) };
    if result == SYSCALL_ERROR { Err(()) } else { Ok(result == 1) }
}

/// Gives a desktop application keyboard focus. Only PID 0 may call this.
pub fn focus(pid: u32) -> Result<(), ()> {
    if unsafe { syscall(15, u64::from(pid), 0, 0) } == SYSCALL_ERROR { Err(()) } else { Ok(()) }
}

/// Uses client-side rendering for this application's surface; console writes
/// continue to reach the serial log without drawing over graphical content.
pub fn graphics_mode() { unsafe { syscall(16, 0, 0, 0); } }

/// Deschedules this process for a duration in PIT ticks (~55 ms each).
/// Zero returns immediately. Durations above i64::MAX or overflowing the
/// absolute tick deadline return Err. No CPU time is consumed while waiting.
pub fn sleep_ticks(duration: u64) -> Result<(), ()> {
    // SAFETY: Syscall 17 takes only an integer duration and no pointers.
    if unsafe { syscall(SYS_SLEEP_TICKS, duration, 0, 0) } == SYSCALL_ERROR { Err(()) } else { Ok(()) }
}

/// Publishes a completed update to this application's private drawing surface.
/// Graphical apps call this after rendering, so the compositor can reuse cached
/// content while it is unchanged. Console output publishes automatically.
pub fn present_surface() -> Result<(), ()> {
    // SAFETY: Syscall 18 has no arguments and publishes only the caller's surface.
    if unsafe { syscall(SYS_PRESENT_SURFACE, 0, 0, 0) } == SYSCALL_ERROR { Err(()) } else { Ok(()) }
}

/// Queries a private surface's (revision, exited) metadata. PID 0 only.
pub fn surface_status(pid: u32) -> Result<(u64, bool), ()> {
    // SAFETY: Syscall 19 takes a PID and returns integer metadata, no pointers.
    let result = unsafe { syscall(SYS_SURFACE_STATUS, u64::from(pid), 0, 0) };
    if result == SYSCALL_ERROR { Err(()) } else { Ok((result >> 1, result & 1 != 0)) }
}

/// Failure to send/receive a scalar service message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageError {
    /// The target's sixteen-message inbox is full. Retry after yielding.
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
    if value == SYSCALL_ERROR { return Err(MessageError::Unavailable); }
    if value == 0 { return Ok(None); }
    Ok(Some((((value >> 32) - 1) as u32, value as u32)))
}
