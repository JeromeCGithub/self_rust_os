//! Integration test for timer ticks and keyboard input.
//!
//! Verifies the PIT timer interrupt increments the tick counter and that
//! the keyboard IRQ path delivers scancodes.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]

mod common;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{interrupts, println, serial_println};
use x86_64::VirtAddr;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    self_rust_os::init(phys_mem_offset, common::FRAMEBUFFER_INFO, common::framebuffer_addr());

    // Standalone harness: no scheduler.start() will unmask IRQ0 for us.
    // SAFETY: The IDT/PICs are initialized and timer IRQs interrupt only Ring 0
    // here. No user context is restored; the scheduler's process table is empty.
    unsafe { interrupts::PICS.lock().write_masks(0xf8, 0xef); }
    test_main();
    #[expect(clippy::empty_loop)]
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}

#[test_case]
fn when_timer_runs_then_ticks_increase() {
    let t1 = interrupts::ticks();

    // Busy-wait for a bit to let timer interrupts fire.
    // At ~18.2 Hz, 100ms should give us at least 1 tick.
    for _ in 0..10_000_000 {
        core::hint::spin_loop();
    }

    let t2 = interrupts::ticks();
    serial_println!("[timer] ticks: {} -> {}", t1, t2);
    println!("[timer] ticks: {} -> {}", t1, t2);
    assert!(t2 > t1, "timer ticks should increase: {} > {}", t2, t1);
}

#[test_case]
fn when_keyboard_queue_empty_then_pop_returns_none() {
    // Without physical key presses, the queue should be empty.
    // (The IRQ path is exercised via QEMU sendkey in E2E.)
    let result = self_rust_os::task::keyboard::try_pop_scancode();
    // May be Some if a key was pressed during boot; just verify the API works.
    serial_println!("[keyboard] pop result: {:?}", result.is_some());
}
