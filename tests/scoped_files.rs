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
    self_rust_os::fs::with_fs(|fs| {
        fs.create_file("/INIT.BIN").unwrap();
        fs.write_file("/INIT.BIN", &[0xc3]).unwrap();
    })
    .unwrap();
    let image = scheduler::load_program_from_disk("/INIT.BIN").unwrap();
    assert_eq!(scheduler::spawn_process(&image, 4096), Ok(1));
    let allocated = allocator::allocated_bytes();
    drop(image);
    assert!(
        allocator::allocated_bytes() < allocated,
        "boot staging is freed after private image copying"
    );
    let mut context = scheduler::schedule(0);
    let mut code = [0];
    assert!(self_rust_os::userspace::usercopy::from_user(
        0x400000, &mut code
    ));
    assert_eq!(code, [0xc3]);
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
    stopped_filesystem_operation_retains_scope(&mut context, initial);
    actual_filesystem_work_retains_scope(&mut context, initial);
    initial_session_grants_publish_with_the_leader(&mut context, initial);
    directory_grants_cover_paths_descriptors_and_children(&mut context, initial);
    directory_output_requires_budget(&mut context, initial);
    path_storage_requires_budget(&mut context, initial);
    file_success_failure_and_io_budgets(&mut context, initial);
    retained_resource_blocks_scope_cleanup(&mut context, initial);
    descriptor_recovery_capacity(&mut context, initial);
    path_and_loader_buffers_require_budget(&mut context, initial);
    launch_vectors_require_budget(&mut context, initial);
    image_snapshot_requires_budget(&mut context, initial);
}
fn stopped_filesystem_operation_retains_scope(context: &mut u64, initial: Usage) {
    let frames = scheduler::frame_count();
    let scope = scheduler::scope_create_service(SYSTEM, limits(0)).unwrap();
    let child = scheduler::spawn_process_in_scope(
        &[0xc3], 4096, &[], &[], scope, Capabilities::FILE_READ,
    ).unwrap();
    activate(child, context);
    let operation = scheduler::begin_filesystem_operation().expect("live process admits I/O");
    let second = scheduler::begin_filesystem_operation().expect("second I/O admitted");
    assert!(!operation.cancelled());
    activate(1, context);
    scheduler::scope_close(scope).unwrap();
    assert!(operation.cancelled(), "stop must cancel outstanding I/O");
    assert!(second.cancelled());
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    assert_ne!(scheduler::scope_status(scope).unwrap().used, Usage::ZERO,
        "scope must retain process resources while I/O remains active");
    assert!(scheduler::scope_ack_closed(scope).is_err());
    drop(operation);
    scheduler::reap_retired();
    assert!(scheduler::scope_ack_closed(scope).is_err(), "all I/O pins must drain");
    drop(second);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
    assert_eq!(scheduler::frame_count(), frames, "all process frames must be recovered");
}
fn actual_filesystem_work_retains_scope(context: &mut u64, initial: Usage) {
    let scope = scheduler::scope_create_service(SYSTEM, limits(0)).unwrap();
    let child = scheduler::spawn_process_in_scope(
        &[0xc3], 4096, &[], &[], scope, Capabilities::FILE_READ,
    ).unwrap();
    activate(child, context);
    self_rust_os::fs::with_fs(|fs| {
        activate(1, context);
        scheduler::scope_close(scope).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        assert_ne!(scheduler::scope_status(scope).unwrap().used, Usage::ZERO,
            "filesystem callback must retain the issuing process");
        assert!(scheduler::scope_ack_closed(scope).is_err());
        let mut bytes = [0; 9];
        assert_eq!(fs.read_file("/LIMIT.TXT", &mut bytes), Err(self_rust_os::fat32::FatError::Cancelled));
    }).unwrap();
    self_rust_os::fs::with_fs(|fs| {
        let mut bytes = [0; 9];
        assert_eq!(fs.read_file("/LIMIT.TXT", &mut bytes), Ok(9));
        assert_eq!(&bytes, b"untouched");
    }).unwrap();
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
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
    // Path copying now needs temporary admission before the FD exists. Allow
    // that setup, then exhaust the exact remaining bytes before I/O assertions.
    tight.kernel_bytes = open.kernel_bytes + 16 * 1024;
    let scope = scheduler::scope_create_service(SYSTEM, tight).unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps).unwrap();
    activate(child, context);
    let fd = call(23, input(b"/LIMIT.TXT\0"), 2, 0);
    assert_eq!(fd, 3);
    let pressure = scheduler::reserve_resource(Usage {
        kernel_bytes: tight.kernel_bytes - open.kernel_bytes,
        ..Usage::ZERO
    })
    .unwrap();
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
    drop(pressure);
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
fn path_and_loader_buffers_require_budget(context: &mut u64, initial: Usage) {
    self_rust_os::fs::with_fs(|fs| {
        fs.create_file("/EXEC.BIN").unwrap();
        fs.write_file("/EXEC.BIN", &[0xc3]).unwrap();
    })
    .unwrap();
    let limit = limits(0);
    let scope = scheduler::scope_create_service(SYSTEM, limit).unwrap();
    let caps = Capabilities::from_bits(
        Capabilities::FILE_READ.bits()
            | Capabilities::FILE_WRITE.bits()
            | Capabilities::SPAWN.bits(),
    )
    .unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps).unwrap();
    let base = scheduler::scope_status(scope).unwrap().used;
    activate(child, context);
    let pressure = scheduler::reserve_resource(Usage {
        kernel_bytes: limit.kernel_bytes - base.kernel_bytes,
        ..Usage::ZERO
    })
    .unwrap();
    let output = self_rust_os::userspace::USER_STACK_TOP - 128 * 1024;
    let path = input(b"/LIMIT.TXT\0");
    for _ in 0..3 {
        assert!(
            scheduler::load_program_from_disk("/EXEC.BIN").is_err(),
            "executable staging must reserve the caller's native byte budget"
        );
        assert!(self_rust_os::userspace::usercopy::to_user(
            output, &[0xa5; 9]
        ));
        assert_eq!(self_rust_os::fs::read_path(path, output, 9), u64::MAX - 1);
        let mut untouched = [0; 9];
        assert!(self_rust_os::userspace::usercopy::from_user(
            output,
            &mut untouched
        ));
        assert_eq!(untouched, [0xa5; 9]);
        assert!(self_rust_os::userspace::usercopy::to_user(
            output, b"changed"
        ));
        assert_eq!(self_rust_os::fs::write_path(path, output, 7), u64::MAX - 1);
    }
    self_rust_os::fs::with_fs(|fs| {
        let mut bytes = [0; 9];
        assert_eq!(fs.read_file("/LIMIT.TXT", &mut bytes).unwrap(), 9);
        assert_eq!(&bytes, b"untouched");
    })
    .unwrap();
    drop(pressure);
    activate(1, context);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_status(scope).unwrap().used, base);
    activate(child, context);
    let empty = scheduler::ResourceBuffer::zeroed(0).unwrap();
    assert!(empty.is_empty());
    drop(empty);
    let mut binary = scheduler::load_program_from_disk("/EXEC.BIN").unwrap();
    assert_eq!(&binary[..], &[0xc3]);
    binary[0] = 0x90;
    activate(1, context);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    assert_eq!(
        scheduler::scope_status(scope).unwrap().used,
        Usage {
            kernel_bytes: 1,
            ..Usage::ZERO
        }
    );
    assert!(scheduler::scope_next_closed().unwrap().is_none());
    drop(binary);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
}
fn launch_vectors_require_budget(context: &mut u64, initial: Usage) {
    use core::cell::Cell;
    let limit = limits(0);
    let scope = scheduler::scope_create_service(SYSTEM, limit).unwrap();
    let child =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, Capabilities::NONE)
            .unwrap();
    let base = scheduler::scope_status(scope).unwrap().used;
    activate(child, context);
    let address = self_rust_os::userspace::USER_STACK_TOP - 8192;
    assert!(self_rust_os::userspace::usercopy::to_user(address, b"arg0"));
    let mut record = [0; 16];
    record[..8].copy_from_slice(&address.to_le_bytes());
    record[8..].copy_from_slice(&4u64.to_le_bytes());
    assert!(self_rust_os::userspace::usercopy::to_user(
        address + 32,
        &record
    ));
    let request = rustos_ipc_abi::Spawn {
        argv: address + 32,
        argc: 1,
        envp: 0,
        envc: 0,
        bindings: 0,
        binding_count: 0,
    };
    for spare in [0, 16] {
        let pressure = scheduler::reserve_resource(Usage {
            kernel_bytes: limit.kernel_bytes - base.kernel_bytes - spare,
            ..Usage::ZERO
        })
        .unwrap();
        let called = Cell::new(false);
        assert_eq!(
            self_rust_os::userspace::with_launch_snapshot(request, |_, _, _| {
                called.set(true);
                77
            }),
            u64::MAX - 1,
            "vector records and strings must be reserved before launch callback"
        );
        assert!(!called.get());
        drop(pressure);
        scheduler::reap_retired();
    }
    assert_eq!(
        self_rust_os::userspace::with_launch_snapshot(request, |argv, envp, bindings| {
            assert_eq!(argv, &["arg0"]);
            assert!(envp.is_empty() && bindings.is_empty());
            77
        }),
        77
    );
    for malformed in [
        rustos_ipc_abi::Spawn {
            argc: rustos_process_abi::MAX_VECTOR as u64 + 1,
            ..request
        },
        rustos_ipc_abi::Spawn {
            envc: rustos_process_abi::MAX_VECTOR as u64 + 1,
            ..request
        },
        rustos_ipc_abi::Spawn {
            binding_count: rustos_ipc_abi::MAX_BINDINGS as u64 + 1,
            ..request
        },
    ] {
        assert_eq!(
            self_rust_os::userspace::with_launch_snapshot(malformed, |_, _, _| {
                panic!("malformed counts reached launch callback")
            }),
            u64::MAX - 1
        );
    }
    activate(1, context);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_status(scope).unwrap().used, base);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
}
fn image_snapshot_requires_budget(context: &mut u64, initial: Usage) {
    let mut limit = limits(0);
    limit.processes = 2;
    limit.threads = 2;
    limit.pages = 8192;
    let scope = scheduler::scope_create_service(SYSTEM, limit).unwrap();
    let child =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, Capabilities::SPAWN)
            .unwrap();
    let base = scheduler::scope_status(scope).unwrap().used;
    activate(child, context);
    let mut image = alloc::vec![0;65536];
    image[0] = 0xc3;
    let pointer = self_rust_os::userspace::USER_STACK_TOP - 256 * 1024;
    assert!(self_rust_os::userspace::usercopy::to_user(pointer, &image));
    let request = rustos_ipc_abi::ImageSpawn {
        launch: rustos_ipc_abi::Spawn {
            argv: 0,
            argc: 0,
            envp: 0,
            envc: 0,
            bindings: 0,
            binding_count: 0,
        },
        image_length: 65536,
    }
    .encode()
    .unwrap();
    let packet = input(&request);
    let pressure = scheduler::reserve_resource(Usage {
        kernel_bytes: limit.kernel_bytes - base.kernel_bytes - 40 * 1024,
        ..Usage::ZERO
    })
    .unwrap();
    // The trusted constructor alone fits the remaining allowance: the rejected
    // operation is specifically the additional copied user-image snapshot.
    let control =
        scheduler::spawn_process(&image, 0).expect("construction fits without a snapshot");
    scheduler::kill(control).unwrap();
    assert_eq!(scheduler::waitpid(control), 9);
    scheduler::reap_retired();
    drop(image);
    for _ in 0..3 {
        assert_eq!(
            self_rust_os::userspace::spawn_from_image(pointer, packet),
            u64::MAX - 1,
            "mapped-image snapshots cannot bypass source kernel-byte quota"
        );
    }
    drop(pressure);
    activate(1, context);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_status(scope).unwrap().used, base);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
}
fn path_storage_requires_budget(context: &mut u64, initial: Usage) {
    use self_rust_os::fs::file_syscall as call;
    self_rust_os::fs::with_fs(|fs| fs.create_dir("/WORK").unwrap()).unwrap();
    let limit = limits(1);
    let scope = scheduler::scope_create_service(SYSTEM, limit).unwrap();
    let caps =
        Capabilities::from_bits(Capabilities::FILE_READ.bits() | Capabilities::FILE_WRITE.bits())
            .unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps).unwrap();
    let base = scheduler::scope_status(scope).unwrap().used;
    activate(child, context);
    let output = self_rust_os::userspace::USER_STACK_TOP - 128 * 1024;
    let pressure = scheduler::reserve_resource(Usage {
        kernel_bytes: limit.kernel_bytes - base.kernel_bytes,
        ..Usage::ZERO
    })
    .unwrap();
    for _ in 0..3 {
        assert!(self_rust_os::userspace::usercopy::to_user(
            output,
            &[0xa5; 16]
        ));
        assert_eq!(
            call(28, output, 16, 0),
            u64::MAX - 1,
            "getcwd must reserve copied path storage"
        );
        let mut unchanged = [0; 16];
        assert!(self_rust_os::userspace::usercopy::from_user(
            output,
            &mut unchanged
        ));
        assert_eq!(unchanged, [0xa5; 16]);
        assert_eq!(
            call(27, input(b"/WORK\0"), 0, 0),
            u64::MAX - 1,
            "failed path admission must preserve cwd"
        );
    }
    drop(pressure);
    scheduler::reap_retired();
    let mut bounded = scheduler::ResourceString::with_capacity(4).unwrap();
    bounded.push_str("é").unwrap();
    let heap = allocator::allocated_bytes();
    assert!(bounded.push_str("日").is_err());
    assert_eq!(bounded.as_str(), "é");
    assert_eq!(allocator::allocated_bytes(), heap);
    drop(bounded);
    let edge = self_rust_os::userspace::USER_STACK_TOP - 1;
    assert!(self_rust_os::userspace::usercopy::to_user(edge, &[0]));
    let empty = self_rust_os::userspace::usercopy::cstring(edge, 4096).unwrap();
    assert!(empty.is_empty());
    drop(empty);
    let unterminated = self_rust_os::userspace::USER_STACK_TOP - 8192;
    let contents = alloc::vec![b'x';4096];
    assert!(self_rust_os::userspace::usercopy::to_user(
        unterminated,
        &contents
    ));
    drop(contents);
    assert!(self_rust_os::userspace::usercopy::cstring(unterminated, 4096).is_none());
    assert_eq!(call(28, output, 16, 0), 2);
    let mut bytes = [0; 2];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    assert_eq!(&bytes, b"/\0");
    assert_eq!(call(27, input(b"/WORK\0"), 0, 0), 0);
    activate(1, context);
    scheduler::reap_retired();
    let changed = scheduler::scope_status(scope).unwrap().used;
    assert!(
        changed.kernel_bytes > base.kernel_bytes,
        "published cwd storage retains its own charge"
    );
    activate(child, context);
    assert_eq!(call(27, input(b"/MISSING/..\0"), 0, 0), u64::MAX - 1);
    assert_eq!(call(28, output, 16, 0), 6);
    let mut bytes = [0; 6];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    assert_eq!(&bytes, b"/WORK\0");
    activate(1, context);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_status(scope).unwrap().used, changed);
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

