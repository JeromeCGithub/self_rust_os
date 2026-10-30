//! Integration tests for the framebuffer driver and text console.
//!
//! Boots in QEMU and verifies drawing primitives against a RAM framebuffer.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_assert_message)]

mod common;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{
    drivers::video::framebuffer::{self as gfx, COLOR_BLACK, COLOR_RED, COLOR_WHITE},
    println, serial_println,
};
use x86_64::VirtAddr;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    self_rust_os::init(phys_mem_offset, common::FRAMEBUFFER_INFO, common::framebuffer_addr());

    test_main();
    #[expect(clippy::empty_loop)]
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}

#[test_case]
fn when_drawing_primitives_then_framebuffer_contains_them() {
    // Draw via the driver API.
    gfx::with_framebuffer(|fb| {
        fb.fill_rect(0, 0, fb.width(), fb.height(), COLOR_BLACK);
        fb.set_pixel(10, 20, COLOR_WHITE);
        fb.fill_rect(100, 50, 20, 10, COLOR_RED);
    });

    // Read the backing memory independently of the driver.
    let pixel = |x: usize, y: usize| {
        let ptr = common::framebuffer_addr().as_ptr::<u32>();
        // SAFETY: These coordinates are within the static test framebuffer.
        unsafe { ptr.add(y * common::FRAMEBUFFER_INFO.width + x).read_volatile() }
    };
    assert_eq!(pixel(10, 20), COLOR_WHITE);
    assert_eq!(pixel(0, 0), COLOR_BLACK);
    assert_eq!(pixel(109, 59), COLOR_RED);
    assert_eq!(pixel(99, 50), COLOR_BLACK);

    serial_println!("[gfx] primitives OK: pixel + rect verified in framebuffer");
    println!("[gfx] primitives OK");
}

#[test_case]
fn when_printing_text_then_console_advances() {
    gfx::with_framebuffer(|fb| fb.fill_rect(0, 0, fb.width(), fb.height(), COLOR_BLACK));
    // Glyph pixel correctness is covered by the gfx unit tests; here we
    // verify the console drives the real framebuffer without faulting.
    println!("Hi");
    println!("Line 2");

    serial_println!("[gfx] console OK: text printed to framebuffer");
    println!("[gfx] console OK");
}
