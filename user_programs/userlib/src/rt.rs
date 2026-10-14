//! Program runtime: entry point and panic handler.
//!
//! Linked automatically with `userlib`. Programs define `user_main`; this
//! module provides the `_start` symbol the kernel jumps to and a panic
//! handler that reports the panic before exiting.

use core::arch::asm;
use core::panic::PanicInfo;

// Entry point implemented by the program.
extern "C" {
    fn user_main();
}

/// Kernel entry point for user programs.
///
/// Calls the program's `user_main`. If it returns, issues the exit syscall
/// directly (inline asm, so LTO cannot reason it away).
///
/// Placed in `.text.start` so the linker script puts it at the load base
/// (`0x400000`), where the kernel jumps to start the program.
#[no_mangle]
#[link_section = ".text.start"]
pub extern "C" fn _start() -> ! {
    unsafe {
        user_main();
        // user_main returned: exit(0) via raw syscall.
        asm!(
            "int 0x80",
            in("rax") 0u64, // SYS_EXIT
            in("rdi") 0u64, // code
            in("rsi") 0u64,
            in("rdx") 0u64,
            options(nostack, noreturn),
        );
    }
}

/// Prints the panic message to the console and exits with code 1.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::println!("panic: {}", info);
    crate::sys::exit(1);
}
