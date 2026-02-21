//! Minimal user-space program for `self_rust_os`.
//!
//! This program runs in Ring 3 and communicates with the kernel through
//! the `int 0x80` syscall interface. It prints a greeting message to the
//! VGA text buffer via `sys_write` and then terminates via `sys_exit`.
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

/// Invokes a syscall via `int 0x80`.
///
/// # Safety
///
/// The caller must ensure that the syscall number and arguments form a valid
/// request according to the kernel's syscall ABI.
#[inline(always)]
unsafe fn syscall(num: u64, arg1: u64, arg2: u64) -> u64 {
    let result: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => result,
        in("rdi") arg1,
        in("rsi") arg2,
        // Mark registers that the kernel syscall handler may clobber.
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack),
    );
    result
}

/// Writes the given byte slice to the VGA text display via `sys_write`.
fn write(buf: &[u8]) -> u64 {
    // SAFETY:
    //
    // The buffer pointer and length are valid and reside in user-accessible
    // memory. `SYS_WRITE` is a valid syscall number.
    unsafe { syscall(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64) }
}

/// Terminates the current process with the given exit code via `sys_exit`.
fn exit(code: u64) -> ! {
    // SAFETY:
    //
    // `SYS_EXIT` is a valid syscall number. The kernel will halt the process
    // and never return to user mode.
    unsafe {
        syscall(SYS_EXIT, code, 0);
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

/// Entry point for the user-mode program.
///
/// The linker script places `_start` at the beginning of the `.text` section
/// at virtual address `0x400000`.
#[no_mangle]
#[link_section = ".text.start"]
pub extern "C" fn _start() -> ! {
    write(b"Hello from user space!\n");
    write(b"This message was printed via sys_write (int 0x80).\n");
    write(b"Goodbye! Exiting with code 0.\n");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Attempt to report the panic to the kernel before exiting.
    write(b"PANIC in user program!\n");
    exit(1);
}
