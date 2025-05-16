//! Simple boot test for the kernel.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use self_rust_os::println;

/// Entry point for the test.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    test_main();

    #[expect(
        clippy::empty_loop,
        reason = "Infinite loop after the test main function is done."
    )]
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}

/// Test that we can print a line.
#[test_case]
fn test_println() {
    println!("test_println output");
}
