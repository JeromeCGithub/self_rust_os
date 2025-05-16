//! Self made OS in Rust.
//! This project is a learning project to understand how an OS works.
//! It is not intended to be a production ready OS.
//! I wanted to combine my love for Rust and my curiosity for OS development.
//! Feel free to ask me any question about this project, this is one of my favorite project.
//! Refer to the README.md file for more information about the features supported.

#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

pub mod serial;
pub mod vga_buffer;

const QEMU_EXIT_PORT: u16 = 0xf4;

/// Test entry point for cargo test.
#[cfg(test)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    test_main();

    #[expect(
        clippy::empty_loop,
        reason = "Infinite loop after the test main function is done."
    )]
    loop {}
}

/// Define possible exit code for qemu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    /// Define a successfull exit status
    Success = 0x10,
    /// Define a failure exit status
    Failure = 0x11,
}

/// Exit qemu with a specific exit code.
/// Connect to an IO Port to exit qemu.
/// Configuration for the exit port is in the config.toml file.
pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;

    // SAFETY:
    // Create a `Port` to write exit_code for qemu.
    unsafe {
        let mut port = Port::new(QEMU_EXIT_PORT);
        port.write(exit_code as u32);
    }
}

/// Panic handler for external (functional) tests.
pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failure);

    #[expect(
        clippy::empty_loop,
        reason = "Endless loop if a test panics. It should not be possible as we exit_qemu before."
    )]
    loop {}
}

/// Custom test runner for `no_std` testing.
pub fn test_runner(tests: &[&dyn Testable]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }

    exit_qemu(QemuExitCode::Success);
}

/// Panic handler for unit tests.
#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failure);
    loop {}
}

/// Trait to generalize tests cases.
/// Encapsulate the prints
pub trait Testable {
    /// Function that encapsulate the test run function.
    fn run(&self);
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn run(&self) {
        use core::any::type_name;

        serial_print!("{}...\t", type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}
