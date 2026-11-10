//! Native file quotas on the explicit disposable secondary-master FAT32 fixture.
#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use bootloader::BootInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use self_rust_os::{allocator, drivers::video::framebuffer::FbInfo, memory, scheduler};
use x86_64::{structures::paging::FrameAllocator, VirtAddr};

const PITCH: usize = 640 * 4;
const BYTES: usize = PITCH * 480;
// Initialization and tests run on one CPU in sequence; Relaxed needs no
// inter-thread publication and is used only to store the backing address.
static DISPLAY_ADDR: AtomicU64 = AtomicU64::new(0);
static mut REGIONS: [memory::MemRegion; 32] = [memory::MemRegion { start: 0, end: 0 }; 32];

// Keep the macro's public ABI wrapper scoped inside the test module.
mod boot_entry {
    bootloader::entry_point!(super::main);
}

fn main(info: &'static BootInfo) -> ! {
    let offset = VirtAddr::new(info.physical_memory_offset);
    // SAFETY: The bootloader maps all physical RAM at this offset. This is
    // the only mapper and allocator for the active address space.
    let mut mapper = unsafe { memory::init(offset) };
    // SAFETY: The bootloader's map describes unowned usable frames.
    let mut frames = unsafe { memory::BootInfoFrameAllocator::new(&info.memory_map) };
    allocator::init_heap(&mut mapper, &mut frames).expect("heap initialization");
    // Keep the large RAM display on the higher-half heap: placing it in
    // low kernel BSS would overlap the reserved user virtual range.
    let display = alloc::vec![0u32; BYTES / 4 + 4].into_boxed_slice();
    DISPLAY_ADDR.store(display.as_ptr() as u64, Ordering::Relaxed);
    self_rust_os::init(
        offset,
        FbInfo {
            addr: 0,
            width: 640,
            height: 480,
            pitch: PITCH,
            bpp: 32,
        },
        VirtAddr::from_ptr(display.as_ptr()),
    );
    let first_free = frames
        .allocate_frame()
        .expect("remaining RAM")
        .start_address()
        .as_u64();
    let mut count = 0;
    for region in info.memory_map.iter() {
        if region.region_type != bootloader::bootinfo::MemoryRegionType::Usable {
            continue;
        }
        let start = region.range.start_addr().max(first_free);
        let end = region.range.end_addr();
        if start < end {
            // SAFETY: Single-threaded initialization, bounded by this fixed
            // array; these frames follow every frame used for the heap.
            assert!(count < 32);
            unsafe {
                REGIONS[count] = memory::MemRegion { start, end };
            }
            count += 1;
        }
    }
    // SAFETY: Ownership of all remaining frames is transferred to the
    // scheduler. `frames` is never used again and REGIONS stays immutable.
    let allocator = unsafe { memory::RegionFrameAllocator::new(&REGIONS[..count]) };
    scheduler::init(offset, allocator);
    scheduler::configure_native_init().expect("native-init profile must reserve PID 0");
    // This test owns tests/fixtures/disk-fat32.img (16 MiB secondary master).
    // Run serially with ATA/FAT32 fixture tests; never substitute a host disk.
    let disk = self_rust_os::drivers::block::ata::AtaDisk::secondary(
        self_rust_os::drivers::block::ata::AtaDrive::Master,
    );
    let fs = self_rust_os::fat32::Fat32::format(disk, 32768, b"SCOPE TEST ").unwrap();
    drop(fs);
    self_rust_os::fs::init();
    test_main();
    self_rust_os::hlt_loop();
}

