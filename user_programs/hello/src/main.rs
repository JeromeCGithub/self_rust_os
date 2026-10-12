//! Minimal user-space program for `self_rust_os`.
//!
//! This program runs in Ring 3 and communicates with the kernel through
//! the `int 0x80` syscall interface. It demonstrates:
//!
//! - `sys_write`: printing to the VGA text display,
//! - `sys_open` / `sys_fwrite` / `sys_read` / `sys_close`: using the
//!   in-memory filesystem (`ramfs`) to create a file, write to it, and
//!   read it back,
//! - `sys_exit`: terminating the process.
//!
//! ## Syscall ABI
//!
//! | Register | Purpose        |
//! |----------|----------------|
//! | `rax`    | syscall number |
//! | `rdi`    | argument 1     |
//! | `rsi`    | argument 2     |
//! | `rdx`    | argument 3     |
//!
//! The return value is placed in `rax`.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

/// Syscall number for `sys_exit`.
const SYS_EXIT: u64 = 0;

/// Syscall number for `sys_write`.
const SYS_WRITE: u64 = 1;

/// Syscall number for `sys_open`.
const SYS_OPEN: u64 = 2;

/// Syscall number for `sys_close`.
const SYS_CLOSE: u64 = 3;

/// Syscall number for `sys_read`.
const SYS_READ: u64 = 4;

/// Syscall number for `sys_fwrite`.
const SYS_FWRITE: u64 = 5;

/// Error value returned by the kernel when a syscall fails.
const SYSCALL_ERROR: u64 = u64::MAX - 1;

/// Open flag: open the file for reading.
const O_READ: u64 = 0x01;

/// Open flag: open the file for writing.
const O_WRITE: u64 = 0x02;

/// Open flag: create the file if it does not exist.
const O_CREATE: u64 = 0x04;

/// Open flag: truncate the file to zero length on open.
const O_TRUNC: u64 = 0x08;

/// Invokes a syscall via `int 0x80`.
///
/// # Safety
///
/// The caller must ensure that the syscall number and arguments form a valid
/// request according to the kernel's syscall ABI.
#[inline(always)]
unsafe fn syscall(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let result: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => result,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        // Mark registers that the kernel syscall handler may clobber.
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack),
    );
    result
}

/// Writes the given byte slice to the VGA text display via `sys_write`.
fn write(buf: &[u8]) {
    // SAFETY:
    //
    // The buffer pointer and length are valid and reside in user-accessible
    // memory. `SYS_WRITE` is a valid syscall number.
    unsafe {
        syscall(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
    }
}

/// Terminates the current process with the given exit code via `sys_exit`.
fn exit(code: u64) -> ! {
    // SAFETY:
    //
    // `SYS_EXIT` is a valid syscall number. The kernel will halt the process
    // and never return to user mode.
    unsafe {
        syscall(SYS_EXIT, code, 0, 0);
    }

    // The kernel should never return from sys_exit, but just in case, spin
    // forever so the function signature `-> !` is satisfied.
    loop {
        // SAFETY:
        //
        // Halting the CPU is safe here as a last-resort spin loop. The kernel
        // should never return from sys_exit, so this is unreachable in practice.
        unsafe {
            asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Prints an error message and exits with code 1.
fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit(1);
}

/// Entry point for the user-mode program.
///
/// The linker script places `_start` at the beginning of the `.text` section
/// at virtual address `0x400000`.
#[no_mangle]
#[link_section = ".text.start"]
pub extern "C" fn _start() -> ! {
    write(b"Hello from user space!\n");
    write(b"This message was printed via sys_write (int 0x80).\n");

    // --- Filesystem demo: create /hello.txt, write to it, read it back. ---
    let path = b"/hello.txt";

    // SAFETY: `sys_open` is a valid syscall; the path lives in user memory.
    let fd = unsafe {
        syscall(
            SYS_OPEN,
            path.as_ptr() as u64,
            path.len() as u64,
            O_CREATE | O_WRITE | O_TRUNC,
        )
    };
    if fd == SYSCALL_ERROR {
        fail(b"ERROR: sys_open could not create /hello.txt\n");
    }
    write(b"Created /hello.txt in the in-memory filesystem.\n");

    let message = b"Hello from a file in ramfs!\n";
    // SAFETY: `sys_fwrite` is a valid syscall; the buffer lives in user memory.
    let written = unsafe { syscall(SYS_FWRITE, fd, message.as_ptr() as u64, message.len() as u64) };
    if written != message.len() as u64 {
        fail(b"ERROR: sys_fwrite could not write /hello.txt\n");
    }
    write(b"Wrote a greeting to /hello.txt.\n");

    // SAFETY: `sys_close` is a valid syscall; `fd` is open.
    unsafe {
        syscall(SYS_CLOSE, fd, 0, 0);
    }

    // Reopen the file for reading.
    // SAFETY: `sys_open` is a valid syscall; the path lives in user memory.
    let fd = unsafe { syscall(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, O_READ) };
    if fd == SYSCALL_ERROR {
        fail(b"ERROR: sys_open could not open /hello.txt for reading\n");
    }

    let mut buf = [0u8; 128];
    // SAFETY: `sys_read` is a valid syscall; `buf` lives on the user stack.
    let n = unsafe { syscall(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) };
    if n == SYSCALL_ERROR {
        fail(b"ERROR: sys_read could not read /hello.txt\n");
    }
    // The kernel never returns more bytes than the buffer holds, but clamp
    // defensively before slicing.
    let n = core::cmp::min(n as usize, buf.len());

    write(b"Read back from /hello.txt: ");
    write(&buf[..n]);

    // SAFETY: `sys_close` is a valid syscall; `fd` is open.
    unsafe {
        syscall(SYS_CLOSE, fd, 0, 0);
    }

    write(b"Filesystem demo complete. Exiting with code 0.\n");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Attempt to report the panic to the kernel before exiting.
    write(b"PANIC in user program!\n");
    exit(1);
}
