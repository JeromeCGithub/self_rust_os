//! Graphics demo: draws a colorful showcase screen and halts.
//!
//! Used for PR screenshots. Not a test — it draws and loops forever.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{gfx, println, serial_println};
use x86_64::VirtAddr;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    self_rust_os::init(phys_mem_offset);

    println!("self_rust_os graphics demo");
    println!("Mode 13h: 320x200, 256 colors");
    println!();
    println!("Framebuffer console: OK");
    println!("Primitives: OK");

    // Draw graphics below the text (text uses rows 0-4, i.e. y=0-40).
    gfx::with_framebuffer(|fb| {
        // Color palette strip.
        let colors = [
            gfx::COLOR_BLACK,
            gfx::COLOR_BLUE,
            gfx::COLOR_GREEN,
            gfx::COLOR_CYAN,
            gfx::COLOR_RED,
            gfx::COLOR_MAGENTA,
            gfx::COLOR_BROWN,
            gfx::COLOR_LIGHT_GRAY,
            gfx::COLOR_DARK_GRAY,
            gfx::COLOR_BRIGHT_BLUE,
            gfx::COLOR_BRIGHT_GREEN,
            gfx::COLOR_BRIGHT_CYAN,
            gfx::COLOR_BRIGHT_RED,
            gfx::COLOR_BRIGHT_MAGENTA,
            gfx::COLOR_YELLOW,
            gfx::COLOR_WHITE,
        ];
        for (i, &c) in colors.iter().enumerate() {
            fb.fill_rect(i * 20, 60, 20, 20, c);
        }

        // Some rectangles.
        fb.fill_rect(20, 90, 80, 60, gfx::COLOR_RED);
        fb.fill_rect(120, 90, 80, 60, gfx::COLOR_GREEN);
        fb.fill_rect(220, 90, 80, 60, gfx::COLOR_YELLOW);
    });

    serial_println!("[demo] graphics demo drawn, halting");

    // Halt so the screenshot tool can capture the screen.
    loop {
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("[demo] PANIC: {}", info);
    loop {
        x86_64::instructions::hlt();
    }
}
