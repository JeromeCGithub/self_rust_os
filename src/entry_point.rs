//! Main for little self made rust OS.
//!
//! This is the kernel entry point. It initializes all subsystems (GDT, IDT, PICs,
//! paging, heap) and then loads and executes the embedded user-mode binary.
//!
//! Two boot paths are supported:
//! - legacy (default): the `bootloader` crate's `entry_point!` via `cargo bootimage`;
//! - `grub` cargo feature: GRUB2 loads the kernel with Multiboot2 (see
//!   `grub_boot.rs`), and [`grub_entry_rust`] takes over.
//!
//! [`grub_entry_rust`]: grub_boot::grub_entry_rust

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]

extern crate alloc;

#[cfg(not(feature = "grub"))]
use bootloader::{entry_point, BootInfo};
#[cfg(feature = "grub")]
mod grub_boot;

use core::panic::PanicInfo;
#[cfg(not(feature = "grub"))]
use self_rust_os::memory::{self, BootInfoFrameAllocator};
#[cfg(not(feature = "grub"))]
use self_rust_os::gfx;
use self_rust_os::{
    allocator,
    println, serial_println,
    task::{executor::Executor, keyboard, Task},
    userspace,
};
use x86_64::{
    structures::paging::{FrameAllocator, OffsetPageTable, Size4KiB},
    VirtAddr,
};

/// The embedded flat binary of the user-mode terminal program.
///
/// This binary is built from `user_programs/terminal/` and converted to a flat
/// binary with `llvm-objcopy -O binary`. See `user_programs/terminal/build.sh`
/// for build instructions.
static USER_TERMINAL_BIN: &[u8] = include_bytes!("../user_programs/terminal/terminal.bin");

/// Auto-generated metadata about the terminal binary layout (section boundaries).
///
/// Provides `READONLY_SIZE`: the page-aligned byte count of the read-only
/// region (`.text` + `.rodata`) so the kernel can enforce W^X permissions.
mod terminal_meta {
    include!("../user_programs/terminal/terminal_meta.rs");
}

/// Shared kernel bring-up used by both boot paths.
///
/// `fb_phys_addr` is the physical address of the linear framebuffer the
/// bootloader configured, with the given dimensions in pixels.
fn run_kernel(
    mapper: &mut OffsetPageTable<'static>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    phys_mem_offset: VirtAddr,
    fb_phys_addr: u64,
    fb_width: usize,
    fb_height: usize,
) -> ! {
    // Initialize graphics + console first so println! works, then GDT/IDT/PICs.
    self_rust_os::init(phys_mem_offset, fb_phys_addr, fb_width, fb_height);

    serial_println!("[kernel] RustOS booting...");
    println!("RustOS booting...");
    println!("Initializing...");
    serial_println!("[kernel] Initialization complete!");
    println!("Initialization complete !");

    #[expect(clippy::expect_used, reason = "Heap is required to boot")]
    {
        // Disable interrupts during heap init: the timer interrupt fires
        // during mapping and the x86_64 OffsetPageTable is not reentrant.
        x86_64::instructions::interrupts::disable();
        let result = allocator::init_heap(mapper, frame_allocator);
        result.expect("Heap initialization failed. Reboot required.");
    }

    // Initialize the keyboard scancode queue (needs heap).
    self_rust_os::task::keyboard::init_scancode_queue();

    // Mount the root filesystem (needs heap for FAT32).
    // If the disk is not formatted, this will panic; the disk fixture
    // must be created with the FAT32 format.
    #[cfg(not(test))]
    {
        serial_println!("[kernel] mounting root filesystem...");
        self_rust_os::fs::init();
        serial_println!("[kernel] root filesystem mounted");
    }

    // Re-enable interrupts now that critical init is done. The PIC must be
    // re-initialized: GRUB leaves it in a state where the timer interrupt
    // hangs the kernel if enabled directly.
    // SAFETY: Single-threaded boot; IDT is already set up.
    unsafe {
        self_rust_os::interrupts::PICS.lock().initialize();
        self_rust_os::interrupts::PICS
            .lock()
            .write_masks(0xFC, 0xFF);
        x86_64::instructions::interrupts::enable();
    }

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
    #[expect(clippy::expect_used, reason = "Cannot boot without the user program")]
    userspace::process::run(
        USER_TERMINAL_BIN,
        terminal_meta::READONLY_SIZE,
        mapper,
        frame_allocator,
    )
    .expect("Failed to launch user process. Reboot required.");

    println!("--- Returning to kernel async executor ---");

    let mut executor = Executor::new();
    executor.spawn(Task::new(keyboard::print_keypresses()));
    executor.run();
}

#[cfg(not(feature = "grub"))]
entry_point!(kernel_main);

/// Legacy boot path: entry via the `bootloader` crate.
///
/// This function is the entry point, since the linker looks for a function
#[cfg(not(feature = "grub"))]
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // Bootloader guarantees that the physical memory is available at the passed offset.
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);

    // SAFETY:
    // Physical memory offset is valid as guaranteed by the bootloader.
    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    // SAFETY:
    // The memory map is valid as guaranteed by the bootloader.
    let mut frame_allocator =
        unsafe { BootInfoFrameAllocator::new(&boot_info.memory_map) };

    run_kernel(
        &mut mapper,
        &mut frame_allocator,
        phys_mem_offset,
        gfx::FRAMEBUFFER_PHYS_ADDR,
        gfx::WIDTH,
        gfx::HEIGHT,
    )
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
