//! Main for little self made rust OS.
//!
//! This is the kernel entry point. It initializes all subsystems (GDT, IDT, PICs,
//! paging, heap) and then loads the desktop compositor from the boot disk.
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
use self_rust_os::memory;
use self_rust_os::{allocator, println, serial_println};
use x86_64::{structures::paging::OffsetPageTable, VirtAddr};

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
    // NOTE: Exercise the heap before scheduler init. On QEMU 11, the frame
    // allocator returns problematic low-memory frames if not advanced past
    // them. These prints allocate from the heap, moving the allocator forward.
    serial_println!("[kernel] Heap OK");

    // Initialize the keyboard scancode queue (needs heap).
    self_rust_os::task::keyboard::init_scancode_queue();
    serial_println!("[kernel] Keyboard queue OK");

    // Initialize the PS/2 mouse (IRQ12). Non-fatal if absent.
    self_rust_os::mouse::init();

    // Optional NIC bring-up owns its fixed DMA frames before scheduler startup.
    self_rust_os::net::init(mapper, &mut frame_allocator, phys_mem_offset);

    // Mount the root filesystem (needs heap for FAT32).
    // If the disk is not formatted, this will panic; the disk fixture
    // must be created with the FAT32 format.
    #[cfg(not(test))]
    {
        serial_println!("[kernel] mounting root filesystem...");
        self_rust_os::fs::init();
        serial_println!("[kernel] root filesystem mounted");
    }
    // Re-initialize the PIC: GRUB leaves it in a state where the timer
    // interrupt hangs the kernel if enabled directly.
    // NOTE: Do NOT enable interrupts here. The timer IRQ must stay disabled
    // until the scheduler has spawned its first process — if a timer fires
    // during scheduler::init/spawn_process, timer_entry runs with no
    // runnable process and corrupts the interrupt return state (double fault
    // at timer_entry's iretq). Interrupts are enabled below, right before
    // scheduler::start().
    // SAFETY: Single-threaded boot; IDT is already set up.
    // Masks: 0xF8 (master: timer, keyboard, cascade), 0xEF (slave: mouse IRQ12).
    #[cfg(feature = "grub")]
    unsafe {
        self_rust_os::interrupts::PICS.lock().initialize();
        self_rust_os::interrupts::PICS
            .lock()
            .write_masks(0xF8, 0xEF);
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
    self_rust_os::scheduler::spawn_process(&self_rust_os::scheduler::load_program_from_disk("/bin/WM.BIN").expect("WM.BIN missing from boot disk"), 0)
        .expect("Failed to spawn initial process. Reboot required.");
    // Now that the scheduler has a runnable process, unmask the timer IRQ.
    // The timer was kept masked since lib::init() to prevent timer_entry from
    // running with no scheduler state (double fault).
    // NOTE: Do NOT enable interrupts here. scheduler::start() disables them
    // during the stack switch, and the iretq to user mode restores RFLAGS
    // with IF=1. Enabling here would open a window where the timer fires
    // in kernel mode during the context-switch setup.
    // SAFETY: Single-threaded boot; scheduler is initialized.
    #[cfg(feature = "grub")]
    unsafe {
        self_rust_os::interrupts::PICS
            .lock()
            .write_masks(0xF8, 0xEF); // unmask timer (IRQ0)
    }
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
    let frame_allocator = unsafe {
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
    // Mirror the panic message to the serial port: in `-serial stdio` setups
    // (and headless runs) the VGA console is invisible, so a VGA-only panic
    // looks like a silent freeze — this exact gap cost a full debug round on
    // the FAT32 `allocate_cluster` overflow. `try_lock` (not `lock`) is used
    // so a panic that fires while the serial port is already held cannot
    // deadlock us inside the panic handler.
    x86_64::instructions::interrupts::without_interrupts(|| {
        use core::fmt::Write;
        if let Some(mut serial) = self_rust_os::serial::SERIAL1.try_lock() {
            let _ = writeln!(serial, "Kernel panic: {}", info);
        }
    });
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
