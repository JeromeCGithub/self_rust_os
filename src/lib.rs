//! Self made OS in Rust.
//! This project is a learning project to understand how an OS works.
//! It is not intended to be a production ready OS.
//! I wanted to combine my love for Rust and my curiosity for OS development.
//! Feel free to ask me any question about this project, this is one of my favorite project.
//! Refer to the README.md file for more information about the features supported.

#![no_std]
#![cfg_attr(test, no_main)]
#![feature(abi_x86_interrupt)]
#![feature(naked_functions)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![cfg_attr(test, allow(missing_docs))]
extern crate alloc;

#[cfg(test)]
use bootloader::{entry_point, BootInfo};

use core::panic::PanicInfo;
use x86_64::instructions;

pub mod allocator;
pub mod ata;
#[cfg(any(feature = "grub", test))]
pub mod boot;
pub mod console;
pub mod fat32;
pub mod fs;
pub mod gdt;
pub mod gfx;
pub mod interrupts;
pub mod mbr;
pub mod memory;
pub mod mouse;
pub mod scheduler;
pub mod serial;
pub mod task;
pub mod userspace;

/// Initialization function for the kernel.
///
/// Initializes the graphics framebuffer + text console first (so `println!`
/// works), then the GDT, IDT, and PICs. Must be called once at boot with the
/// physical memory offset from the bootloader, the physical address of the
/// linear framebuffer, and its dimensions in pixels.
pub fn init(phys_mem_offset: x86_64::VirtAddr, fb_phys_addr: u64, fb_width: usize, fb_height: usize) {
    gfx::init(phys_mem_offset, fb_phys_addr, fb_width, fb_height);
    console::init();

    gdt::init();
    interrupts::init_idt();

    // SAFETY:
    // Initialize the Programmable Interrupt Controller (PIC).
    unsafe {
        interrupts::PICS.lock().initialize();
        // Unmask timer (IRQ0), keyboard (IRQ1), and the PIC2 cascade (IRQ2);
        // on the slave, unmask the mouse (IRQ12). The firmware may have these
        // masked, which would break input.
        interrupts::PICS.lock().write_masks(0xF8, 0xEF);
    }
    // Enable interrupts.
    instructions::interrupts::enable();
}

const QEMU_EXIT_PORT: u16 = 0xf4;

#[cfg(test)]
entry_point!(test_kernel_main);

#[cfg(test)]
fn test_kernel_main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = x86_64::VirtAddr::new(boot_info.physical_memory_offset);
    init(
        phys_mem_offset,
        gfx::FRAMEBUFFER_PHYS_ADDR,
        gfx::WIDTH,
        gfx::HEIGHT,
    );

    // Set up paging and the heap so unit tests can use `alloc`.
    // SAFETY: The bootloader guarantees the physical memory offset is valid.
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    // SAFETY: The bootloader guarantees the memory map is valid.
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::new(&boot_info.memory_map) };
    #[expect(clippy::expect_used, reason = "Heap is required for unit tests using alloc")]
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap init failed");

    // Initialize the keyboard scancode queue (needs heap).
    task::keyboard::init_scancode_queue();

    test_main();
    hlt_loop();
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

/// Halt the CPU in an infinite loop.
pub fn hlt_loop() -> ! {
    loop {
        instructions::hlt();
    }
}

/// Panic handler for external (functional) tests.
pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failure);

    hlt_loop();
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
    hlt_loop();
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
