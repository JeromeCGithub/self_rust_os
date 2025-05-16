//! Main for little self made rust OS.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use self_rust_os::println;

/// This function is the entry point, since the linker looks for a function
#[no_mangle]
pub extern "C" fn _start() -> ! {
    println!("toto");

    #[cfg(test)]
    test_main();

    #[expect(clippy::empty_loop, reason = "This is the main loop of the OS.")]
    loop {}
}

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
const fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}

/// Custom test try.
/// # Panics
/// May panic if the test fail
#[expect(clippy::assertions_on_constants)]
#[test_case]
fn trivial_assertion() {
    assert!(true, "Make this test pass.");
}
