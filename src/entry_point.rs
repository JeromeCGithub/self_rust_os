//! Main for little self made rust OS.

#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![no_std]
#![no_main]

use core::panic::PanicInfo;
mod serial;
mod vga_buffer;

////////////////////////
//     Entry point    //
////////////////////////

/// This function is the entry point, since the linker looks for a function
#[no_mangle]
pub extern "C" fn _start() -> ! {
    println!("toto");

    #[cfg(test)]
    test_main();

    #[expect(clippy::empty_loop, reason = "This is the main loop of the OS.")]
    loop {}
}

////////////////////////
//    Panic handler   //
////////////////////////

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
const fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

////////////////////////
// Qemu exit handler  //
////////////////////////

const QEMU_EXIT_PORT: u16 = 0xf4;

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

////////////////////////
//  Test entry point  //
////////////////////////

/// Custom test runner for `no_std` testing.
#[cfg(test)]
pub fn test_runner(tests: &[&dyn Testable]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }

    exit_qemu(QemuExitCode::Success);
}

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

/// Custom test try.
/// # Panics
/// May panic if the test fail
#[expect(clippy::assertions_on_constants)]
#[test_case]
fn trivial_assertion() {
    assert!(true, "Make this test pass.");
}