use rustos_process_abi::principal::Capabilities;
use rustos_scope_core::{Usage, SYSTEM};
fn limits(descriptors: u64) -> Usage {
    Usage {
        processes: 1,
        threads: 1,
        pages: 4096,
        kernel_bytes: 1024 * 1024,
        descriptors,
        ..Usage::ZERO
    }
}
fn input(bytes: &[u8]) -> u64 {
    let address = self_rust_os::userspace::USER_STACK_TOP - 4096;
    assert!(self_rust_os::userspace::usercopy::to_user(address, bytes));
    address
}
/// File admission, temporary buffers and retained leases preserve scope lifetimes.
#[test_case]
fn native_file_quota_precedes_create_and_truncate() {
    assert_eq!(scheduler::spawn_process(&[0xc3], 4096), Ok(1));
    let mut context = scheduler::schedule(0);
    let initial = scheduler::scope_status(SYSTEM).unwrap().used;
    self_rust_os::fs::with_fs(|fs| {
        fs.create_file("/LIMIT.TXT").unwrap();
        fs.write_file("/LIMIT.TXT", b"untouched").unwrap();
    })
    .unwrap();
    let scope = scheduler::scope_create_service(SYSTEM, limits(0)).unwrap();
    let caps =
        Capabilities::from_bits(Capabilities::FILE_READ.bits() | Capabilities::FILE_WRITE.bits())
            .unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps).unwrap();
    activate(child, &mut context);
    for _ in 0..3 {
        let path = input(b"/LIMIT.TXT\0");
        assert_eq!(
            self_rust_os::fs::file_syscall(23, path, 1 | 0o1000, 0),
            u64::MAX - 1,
            "scope descriptor admission must precede truncate"
        );
        let path = input(b"/NEW.TXT\0");
        assert_eq!(
            self_rust_os::fs::file_syscall(23, path, 1 | 0o100, 0),
            u64::MAX - 1,
            "scope descriptor admission must precede create"
        );
    }
    self_rust_os::fs::with_fs(|fs| {
        let mut bytes = [0; 9];
        assert_eq!(fs.read_file("/LIMIT.TXT", &mut bytes).unwrap(), 9);
        assert_eq!(&bytes, b"untouched");
        assert!(fs.file_size("/NEW.TXT").is_err());
    })
    .unwrap();
    activate(1, &mut context);
    assert_eq!(scheduler::scope_status(scope).unwrap().used.descriptors, 0);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
    file_success_failure_and_io_budgets(&mut context, initial);
    retained_resource_blocks_scope_cleanup(&mut context, initial);
    descriptor_recovery_capacity(&mut context, initial);
}
fn file_success_failure_and_io_budgets(context: &mut u64, initial: Usage) {
    use self_rust_os::fs::{file_read, file_syscall as call};
    let caps =
        Capabilities::from_bits(Capabilities::FILE_READ.bits() | Capabilities::FILE_WRITE.bits())
            .unwrap();
    let scope = scheduler::scope_create_service(SYSTEM, limits(1)).unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps).unwrap();
    activate(child, context);
    let output = self_rust_os::userspace::USER_STACK_TOP - 128 * 1024;
    for _ in 0..16 {
        assert_eq!(call(23, input(b"/MISSING.TXT\0"), 0, 0), u64::MAX - 1);
        let fd = call(23, input(b"/LIMIT.TXT\0"), 2, 0);
        assert_eq!(
            fd, 3,
            "failed open must restore the slot and scope reservation"
        );
        assert_eq!(call(23, input(b"/NEW.TXT\0"), 1 | 0o100, 0), u64::MAX - 1);
        assert_eq!(file_read(fd, output, 5), 5);
        let mut bytes = [0; 5];
        assert!(self_rust_os::userspace::usercopy::from_user(
            output, &mut bytes
        ));
        assert_eq!(&bytes, b"untou");
        assert_eq!(call(26, fd, 0, 1), 5);
        for (pointer, length) in [(0, 1), (0x400000, 1), (u64::MAX - 8, 16)] {
            assert_eq!(file_read(fd, pointer, length), u64::MAX - 1);
            assert_eq!(
                call(26, fd, 0, 1),
                5,
                "invalid copy must not advance the cursor"
            );
        }
        assert_eq!(call(24, fd, 0, 0), 0);
    }
    let fd = call(23, input(b"/LIMIT.TXT\0"), 2, 0);
    assert_eq!(fd, 3);
    activate(1, context);
    scheduler::reap_retired();
    let open = scheduler::scope_status(scope).unwrap().used;
    assert_eq!(open.descriptors, 1);
    self_rust_os::fs::with_fs(|fs| assert!(fs.file_size("/NEW.TXT").is_err())).unwrap();
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);

    let mut tight = limits(1);
    tight.kernel_bytes = open.kernel_bytes;
    let scope = scheduler::scope_create_service(SYSTEM, tight).unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps).unwrap();
    activate(child, context);
    let fd = call(23, input(b"/LIMIT.TXT\0"), 2, 0);
    assert_eq!(fd, 3);
    for _ in 0..3 {
        assert_eq!(
            file_read(fd, output, 1),
            u64::MAX - 1,
            "I/O buffer admission precedes read/cursor effects"
        );
        assert_eq!(
            call(25, fd, input(b"x"), 1),
            u64::MAX - 1,
            "I/O buffer admission precedes write effects"
        );
        assert_eq!(file_read(fd, output, 0), 0);
        assert_eq!(call(26, fd, 0, 1), 0);
    }
    activate(1, context);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_status(scope).unwrap().used, open);
    self_rust_os::fs::with_fs(|fs| {
        let mut bytes = [0; 9];
        assert_eq!(fs.read_file("/LIMIT.TXT", &mut bytes).unwrap(), 9);
        assert_eq!(&bytes, b"untouched");
    })
    .unwrap();
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
}
fn retained_resource_blocks_scope_cleanup(context: &mut u64, initial: Usage) {
    use alloc::{sync::Arc, vec::Vec};
    use rustos_scope_core::{Error, State};
    struct RetainedFile {
        _bytes: Vec<u8>,
        _lease: scheduler::ResourceLease,
    }
    let scope = scheduler::scope_create_service(SYSTEM, limits(1)).unwrap();
    let child =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, Capabilities::NONE)
            .unwrap();
    activate(child, context);
    let lease = scheduler::reserve_resource(Usage {
        descriptors: 1,
        kernel_bytes: 128,
        ..Usage::ZERO
    })
    .unwrap();
    assert!(matches!(
        scheduler::reserve_resource(Usage {
            descriptors: 1,
            ..Usage::ZERO
        }),
        Err(Error::Limit)
    ));
    let file = Arc::new(RetainedFile {
        _bytes: alloc::vec![0;128],
        _lease: lease,
    });
    let inflight = Arc::clone(&file);
    drop(file);
    activate(1, context);
    assert_eq!(
        scheduler::grow_resource(
            &inflight._lease,
            Usage {
                kernel_bytes: 1,
                ..Usage::ZERO
            }
        ),
        Err(Error::Denied)
    );
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    let held = scheduler::scope_status(scope).unwrap();
    assert_eq!(held.state, State::Closing);
    assert_eq!(
        held.used,
        Usage {
            descriptors: 1,
            kernel_bytes: 128,
            ..Usage::ZERO
        },
        "object lease outlives every process/frame/stack reservation"
    );
    assert!(scheduler::scope_next_closed().unwrap().is_none());
    drop(inflight);
    assert_eq!(
        scheduler::scope_status(scope).unwrap().state,
        State::Closing,
        "queued release retains quota until scheduler collection"
    );
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_next_closed().unwrap().unwrap().id, scope);
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
    assert_eq!(scheduler::frame_count(), initial.pages);
}
fn descriptor_recovery_capacity(context: &mut u64, initial: Usage) {
    use rustos_scope_core::Error;
    let scope = scheduler::scope_create_service(SYSTEM, limits(4096)).unwrap();
    let child =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, Capabilities::NONE)
            .unwrap();
    activate(child, context);
    // Exercise the native bootstrap ceilings with an unpublished aggregate
    // reservation; this is not a claim that 4064 real descriptors were opened.
    let user = scheduler::reserve_resource(Usage {
        descriptors: 4064,
        ..Usage::ZERO
    })
    .unwrap();
    assert!(matches!(
        scheduler::reserve_resource(Usage {
            descriptors: 1,
            ..Usage::ZERO
        }),
        Err(Error::Limit)
    ));
    activate(1, context);
    let recovery = scheduler::reserve_resource(Usage {
        descriptors: 32,
        ..Usage::ZERO
    })
    .unwrap();
    assert!(matches!(
        scheduler::reserve_resource(Usage {
            descriptors: 1,
            ..Usage::ZERO
        }),
        Err(Error::Limit)
    ));
    drop(user);
    drop(recovery);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_status(scope).unwrap().used.descriptors, 0);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
}
fn activate(pid: u32, context: &mut u64) {
    for _ in 0..64 {
        if scheduler::current_pid() == Some(pid) {
            return;
        }
        let next = scheduler::on_timer(*context);
        if next != 0 {
            *context = next;
        }
    }
    panic!("process did not become current");
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}
