//! Simple boot test for the kernel.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

mod common;

use bootloader::BootInfo;
use core::panic::PanicInfo;
use x86_64::VirtAddr;

use self_rust_os::println;

// Keep the generated public boot ABI wrapper internal to this test.
mod boot_entry {
    bootloader::entry_point!(super::main);
}

/// Entry point for the test.
fn main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    self_rust_os::init(phys_mem_offset, common::FRAMEBUFFER_INFO, common::framebuffer_addr());

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
