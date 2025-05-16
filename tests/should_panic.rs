//! Test that should panic.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use self_rust_os::{exit_qemu, serial_println, QemuExitCode};

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

/// Test runner for this test file.
pub fn test_runner(tests: &[&dyn Fn()]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test();
        serial_println!("[test did not panic]");
        exit_qemu(QemuExitCode::Failure);
    }
    exit_qemu(QemuExitCode::Success);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    serial_println!("[ok]");
    exit_qemu(QemuExitCode::Success);
    loop {}
}

/// Test that should panic.
/// # Panics
/// Fail if the test does not panic.
#[test_case]
fn should_fail() {
    serial_println!("should_panic::should_fail...\t");
    assert_eq!(0, 1, "This test should panic.");
}
