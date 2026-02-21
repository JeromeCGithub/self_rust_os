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
    allocator,
    memory::{self, BootInfoFrameAllocator},
    println, serial_println,
    task::{executor::Executor, keyboard, Task},
    userspace,
};
use x86_64::VirtAddr;

/// The embedded flat binary of the user-mode hello program.
///
/// This binary is built from `user_programs/hello/` and converted to a flat
/// binary with `llvm-objcopy -O binary`. See `user_programs/hello/build.sh`
/// for build instructions.
static USER_HELLO_BIN: &[u8] = include_bytes!("../user_programs/hello/hello.bin");

/// Auto-generated metadata about the hello binary layout (section boundaries).
///
/// Provides `READONLY_SIZE`: the page-aligned byte count of the read-only
/// region (`.text` + `.rodata`) so the kernel can enforce W^X permissions.
mod hello_meta {
    include!("../user_programs/hello/hello_meta.rs");
}

entry_point!(kernel_main);

/// This function is the entry point, since the linker looks for a function
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial_println!("[kernel] RustOS booting...");
    println!("RustOS booting...");
    println!("Initializing...");
    self_rust_os::init();
    serial_println!("[kernel] Initialization complete!");
    println!("Initialization complete !");

    // Bootloader guarantees that the physical memory is available at the passed offset.
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    // SAFETY:
    // Physical memory offset is valid as guaranteed by the bootloader.
    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    // SAFETY:
    // The memory map is valid as guaranteed by the bootloader.
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::new(&boot_info.memory_map) };

    #[expect(clippy::expect_used)]
    allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("Heap initialization failed. Reboot required.");

    // In test mode, run the test harness and exit before entering user space
    // or the async executor (both of which never return).
    #[cfg(test)]
    test_main();

    serial_println!("[kernel] Heap initialized, starting user space demo...");
    println!("--- User Space Demo ---");

    // Load and execute the embedded user binary.
    // The CPU switches to Ring 3 and the user program runs until it calls
    // `sys_exit`, at which point the syscall handler restores the kernel
    // context and process::run returns here.
    #[expect(clippy::expect_used)]
    userspace::process::run(
        USER_HELLO_BIN,
        hello_meta::READONLY_SIZE,
        &mut mapper,
        &mut frame_allocator,
    )
    .expect("Failed to launch user process. Reboot required.");

    println!("--- Returning to kernel async executor ---");

    let mut executor = Executor::new();
    executor.spawn(Task::new(keyboard::print_keypresses()));
    executor.run();
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