fn directory_output_requires_budget(context: &mut u64, initial: Usage) {
    use self_rust_os::fs::read_directory;
    self_rust_os::fs::with_fs(|fs| {
        fs.create_dir("/LISTING").unwrap();
        for index in 0..40 {
            let name = alloc::format!("/LISTING/{index:03} café 文書 {}.txt", "x".repeat(210));
            fs.create_file(&name).unwrap();
        }
    })
    .unwrap();
    let limit = limits(0);
    let scope = scheduler::scope_create_service(SYSTEM, limit).unwrap();
    let child =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, Capabilities::FILE_READ)
            .unwrap();
    let base = scheduler::scope_status(scope).unwrap().used;
    activate(child, context);
    let output = self_rust_os::userspace::USER_STACK_TOP - 128 * 1024;
    let size = read_directory(input(b"/LISTING\0"), 0, 0);
    assert!(size > 8192 && size < 64 * 1024);
    let pressure = scheduler::reserve_resource(Usage {
        kernel_bytes: limit.kernel_bytes - base.kernel_bytes - 8192,
        ..Usage::ZERO
    })
    .unwrap();
    for _ in 0..3 {
        assert_eq!(read_directory(input(b"/LISTING\0"), 0, 0), size);
        assert!(self_rust_os::userspace::usercopy::to_user(
            output,
            &[0xa5; 32]
        ));
        assert_eq!(
            read_directory(input(b"/LISTING\0"), output, size),
            u64::MAX - 1,
            "directory output must be admitted before allocating or copying records"
        );
        let mut unchanged = [0; 32];
        assert!(self_rust_os::userspace::usercopy::from_user(
            output,
            &mut unchanged
        ));
        assert_eq!(unchanged, [0xa5; 32]);
    }
    drop(pressure);
    scheduler::reap_retired();
    assert_eq!(
        read_directory(input(b"/LISTING\0"), output, size - 1),
        u64::MAX - 1
    );
    for pointer in [
        0,
        0x400000,
        u64::MAX - 8,
        self_rust_os::userspace::USER_STACK_TOP - 1,
    ] {
        assert_eq!(
            read_directory(input(b"/LISTING\0"), pointer, size),
            u64::MAX - 1
        );
    }
    assert_eq!(read_directory(input(b"/LISTING\0"), output, size), size);
    let mut bytes = alloc::vec![0; size as usize];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    let mut position = 0;
    for index in 0..40 {
        let record = &bytes[position..];
        let length = u16::from_le_bytes(record[..2].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes(record[2..4].try_into().unwrap()) as usize;
        let name = alloc::format!("{index:03} café 文書 {}.txt", "x".repeat(210));
        assert_eq!(&record[16..16 + name_len], name.as_bytes());
        assert!(record[16 + name_len..length].iter().all(|&b| b == 0));
        position += length;
    }
    assert_eq!(position, size as usize);
    for maximum in [0, 1, 7, 40, 41] {
        let count = maximum.min(40);
        assert_eq!(
            self_rust_os::fs::list_directory(input(b"/LISTING\0"), output, maximum),
            count
        );
        let mut short = alloc::vec![0; count as usize * 12];
        assert!(self_rust_os::userspace::usercopy::from_user(
            output, &mut short
        ));
        for index in 0..count as usize {
            assert_eq!(short[index * 12 + 11], 0x20);
            assert_eq!(
                &short[index * 12..index * 12 + 3],
                alloc::format!("{index:03}").as_bytes()
            );
        }
    }
    assert_eq!(
        self_rust_os::fs::list_directory(input(b"/LISTING\0"), output, u64::MAX),
        u64::MAX - 1
    );
    assert_eq!(
        self_rust_os::fs::list_directory(input(b"/LISTING\0"), 0, 1),
        u64::MAX - 1
    );
    assert_eq!(
        read_directory(input(b"/LIMIT.TXT\0"), output, size),
        u64::MAX - 1
    );
    assert_eq!(
        read_directory(input(b"/MISSING\0"), output, size),
        u64::MAX - 1
    );
    activate(1, context);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_status(scope).unwrap().used, base);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
}

