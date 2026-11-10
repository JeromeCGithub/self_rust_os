//! Kernel-side native scope integration; no Ring 3 scope ABI is exposed yet.
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
    test_main();
    self_rust_os::hlt_loop();
}

use rustos_process_abi::principal::Capabilities as Caps;
use rustos_scope_core::{Error, State, Usage, SYSTEM};
fn limits(processes: u64, threads: u64, pages: u64) -> Usage {
    Usage {
        processes,
        threads,
        pages,
        kernel_bytes: 1024 * 1024,
        ..Usage::ZERO
    }
}
fn caps() -> Caps {
    Caps::from_bits(
        Caps::FILE_READ.bits()
            | Caps::FILE_WRITE.bits()
            | Caps::SPAWN.bits()
            | Caps::THREADS.bits()
            | Caps::IPC.bits(),
    )
    .unwrap()
}
/// Scope reservations follow native allocation, authority and real reclamation.
#[test_case]
fn native_scope_allocation_lifecycle_and_identity() {
    assert_eq!(scheduler::spawn_process(&[0xc3], 4096), Ok(1));
    let mut context = scheduler::schedule(0);
    let initial =
        scheduler::scope_status(SYSTEM).expect("native scope registry must be bootstrapped");
    assert_eq!(initial.used.processes, 1);
    assert_eq!(initial.used.threads, 1);
    assert_eq!(initial.used.pages, scheduler::frame_count());
    assert!(initial.used.kernel_bytes >= 16384);
    failed_constructor_rolls_back(&initial);
    scoped_process_heap_and_threads(&mut context, &initial);
    scoped_tls_and_thread_page_limits(&mut context, &initial);
    session_ownership_and_orphans(&mut context, &initial);
}
fn failed_constructor_rolls_back(initial: &rustos_scope_core::Snapshot) {
    let mut base_bytes = limits(1, 1, 4096);
    base_bytes.kernel_bytes = 0;
    let mut late_bytes = limits(1, 1, 4096);
    late_bytes.kernel_bytes = 16385;
    for limit in [limits(1, 1, 1), base_bytes, late_bytes] {
        let scope = scheduler::scope_create_service(SYSTEM, limit).unwrap();
        for _ in 0..3 {
            assert!(
                scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps()).is_err()
            );
            let status = scheduler::scope_status(scope).unwrap();
            assert_eq!(status.used, Usage::ZERO);
            assert_eq!(status.leader, None);
            assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial.used);
            assert_eq!(scheduler::frame_count(), initial.used.pages);
        }
        scheduler::scope_close(scope).unwrap();
        assert_eq!(scheduler::scope_next_closed().unwrap().unwrap().id, scope);
        scheduler::scope_ack_closed(scope).unwrap();
    }
}
fn scoped_process_heap_and_threads(context: &mut u64, initial: &rustos_scope_core::Snapshot) {
    let scope = scheduler::scope_create_service(SYSTEM, limits(1, 2, 4096)).unwrap();
    let child = scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], scope, caps()).unwrap();
    let allocated = scheduler::scope_status(scope).unwrap().used;
    assert_eq!(allocated.processes, 1);
    assert_eq!(allocated.threads, 1);
    assert!(allocated.pages > 1024);
    assert_eq!(
        scheduler::frame_count(),
        initial.used.pages + allocated.pages
    );
    activate(child, context);
    let main = scheduler::current_tid().unwrap();
    assert!(
        scheduler::spawn_process(&[0xc3], 4096).is_err(),
        "ordinary spawn cannot escape the process quota"
    );
    assert_eq!(scheduler::scope_status(scope), Err(Error::Denied));
    let heap = rustos_thread_abi::HEAP_BASE;
    let before = scheduler::frame_count();
    let room = 4096 - allocated.pages;
    assert_eq!(
        scheduler::program_break(heap + room * 4096),
        heap,
        "parent-table allocations count toward quota"
    );
    assert_eq!(
        scheduler::frame_count(),
        before,
        "failed growth removes leaves and parents"
    );
    assert_eq!(scheduler::program_break(heap + 4096), heap + 4096);
    let grown = scheduler::frame_count();
    assert!(grown >= before + 1);
    assert_eq!(scheduler::program_break(heap), heap);
    let retained = scheduler::frame_count();
    assert_eq!(
        retained,
        grown - 1,
        "empty parent tables remain owned until process teardown"
    );
    for _ in 0..3 {
        let request = rustos_thread_abi::Create {
            version: 1,
            flags: 0,
            entry: 0x400000,
            argument: 0,
            stack_size: 16384,
            affinity: 0,
        };
        let tid = scheduler::create_thread(request);
        assert!(tid < u64::MAX - 10);
        let with_thread = scheduler::frame_count();
        assert!(with_thread > retained);
        assert_eq!(
            scheduler::create_thread(request),
            u64::MAX - 1,
            "thread budget counts the base thread"
        );
        assert_eq!(scheduler::frame_count(), with_thread);
        for _ in 0..64 {
            if scheduler::current_tid() == Some(tid) {
                break;
            }
            let next = scheduler::on_timer(*context);
            if next != 0 {
                *context = next;
            }
        }
        assert_eq!(scheduler::current_tid(), Some(tid));
        scheduler::exit_thread();
        for _ in 0..64 {
            let next = scheduler::on_timer(*context);
            if next != 0 {
                *context = next;
            }
            if scheduler::current_tid() == Some(main) {
                break;
            }
        }
        assert_eq!(scheduler::current_tid(), Some(main));
        assert_eq!(scheduler::join_thread(tid), 0);
        assert_eq!(
            scheduler::create_thread(request),
            u64::MAX - 1,
            "join status alone does not release a stack charge"
        );
        scheduler::reap_retired();
        assert_eq!(scheduler::frame_count(), retained);
    }
    activate(1, context);
    let status = scheduler::scope_status(scope).unwrap();
    assert_eq!(status.used.threads, 1);
    assert_eq!(
        status.used.pages,
        scheduler::frame_count() - initial.used.pages
    );
    scheduler::kill(child).unwrap();
    assert_eq!(
        scheduler::scope_status(scope).unwrap().state,
        State::Closing
    );
    assert!(scheduler::scope_next_closed().unwrap().is_none());
    assert_eq!(scheduler::waitpid(child), 9);
    assert!(
        scheduler::scope_next_closed().unwrap().is_none(),
        "process status does not imply physical reclamation"
    );
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_next_closed().unwrap().unwrap().id, scope);
    scheduler::scope_ack_closed(scope).unwrap();
    assert_eq!(scheduler::frame_count(), initial.used.pages);
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial.used);
}
fn tls_image(memory: u64) -> alloc::vec::Vec<u8> {
    let mut bytes = alloc::vec![0;0x2008];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    for (offset, value) in [(16, 2u16), (18, 62), (52, 64), (54, 56), (56, 3)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    for (index, kind, address, offset, flags, size, mem, alignment) in [
        (0, 1u32, 0x400000u64, 0x1000u64, 5u32, 1u64, 1u64, 4096u64),
        (1, 1, 0x401000, 0x2000, 6, 8, memory, 4096),
        (2, 7, 0x401000, 0x2000, 4, 8, memory, 16),
    ] {
        let p = 64 + index * 56;
        bytes[p..p + 4].copy_from_slice(&kind.to_le_bytes());
        bytes[p + 4..p + 8].copy_from_slice(&flags.to_le_bytes());
        for (field, value) in [
            (8, offset),
            (16, address),
            (32, size),
            (40, mem),
            (48, alignment),
        ] {
            bytes[p + field..p + field + 8].copy_from_slice(&value.to_le_bytes());
        }
    }
    bytes[0x1000] = 0xc3;
    bytes[0x2000..].copy_from_slice(b"tls-data");
    bytes
}
fn scoped_tls_and_thread_page_limits(context: &mut u64, initial: &rustos_scope_core::Snapshot) {
    use self_rust_os::userspace::usercopy;
    for memory in [8192u64, 1024 * 1024] {
        let image = tls_image(memory);
        let tls_pages = (memory + 16).div_ceil(4096);
        let scope = scheduler::scope_create_service(SYSTEM, limits(1, 2, 4096)).unwrap();
        let child = scheduler::spawn_process_in_scope(&image, 0, &[], &[], scope, caps()).unwrap();
        let start = scheduler::scope_status(scope).unwrap().used;
        activate(child, context);
        let main = scheduler::current_tid().unwrap();
        let base = 0x4_4000_0000 + 4096;
        let mut data = [0; 8];
        assert!(usercopy::from_user(base, &mut data));
        assert_eq!(&data, b"tls-data");
        assert!(usercopy::to_user(base, b"changed!"));
        let request = rustos_thread_abi::Create {
            version: 1,
            flags: 0,
            entry: 0x400000,
            argument: 0,
            stack_size: 16384,
            affinity: 0,
        };
        let before = scheduler::frame_count();
        let tid = scheduler::create_thread(request);
        assert!(tid < u64::MAX - 10);
        assert_eq!(
            scheduler::frame_count() - before,
            4 + tls_pages,
            "TLS frames must be charged with stack leaves"
        );
        let worker_base = base + 2 * 1024 * 1024;
        assert!(usercopy::from_user(worker_base, &mut data));
        assert_eq!(&data, b"tls-data");
        for _ in 0..64 {
            if scheduler::current_tid() == Some(tid) {
                break;
            }
            let next = scheduler::on_timer(*context);
            if next != 0 {
                *context = next;
            }
        }
        assert_eq!(scheduler::current_tid(), Some(tid));
        scheduler::exit_thread();
        for _ in 0..64 {
            let next = scheduler::on_timer(*context);
            if next != 0 {
                *context = next;
            }
            if scheduler::current_tid() == Some(main) {
                break;
            }
        }
        assert_eq!(scheduler::join_thread(tid), 0);
        scheduler::reap_retired();
        assert_eq!(scheduler::frame_count(), before);
        activate(1, context);
        assert_eq!(scheduler::scope_status(scope).unwrap().used, start);
        scheduler::scope_close(scope).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        scheduler::scope_ack_closed(scope).unwrap();
        let tight = scheduler::scope_create_service(SYSTEM, limits(1, 2, start.pages + 4)).unwrap();
        let child = scheduler::spawn_process_in_scope(&image, 0, &[], &[], tight, caps()).unwrap();
        activate(child, context);
        let before = scheduler::frame_count();
        assert_eq!(
            scheduler::create_thread(request),
            u64::MAX - 1,
            "TLS quota cannot be bypassed with a smaller stack"
        );
        assert_eq!(scheduler::frame_count(), before);
        activate(1, context);
        scheduler::scope_close(tight).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        scheduler::scope_ack_closed(tight).unwrap();
        assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial.used);
        assert_eq!(scheduler::frame_count(), initial.used.pages);
    }
}
fn session_ownership_and_orphans(context: &mut u64, initial: &rustos_scope_core::Snapshot) {
    let session = scheduler::scope_create_session(1, limits(4, 8, 16384)).unwrap();
    let factory = Caps::from_bits(caps().bits() | Caps::SCOPE_CREATE.bits()).unwrap();
    assert!(
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], session, Caps::ALL).is_err(),
        "user identity cannot gain system writes"
    );
    let manager =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], session, factory).unwrap();
    let other_session = scheduler::scope_create_session(1, limits(1, 2, 4096)).unwrap();
    let other_manager =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], other_session, factory).unwrap();
    activate(other_manager, context);
    assert_eq!(
        scheduler::scope_create_service(session, Usage::ZERO),
        Err(Error::Denied)
    );
    activate(manager, context);
    assert_eq!(
        scheduler::scope_create_service(other_session, Usage::ZERO),
        Err(Error::Denied)
    );
    assert_eq!(scheduler::scope_close(other_session), Err(Error::Denied));
    let output = 0x3ffff_ff00;
    assert_eq!(scheduler::principal_user(output, 40, 0), 0);
    let mut bytes = [0; 40];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    let principal = rustos_process_abi::principal::Principal::decode(&bytes).unwrap();
    assert_eq!(
        (
            principal.user,
            principal.session,
            principal.scope,
            principal.flags
        ),
        (1, session, session, 0)
    );
    assert_eq!(principal.capabilities, factory);
    assert_eq!(scheduler::scope_close(session), Err(Error::Denied));
    session_record_admission(context, session, manager, other_session, other_manager);
    let service = scheduler::scope_create_service(session, limits(2, 2, 8192)).unwrap();
    let leader =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], service, caps()).unwrap();
    activate(leader, context);
    let descendant = scheduler::spawn_process(&[0xc3], 4096).unwrap();
    assert_eq!(
        scheduler::scope_create_service(session, Usage::ZERO),
        Err(Error::Denied)
    );
    activate(manager, context);
    scheduler::kill(leader).unwrap();
    // Both completion statuses belong to the live owning manager, before init.
    assert_eq!(scheduler::waitpid(descendant), 9);
    assert_eq!(scheduler::waitpid(leader), 9);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_next_closed().unwrap().unwrap().id, service);
    scheduler::scope_ack_closed(service).unwrap();
    let service = scheduler::scope_create_service(session, limits(2, 2, 8192)).unwrap();
    let leader =
        scheduler::spawn_process_in_scope(&[0xc3], 4096, &[], &[], service, caps()).unwrap();
    activate(1, context);
    scheduler::kill(manager).unwrap();
    assert_eq!(scheduler::waitpid(leader), 9);
    assert_eq!(scheduler::waitpid(manager), 9);
    scheduler::reap_retired();
    assert_eq!(scheduler::scope_next_closed().unwrap().unwrap().id, service);
    scheduler::scope_ack_closed(service).unwrap();
    assert_eq!(scheduler::scope_next_closed().unwrap().unwrap().id, session);
    scheduler::scope_ack_closed(session).unwrap();
    activate(other_manager, context);
    assert_eq!(
        scheduler::current_pid(),
        Some(other_manager),
        "other session survives manager death"
    );
    activate(1, context);
    scheduler::scope_close(other_session).unwrap();
    assert_eq!(scheduler::waitpid(other_manager), 9);
    scheduler::reap_retired();
    assert_eq!(
        scheduler::scope_next_closed().unwrap().unwrap().id,
        other_session
    );
    scheduler::scope_ack_closed(other_session).unwrap();
    assert_eq!(scheduler::scope_status(SYSTEM).unwrap().used, initial.used);
    assert_eq!(scheduler::frame_count(), initial.used.pages);
}
fn session_record_admission(
    context: &mut u64,
    session: u64,
    manager: u32,
    other_session: u64,
    other_manager: u32,
) {
    let mut leaves = [0; rustos_scope_core::MAX_SESSION_SERVICES];
    for id in &mut leaves {
        *id = scheduler::scope_create_service(session, Usage::ZERO).unwrap();
    }
    assert_eq!(
        scheduler::scope_create_service(session, Usage::ZERO),
        Err(Error::Limit)
    );
    scheduler::scope_close(leaves[0]).unwrap();
    assert_eq!(
        scheduler::scope_create_service(session, Usage::ZERO),
        Err(Error::Limit),
        "closed records retain admission capacity until acknowledgement"
    );
    activate(other_manager, context);
    let sibling = scheduler::scope_create_service(other_session, Usage::ZERO)
        .expect("one session must leave record capacity for another");
    scheduler::scope_close(sibling).unwrap();
    scheduler::scope_ack_closed(sibling).unwrap();
    activate(manager, context);
    scheduler::scope_ack_closed(leaves[0]).unwrap();
    leaves[0] = scheduler::scope_create_service(session, Usage::ZERO).unwrap();
    assert_eq!(
        scheduler::scope_create_service(session, Usage::ZERO),
        Err(Error::Limit)
    );
    for id in leaves {
        scheduler::scope_close(id).unwrap();
        scheduler::scope_ack_closed(id).unwrap();
    }
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
