//! Main for little self made rust OS.
//!
//! This is the kernel entry point. It initializes all subsystems (GDT, IDT, PICs,
//! paging, heap) and then loads and executes the embedded user-mode binary.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]

extern crate alloc;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{
    allocator, fs,
    memory::{self, BootInfoFrameAllocator},
    println, serial_println,
    task::keyboard,
    userspace,
};
use x86_64::VirtAddr;

/// The embedded flat binary of the user-mode hello program.
///
/// This binary is built from `user_programs/terminal/` and converted to a flat
/// binary with `llvm-objcopy -O binary`. See `user_programs/terminal/build.sh`
/// for build instructions.
static USER_FORGE_BIN: &[u8] = include_bytes!("../user_programs/forge/forge.bin");

/// Auto-generated metadata about the terminal binary layout (section boundaries).
///
/// Provides `READONLY_SIZE`: the page-aligned byte count of the read-only
/// region (`.text` + `.rodata`) so the kernel can enforce W^X permissions.
mod forge_meta {
    include!("../user_programs/forge/forge_meta.rs");
}

entry_point!(kernel_main);

/// This function is the entry point, since the linker looks for a function
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // Bootloader guarantees that the physical memory is available at the passed offset.
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);

    // Initialize graphics + console first so println! works, then GDT/IDT/PICs.
    self_rust_os::init(phys_mem_offset);

    serial_println!("[kernel] RustOS booting...");
    println!("RustOS booting...");
    println!("Initializing...");
    serial_println!("[kernel] Initialization complete!");
    println!("Initialization complete !");

    // SAFETY:
    // Physical memory offset is valid as guaranteed by the bootloader.
    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    // SAFETY:
    // The memory map is valid as guaranteed by the bootloader.
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::new(&boot_info.memory_map) };

    #[expect(clippy::expect_used)]
    allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("Heap initialization failed. Reboot required.");

    // Initialize the keyboard scancode queue (needs heap).
    self_rust_os::task::keyboard::init_scancode_queue();

    // Initialize the PS/2 mouse (IRQ12). Non-fatal if absent.
    self_rust_os::mouse::init();

    // Mount the root filesystem (needs heap for FAT32).
    // If the disk is not formatted, this will panic; the disk fixture
    // must be created with the FAT32 format.
    #[cfg(not(test))]
    {
        serial_println!("[kernel] mounting root filesystem...");
        self_rust_os::fs::init();
        serial_println!("[kernel] root filesystem mounted");
    }
    // In test mode, run the test harness and exit before entering user space
    // or the async executor (both of which never return).
    #[cfg(test)]
    test_main();

    serial_println!("[kernel] Heap initialized, starting scheduler...");
    println!("--- Starting Scheduler ---");

    // Initialize the preemptive scheduler and spawn the initial process
    // (the terminal). The scheduler takes ownership of the frame allocator.
    // This never returns; the timer IRQ drives preemptive multitasking.
    self_rust_os::scheduler::init(phys_mem_offset, frame_allocator);
    #[expect(clippy::expect_used)]
    self_rust_os::scheduler::spawn_process(USER_FORGE_BIN, forge_meta::READONLY_SIZE)
        .expect("Failed to spawn initial process. Reboot required.");
    self_rust_os::scheduler::start();
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("Kernel {}", info);
    self_rust_os::hlt_loop();
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