fn directory_grants_cover_paths_descriptors_and_children(context: &mut u64, initial: Usage) {
    use self_rust_os::fs::{self, FileRights};
    assert!(
        scheduler::restrict_filesystem(&[]).is_err(),
        "init must retain recovery access"
    );
    fs::with_fs(|fs| {
        for directory in ["/GRANTHOME", "/GRANTTMP", "/GRANTPUBLIC", "/FOREIGN"] {
            fs.create_dir(directory).unwrap();
        }
        for path in [
            "/GRANTHOME/DATA",
            "/GRANTPUBLIC/PROBE.BIN",
            "/FOREIGN/SECRET",
        ] {
            fs.create_file(path).unwrap();
            fs.write_file(path, b"abc").unwrap();
        }
    })
    .unwrap();
    let mut limit = limits(6);
    limit.processes = 2;
    limit.threads = 2;
    let scope = scheduler::scope_create_service(SYSTEM, limit).unwrap();
    let caps = Capabilities::from_bits(
        Capabilities::FILE_READ.bits()
            | Capabilities::FILE_WRITE.bits()
            | Capabilities::SPAWN.bits(),
    )
    .unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps).unwrap();
    activate(child, context);
    let outside = fs::file_syscall(23, input(b"/FOREIGN/SECRET\0"), 2, 0);
    assert_eq!(outside, 3);
    let home = fs::file_syscall(23, input(b"/GRANTHOME/DATA\0"), 2, 0);
    assert_eq!(home, 4);
    let public = fs::file_syscall(23, input(b"/GRANTPUBLIC/PROBE.BIN\0"), 2, 0);
    assert_eq!(public, 5);
    scheduler::restrict_filesystem(&[
        ("/GRANTHOME", FileRights::READ_WRITE),
        ("/GRANTTMP", FileRights::WRITE),
        ("/GRANTPUBLIC", FileRights::READ),
    ])
    .unwrap();
    let output = self_rust_os::userspace::USER_STACK_TOP - 128 * 1024;
    assert!(self_rust_os::userspace::usercopy::to_user(
        output,
        &[0xa5; 16]
    ));
    assert_eq!(
        fs::file_read(outside, output, 3),
        u64::MAX - 1,
        "existing descriptors must not bypass narrowed directory grants"
    );
    let mut untouched = [0; 16];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output,
        &mut untouched
    ));
    assert_eq!(untouched, [0xa5; 16]);
    assert_eq!(fs::file_syscall(26, outside, 0, 2), u64::MAX - 1);
    assert_eq!(fs::file_syscall(25, outside, output, 3), u64::MAX - 1);
    assert_eq!(fs::file_read(public, output, 3), 3);
    assert_eq!(fs::file_syscall(25, public, output, 3), u64::MAX - 1);
    assert_eq!(
        fs::create_directory(input(b"/FOREIGN/new/sub\0")),
        u64::MAX - 1
    );
    assert_eq!(
        fs::create_directory(input(b"/GRANTPUBLIC/new\0")),
        u64::MAX - 1
    );
    assert_eq!(fs::create_directory(input(b"/GRANTHOME/new/sub\0")), 0);
    assert_eq!(fs::write_path(input(b"/GRANTTMP/WRITE\0"), output, 3), 3);
    assert_eq!(
        fs::read_path(input(b"/GRANTTMP/WRITE\0"), output, 3),
        u64::MAX - 1
    );

    assert_eq!(
        fs::file_syscall(23, input(b"/FOREIGN/SECRET\0"), 0, 0),
        u64::MAX - 1
    );
    assert_eq!(
        fs::file_syscall(23, input(b"/FOREIGN/../GRANTHOME/DATA\0"), 0, 0),
        u64::MAX - 1
    );
    assert_eq!(
        fs::read_path(input(b"/FOREIGN/SECRET\0"), output, 3),
        u64::MAX - 1
    );
    assert_eq!(
        fs::write_path(input(b"/FOREIGN/SECRET\0"), output, 3),
        u64::MAX - 1
    );
    assert_eq!(
        fs::read_directory(input(b"/\0"), output, 4096),
        u64::MAX - 1
    );
    assert_eq!(fs::list_directory(input(b"/\0"), output, 4), u64::MAX - 1);
    assert_eq!(
        fs::read_directory(input(b"/GRANTTMP\0"), output, 4096),
        u64::MAX - 1
    );
    assert!(scheduler::load_program_from_disk("/FOREIGN/SECRET").is_err());
    let binary = scheduler::load_program_from_disk("/GRANTPUBLIC/PROBE.BIN").unwrap();
    assert_eq!(&binary[..], b"abc");
    drop(binary);
    assert_eq!(fs::file_read(home, output, 3), 3);
    assert_eq!(fs::file_syscall(27, input(b"/GRANTHOME\0"), 0, 0), 0);
    assert_eq!(
        fs::file_syscall(27, input(b"/FOREIGN\0"), 0, 0),
        u64::MAX - 1
    );
    assert_eq!(
        fs::file_syscall(23, input(b"/GRANTPUBLIC/PROBE.BIN\0"), 1 | 0o1000, 0),
        u64::MAX - 1
    );
    assert!(scheduler::restrict_filesystem(&[("/", FileRights::READ)]).is_err());
    let grandchild = scheduler::spawn_process(&[0xc3], 4096).unwrap();
    activate(grandchild, context);
    assert_eq!(
        fs::read_path(input(b"/FOREIGN/SECRET\0"), output, 3),
        u64::MAX - 1
    );
    assert!(scheduler::restrict_filesystem(&[("/FOREIGN", FileRights::READ)]).is_err());
    assert_eq!(fs::read_path(input(b"/GRANTHOME/DATA\0"), output, 3), 3);
    scheduler::restrict_filesystem(&[]).unwrap();
    assert_eq!(
        fs::read_path(input(b"/GRANTHOME/DATA\0"), output, 3),
        u64::MAX - 1
    );
    activate(1, context);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    assert_eq!(scheduler::waitpid(grandchild), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
    let scope = scheduler::scope_create_service(SYSTEM, limits(0)).unwrap();
    let no_read =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, Capabilities::SPAWN)
            .unwrap();
    activate(no_read, context);
    assert!(
        scheduler::load_program_from_disk("/GRANTPUBLIC/PROBE.BIN").is_err(),
        "path loading requires FILE_READ as well as SPAWN"
    );
    assert!(scheduler::restrict_filesystem(&[("/GRANTPUBLIC", FileRights::READ)]).is_err());
    activate(1, context);
    scheduler::scope_close(scope).unwrap();
    assert_eq!(scheduler::waitpid(no_read), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
    fs::with_fs(|fs| {
        let mut data = [0; 3];
        fs.read_file("/FOREIGN/SECRET", &mut data).unwrap();
        assert_eq!(&data, b"abc");
        assert!(fs.check_directory("/FOREIGN/new").is_err());
        assert!(fs.check_directory("/GRANTPUBLIC/new").is_err());
        assert!(fs.check_directory("/GRANTHOME/new/sub").is_ok());
    })
    .unwrap();
}

