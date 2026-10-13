//! Program runtime: entry point and panic handler.
//!
//! Linked automatically with `userlib`. Programs define `user_main`; this
//! module provides the `_start` symbol the kernel jumps to and a panic
//! handler that reports the panic before exiting.

use core::panic::PanicInfo;

/// Kernel entry point for user programs.
///
/// Calls the program's `user_main`, then exits cleanly if it returns.
///
/// Placed in `.text.start` so the linker script puts it at the load base
/// (`0x400000`), where the kernel jumps to start the program.
#[no_mangle]
#[link_section = ".text.start"]
pub extern "C" fn _start() -> ! {
    unsafe { super::user_main() };
    crate::sys::exit(0);
}

/// Prints the panic message to the console and exits with code 1.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::println!("panic: {}", info);
    crate::sys::exit(1);
}
