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

/// Lists directory entries into `buf` (12 bytes per entry, 8.3 format).
/// Returns the number of entries, or `None` on error.
pub fn ls(buf: &mut [u8], max_entries: usize) -> Option<usize> {
    let n = unsafe {
        syscall(
            SYS_LS,
            buf.as_mut_ptr() as u64,
            max_entries as u64,
            0,
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