fn initial_session_grants_publish_with_the_leader(context: &mut u64, initial: Usage) {
    use self_rust_os::fs::{self, FileRights};
    fs::with_fs(|fs| {
        fs.create_dir("/SESSIONHOME").unwrap();
        fs.create_dir("/SESSIONOUT").unwrap();
        fs.create_file("/SESSIONHOME/DATA").unwrap();
        fs.write_file("/SESSIONHOME/DATA", b"abc").unwrap();
        fs.create_file("/SESSIONOUT/SECRET").unwrap();
        fs.write_file("/SESSIONOUT/SECRET", b"out").unwrap();
    })
    .unwrap();
    use rustos_ipc_abi as ipc;
    let output = self_rust_os::userspace::USER_STACK_TOP - 128 * 1024;
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_PAIR, output, 16, 0), 0);
    let mut pair = [0; 16];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut pair
    ));
    let peer = u64::from_le_bytes(pair[..8].try_into().unwrap());
    let moved = u64::from_le_bytes(pair[8..].try_into().unwrap());
    let bindings = [ipc::Binding {
        source: ipc::Handle(moved),
        tag: 7,
        rights: ipc::Rights::ALL,
    }];
    let source = scheduler::scope_status(SYSTEM).unwrap().used;
    for deny_frames in [false, true] {
        let mut bad_limit = limits(0);
        bad_limit.ipc_handles = 1;
        if deny_frames {
            bad_limit.pages = 0;
        }
        let denied = scheduler::scope_create_session(1000, bad_limit).unwrap();
        let before = scheduler::frame_count();
        for _ in 0..3 {
            let root = if deny_frames {
                "/SESSIONHOME"
            } else {
                "/NO_SUCH_GRANT_ROOT"
            };
            assert!(scheduler::spawn_process_in_scope_with_grants(
                &[0xc3],
                4096,
                &[],
                &[],
                denied,
                Capabilities::FILE_READ,
                scheduler::InitialGrants {
                    bindings: &bindings,
                    filesystem: &[(root, FileRights::READ)]
                }
            )
            .is_err());
            let state = scheduler::scope_status(denied).unwrap();
            assert_eq!(state.used, Usage::ZERO);
            assert_eq!(state.leader, None);
            assert_eq!(scheduler::frame_count(), before);
            assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, source);
            assert_eq!(
                scheduler::ipc_syscall(ipc::SYS_INTEREST, moved, ipc::INTEREST_READ, 0),
                0,
                "failed initial grants must retain source endpoint ownership"
            );
        }
        scheduler::scope_close(denied).unwrap();
        scheduler::scope_ack_closed(denied).unwrap();
    }
    let mut limit = limits(4);
    limit.processes = 2;
    limit.threads = 2;
    limit.pages = 4096;
    limit.ipc_handles = 1;
    let session = scheduler::scope_create_session(1000, limit).unwrap();
    let caps = Capabilities::from_bits(
        Capabilities::FILE_READ.bits()
            | Capabilities::FILE_WRITE.bits()
            | Capabilities::SPAWN.bits()
            | Capabilities::SCOPE_CREATE.bits(),
    )
    .unwrap();
    let leader = scheduler::spawn_process_in_scope_with_grants(
        &[0xc3],
        4096,
        &[],
        &[],
        session,
        caps,
        scheduler::InitialGrants {
            bindings: &bindings,
            filesystem: &[("/SESSIONHOME", FileRights::READ_WRITE)],
        },
    )
    .unwrap();
    assert_eq!(
        scheduler::scope_status(session)
            .unwrap()
            .leader
            .unwrap()
            .pid,
        leader
    );
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_CLOSE, moved, 0, 0),
        u64::MAX - 1,
        "publication moves the endpoint and filesystem authority together"
    );
    activate(leader, context);
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_BOOTSTRAP, 7, 0, 0), moved);

    let output = self_rust_os::userspace::USER_STACK_TOP - 128 * 1024;
    assert_eq!(
        fs::read_path(input(b"/SESSIONHOME/DATA\0"), output, 3),
        3,
        "the first filesystem operation must see the approved initial session grants"
    );
    assert_eq!(
        fs::read_path(input(b"/SESSIONOUT/SECRET\0"), output, 3),
        u64::MAX - 1
    );
    let child_caps =
        Capabilities::from_bits(Capabilities::FILE_READ.bits() | Capabilities::FILE_WRITE.bits())
            .unwrap();
    let service = scheduler::scope_create_service(session, limits(2)).unwrap();
    assert!(scheduler::spawn_process_in_scope_with_grants(
        &[0xc3],
        4096,
        &[],
        &[],
        service,
        child_caps,
        scheduler::InitialGrants {
            bindings: &[],
            filesystem: &[("/SESSIONOUT", FileRights::READ)]
        }
    )
    .is_err());
    let child = scheduler::spawn_process_in_scope_with_grants(
        &[0xc3],
        4096,
        &[],
        &[],
        service,
        child_caps,
        scheduler::InitialGrants {
            bindings: &[],
            filesystem: &[("/SESSIONHOME", FileRights::READ)],
        },
    )
    .unwrap();
    activate(child, context);
    assert_eq!(fs::read_path(input(b"/SESSIONHOME/DATA\0"), output, 3), 3);
    assert_eq!(
        fs::write_path(input(b"/SESSIONHOME/DATA\0"), output, 3),
        u64::MAX - 1
    );
    assert_eq!(
        fs::read_path(input(b"/SESSIONOUT/SECRET\0"), output, 3),
        u64::MAX - 1
    );
    activate(1, context);
    scheduler::scope_close(session).unwrap();
    assert_eq!(scheduler::waitpid(leader), 9);
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(service).unwrap();
    scheduler::scope_ack_closed(session).unwrap();
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_CLOSE, peer, 0, 0), 0);
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
    // Explicit empty grants must override same-session inherited system access.
    let empty = scheduler::scope_create_service(SYSTEM, limits(0)).unwrap();
    assert!(
        scheduler::spawn_process_in_scope_with_grants(
            &[0xc3],
            4096,
            &[],
            &[],
            empty,
            Capabilities::NONE,
            scheduler::InitialGrants {
                bindings: &[],
                filesystem: &[("/SESSIONHOME", FileRights::READ)]
            }
        )
        .is_err(),
        "initial object rights must fit target mechanism rights"
    );
    assert_eq!(scheduler::scope_status(empty).unwrap().leader, None);
    assert_eq!(scheduler::scope_status(empty).unwrap().used, Usage::ZERO);
    let no_files = scheduler::spawn_process_in_scope_with_grants(
        &[0xc3],
        4096,
        &[],
        &[],
        empty,
        Capabilities::FILE_READ,
        scheduler::InitialGrants {
            bindings: &[],
            filesystem: &[],
        },
    )
    .unwrap();
    activate(no_files, context);
    assert_eq!(
        fs::read_path(input(b"/SESSIONHOME/DATA\0"), output, 3),
        u64::MAX - 1
    );
    activate(1, context);
    scheduler::scope_close(empty).unwrap();
    assert_eq!(scheduler::waitpid(no_files), 9);
    scheduler::reap_retired();
    scheduler::scope_ack_closed(empty).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial);
}
