//! Main for little self made rust OS.

#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![no_std]
#![no_main]

use core::panic::PanicInfo;
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
pub enum QuemuExitCode {
    /// Define a successfull exit status
    Success = 0x10,
    /// Define a failure exit status
    Failure = 0x11,
}

/// Exit qemu with a specific exit code.
/// Connect to an IO Port to exit qemu.
/// Configuration for the exit port is in the config.toml file.
pub fn exit_qemu(exit_code: QuemuExitCode) {
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
pub fn test_runner(tests: &[&dyn Fn()]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test();
    }

    exit_qemu(QuemuExitCode::Success);
}

/// Custom test try.
/// # Panics
/// May panic if the test fail
#[expect(
    clippy::assertions_on_constants,
    reason = "This is the main loop of the OS."
)]
#[test_case]
fn trivial_assertion() {
    print!("trivial assertion... ");
    assert!(true, "Make this test pass.");
    println!("[ok]");
}
