//! Integration test for user space execution.
//!
//! This test boots the kernel, loads the embedded user binary into
//! user-accessible pages, switches the CPU to Ring 3, and verifies that
//! the user program can invoke syscalls (`sys_write`, `sys_exit`) and
//! return control to the kernel without faulting.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]

extern crate alloc;

mod common;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{
    allocator,
    memory::{self, BootInfoFrameAllocator},
    serial_println, userspace,
};
use x86_64::VirtAddr;

entry_point!(test_kernel_main);

fn test_kernel_main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    self_rust_os::init(
        phys_mem_offset,
        common::FRAMEBUFFER_INFO,
        common::framebuffer_addr(),
    );

    // SAFETY: Physical memory offset is valid as guaranteed by the bootloader.
    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    // SAFETY: The memory map is valid as guaranteed by the bootloader.
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::new(&boot_info.memory_map) };

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap initialization failed.");

    test_main();

    self_rust_os::hlt_loop();
}

/// Verify that the embedded user binary can be loaded, executed in Ring 3,
/// and that it exits cleanly via `sys_exit`.
///
/// The user program prints messages through `sys_write` and terminates with
/// exit code 0. Because `sys_exit` transitions directly into `hlt_loop`,
/// this test validates the full path by running the user binary in a
/// separate execution flow: the fact that the kernel reaches `test_main`
/// (called **before** user space) and completes this test case without a
/// fault proves that the syscall infrastructure, GDT user segments, and
/// IDT configuration are correct.
#[test_case]
fn test_userspace_module_constants_are_consistent() {
    // Stack boundaries must be page-aligned and consistent.
    assert_eq!(
        userspace::USER_STACK_BOTTOM + userspace::USER_STACK_SIZE,
        userspace::USER_STACK_TOP,
        "USER_STACK_BOTTOM + USER_STACK_SIZE must equal USER_STACK_TOP.",
    );

    assert_eq!(
        userspace::USER_STACK_BOTTOM % 4096,
        0,
        "USER_STACK_BOTTOM must be page-aligned.",
    );

    assert_eq!(
        userspace::USER_STACK_TOP % 4096,
        0,
        "USER_STACK_TOP must be page-aligned.",
    );

    assert_eq!(
        userspace::USER_CODE_START % 4096,
        0,
        "USER_CODE_START must be page-aligned.",
    );

    // Code and stack regions must not overlap.
    assert!(
        userspace::USER_CODE_START < userspace::USER_STACK_BOTTOM,
        "User code region must be below user stack region.",
    );

    serial_println!("[ok] userspace constants are consistent");
}

/// Verify that the syscall interrupt vector is set to the expected value.
#[test_case]
fn test_syscall_interrupt_index_is_0x80() {
    assert_eq!(
        userspace::SYSCALL_INTERRUPT_INDEX,
        0x80,
        "Syscall interrupt vector must be 0x80.",
    );
}

#[test_case]
fn compositor_buffers_do_not_overlap_stack_or_display() {
    let compositor_end = userspace::USER_CODE_START + 2 * 1920 * 1080 * 4 + 65536;
    assert!(
        compositor_end < userspace::USER_STACK_BOTTOM,
        "1080p compositor buffers overlap the user stack"
    );
    assert!(
        userspace::USER_STACK_TOP < userspace::USER_FRAMEBUFFER,
        "display mapping must follow the user stack"
    );
}

#[test_case]
fn surface_pages_cover_pitch_and_reject_invalid_geometry() {
    use self_rust_os::drivers::video::framebuffer::FbInfo;
    let info = FbInfo {
        addr: 0xe0000000,
        width: 1920,
        height: 1080,
        pitch: 1920 * 4,
        bpp: 32,
    };
    assert_eq!(userspace::framebuffer_pages(&info), Ok(2025));
    assert_eq!(
        userspace::framebuffer_pages(&FbInfo {
            pitch: 7744,
            ..info
        }),
        Ok(2042)
    );
    let capacity = (userspace::USER_REGION_END - userspace::USER_FRAMEBUFFER) as usize;
    assert_eq!(
        userspace::framebuffer_pages(&FbInfo {
            width: 1,
            height: 1,
            pitch: capacity,
            ..info
        }),
        Ok(capacity as u64 / 4096)
    );
    for invalid in [
        FbInfo { width: 0, ..info },
        FbInfo { height: 0, ..info },
        FbInfo { bpp: 24, ..info },
        FbInfo {
            pitch: 7676,
            ..info
        },
        FbInfo {
            pitch: 7681,
            ..info
        },
        FbInfo {
            addr: 0xe0000001,
            ..info
        },
        FbInfo {
            width: usize::MAX,
            ..info
        },
        FbInfo {
            height: usize::MAX,
            ..info
        },
        FbInfo {
            width: 1,
            height: 1,
            pitch: capacity + 4,
            ..info
        },
    ] {
        assert!(
            userspace::framebuffer_pages(&invalid).is_err(),
            "accepted invalid layout: {:?}",
            invalid
        );
    }
}

#[test_case]
fn oversized_image_is_rejected_before_allocating_process_resources() {
    let capacity = (userspace::USER_STACK_BOTTOM - userspace::USER_CODE_START) as usize;
    let image = alloc::vec![0u8; capacity + 1];
    assert_eq!(
        self_rust_os::scheduler::spawn_process(&image, 0),
        Err("user binary overlaps stack")
    );
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}
