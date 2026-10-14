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
    allocator, fs,
    memory::{self, BootInfoFrameAllocator},
    println, serial_println,
    task::keyboard,
    userspace,
};
use x86_64::{
    structures::paging::{FrameAllocator, OffsetPageTable, Size4KiB},
    VirtAddr,
};

/// The embedded flat binary of the user-mode terminal program.
///
/// This binary is built from `user_programs/wm/` and converted to a flat
/// binary with `llvm-objcopy -O binary`. See `tools/build-user.sh`
/// for build instructions.
static USER_WM_BIN: &[u8] = include_bytes!("../user_programs/wm/wm.bin");

/// Auto-generated metadata about the WM binary layout (section boundaries).
///
/// Provides `READONLY_SIZE`: the page-aligned byte count of the read-only
/// region (`.text` + `.rodata`) so the kernel can enforce W^X permissions.
mod wm_meta {
    include!("../user_programs/wm/wm_meta.rs");
}

/// Shared kernel bring-up used by both boot paths.
///
/// `fb_info` describes the framebuffer the bootloader configured.
/// `fb_virt_addr` is the virtual address where the framebuffer is mapped.
fn run_kernel(
    mapper: &mut OffsetPageTable<'static>,
    mut frame_allocator: self_rust_os::memory::RegionFrameAllocator,
    phys_mem_offset: VirtAddr,
    fb_info: self_rust_os::gfx::FbInfo,
    fb_virt_addr: VirtAddr,
) -> ! {
    // Initialize graphics + console first so println! works, then GDT/IDT/PICs.
    self_rust_os::init(phys_mem_offset, fb_info, fb_virt_addr);

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
        let result = allocator::init_heap(mapper, &mut frame_allocator);
        result.expect("Heap initialization failed. Reboot required.");
    }

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
    // Re-enable interrupts now that critical init is done. The PIC must be
    // re-initialized: GRUB leaves it in a state where the timer interrupt
    // hangs the kernel if enabled directly.
    // SAFETY: Single-threaded boot; IDT is already set up.
    // Masks: 0xF8 (master: timer, keyboard, cascade), 0xEF (slave: mouse IRQ12).
    #[cfg(feature = "grub")]
    unsafe {
        self_rust_os::interrupts::PICS.lock().initialize();
        self_rust_os::interrupts::PICS
            .lock()
            .write_masks(0xF8, 0xEF);
        x86_64::instructions::interrupts::enable();
    }
    // In test mode, run the test harness and exit before entering user space
    // or the async executor (both of which never return).
    #[cfg(test)]
    test_main();

    serial_println!("[kernel] Heap initialized, starting scheduler...");
    println!("--- Starting Scheduler ---");

    // Initialize the preemptive scheduler and spawn the initial process
    // (the window manager). The scheduler takes ownership of the frame allocator.
    // This never returns; the timer IRQ drives preemptive multitasking.
    self_rust_os::scheduler::init(phys_mem_offset, frame_allocator);
    #[expect(clippy::expect_used)]
    self_rust_os::scheduler::spawn_process(USER_WM_BIN, wm_meta::READONLY_SIZE)
        .expect("Failed to spawn initial process. Reboot required.");
    self_rust_os::scheduler::start();
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

    // Convert the bootloader memory map to MemRegions for RegionFrameAllocator.
    // We need a 'static slice, so use a static mut array.
    static mut LEGACY_REGIONS: [memory::MemRegion; 32] =
        [memory::MemRegion { start: 0, end: 0 }; 32];
    let mut region_count = 0;
    for region in boot_info.memory_map.iter() {
        use bootloader::bootinfo::MemoryRegionType;
        if region.region_type == MemoryRegionType::Usable && region_count < 32 {
            // SAFETY: Single-threaded boot; no other code touches this.
            unsafe {
                LEGACY_REGIONS[region_count] = memory::MemRegion {
                    start: region.range.start_addr(),
                    end: region.range.end_addr(),
                };
            }
            region_count += 1;
        }
    }
    // SAFETY: The regions describe usable RAM from the bootloader memory map.
    let mut frame_allocator = unsafe {
        memory::RegionFrameAllocator::new(&LEGACY_REGIONS[..region_count])
    };

    run_kernel(
        &mut mapper,
        frame_allocator,
        phys_mem_offset,
        // Legacy VGA path: 320x200. Note: the bootloader sets up 8bpp mode 13h,
        // but we describe it as 32bpp for API compatibility. Colors will be wrong
        // on the legacy path; use the GRUB path for correct 720p output.
        self_rust_os::gfx::FbInfo {
            addr: 0xA0000,
            width: 320,
            height: 200,
            pitch: 320 * 4,
            bpp: 32,
        },
        phys_mem_offset + 0xA0000u64,
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
