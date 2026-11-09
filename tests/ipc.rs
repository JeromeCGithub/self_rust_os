//! Owned child readiness, consumption, process identities and termination causes.
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
    test_main();
    self_rust_os::hlt_loop();
}

/// Queue publication, usercopy, durable readiness and endpoint ownership.
#[test_case]
fn native_channel_roundtrip_and_retained_copy_failure() {
    use rustos_ipc_abi as ipc;
    scheduler::spawn_process(&[0xc3], 4096).expect("initial owner");
    let mut context = scheduler::schedule(0);
    let output = self_rust_os::userspace::USER_STACK_TOP - 4096;
    for (pointer, size, reserved) in [
        (0, 16, 0),
        (0x600000, 16, 0),
        (u64::MAX - 8, 16, 0),
        (0x400000, 16, 0),
        (output, 15, 0),
        (output, 16, 1),
    ] {
        assert_eq!(
            scheduler::ipc_syscall(ipc::SYS_PAIR, pointer, size, reserved),
            u64::MAX - 1
        );
    }
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_PAIR, output, 16, 0),
        0,
        "owned channel pair must be created"
    );
    let mut pair = [0; 16];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut pair
    ));
    let a = u64::from_le_bytes(pair[..8].try_into().unwrap());
    let b = u64::from_le_bytes(pair[8..].try_into().unwrap());
    assert_ne!(a, b);
    assert!(self_rust_os::userspace::usercopy::to_user(
        output + 512,
        b"ready"
    ));
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_SEND, a, output + 512, 5), 0);
    assert_eq!(
        scheduler::wait_events(64, 0),
        64,
        "queued IPC must be durable readiness"
    );
    for (pointer, size, reserved) in [
        (0, ipc::EVENT_SIZE as u64, 0),
        (0x400000, ipc::EVENT_SIZE as u64, 0),
        (output, ipc::EVENT_SIZE as u64 - 1, 0),
        (output, ipc::EVENT_SIZE as u64, 1),
    ] {
        assert_eq!(
            scheduler::ipc_syscall(ipc::SYS_NEXT, pointer, size, reserved),
            u64::MAX - 1
        );
    }
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_NEXT, output, ipc::EVENT_SIZE as u64, 0),
        1
    );
    let mut bytes = [0; ipc::EVENT_SIZE];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    let event = ipc::Event::decode(&bytes).unwrap();
    assert_eq!(event.endpoint, ipc::Handle(b));
    assert_eq!(&event.payload[..5], b"ready");
    assert_eq!(
        event.sender,
        Some(ipc::ProcessKey {
            pid: 0,
            generation: 1
        })
    );
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_NEXT, output, ipc::EVENT_SIZE as u64, 0),
        0
    );
    let intruder = scheduler::spawn_process(&[0xc3], 0).unwrap();
    activate(intruder, &mut context);
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_SEND, a, output + 512, 5),
        u64::MAX - 1
    );
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_CLOSE, b, 0, 0),
        u64::MAX - 1
    );
    activate(0, &mut context);
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_CLOSE, a, 0, 0), 0);
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_NEXT, output, ipc::EVENT_SIZE as u64, 0),
        1
    );
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    assert_eq!(
        ipc::Event::decode(&bytes).unwrap().kind,
        ipc::Kind::PeerClosed
    );
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_NEXT, output, ipc::EVENT_SIZE as u64, 0),
        0
    );
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_CLOSE, b, 0, 0), 0);
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_SEND, a, output + 512, 5),
        u64::MAX - 1
    );
    scheduler::kill(intruder).unwrap();
    assert_eq!(scheduler::waitpid(intruder), 9);
    binding_wake_and_exit(&mut context);
}
fn binding_wake_and_exit(context: &mut u64) {
    use rustos_ipc_abi as ipc;
    scheduler::reap_retired();
    let output = self_rust_os::userspace::USER_STACK_TOP - 4096;
    let baseline = scheduler::frame_count();
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_PAIR, output, 16, 0), 0);
    let mut pair = [0; 16];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut pair
    ));
    let a = u64::from_le_bytes(pair[..8].try_into().unwrap());
    let b = u64::from_le_bytes(pair[8..].try_into().unwrap());
    let binding = ipc::Binding {
        source: ipc::Handle(b),
        tag: 7,
        rights: ipc::Rights::SEND,
    };
    let invalid = [binding, ipc::Binding { tag: 8, ..binding }];
    assert!(scheduler::spawn_process_with_bindings(&[0xc3], 0, &[], &[], &invalid).is_err());
    assert_eq!(
        scheduler::frame_count(),
        baseline,
        "failed binding must not retain frames"
    );
    assert_eq!(
        scheduler::child_next_user(output, 40, 0),
        0,
        "failed binding must not publish a zombie"
    );
    let child = scheduler::spawn_process_with_bindings(&[0xc3], 0, &[], &[], &[binding])
        .expect("explicit bootstrap binding must be published atomically with child");
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_CLOSE, b, 0, 0),
        u64::MAX - 1,
        "parent alias loses authority"
    );
    assert_eq!(
        scheduler::wait_events(64, 0),
        scheduler::PROCESS_BLOCK_SENTINEL
    );
    *context = scheduler::schedule(*context);
    activate(child, context);
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_BOOTSTRAP, 7, 0, 0), b);
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_BOOTSTRAP, 7, 0, 0),
        u64::MAX - 1,
        "bootstrap grant must be taken exactly once for owned SDK handles"
    );
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_BOOTSTRAP, 8, 0, 0),
        u64::MAX - 1
    );
    assert!(
        scheduler::spawn_process_with_bindings(&[0xc3], 0, &[], &[], &[binding]).is_err(),
        "send-only endpoint cannot be delegated"
    );
    assert!(self_rust_os::userspace::usercopy::to_user(
        output + 512,
        b"native-ready"
    ));
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_SEND, b, output + 512, 12),
        0
    );
    activate(0, context); // A blocked parent can run only if the send woke it.
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_NEXT, output, ipc::EVENT_SIZE as u64, 0),
        1
    );
    let mut bytes = [0; ipc::EVENT_SIZE];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    let event = ipc::Event::decode(&bytes).unwrap();
    assert_eq!(
        event.sender,
        Some(ipc::ProcessKey {
            pid: child,
            generation: 1
        })
    );
    assert_eq!(&event.payload[..12], b"native-ready");
    assert_eq!(
        scheduler::wait_events(64, 0),
        scheduler::PROCESS_BLOCK_SENTINEL
    );
    *context = scheduler::schedule(*context);
    activate(child, context);
    scheduler::exit_current(0);
    *context = scheduler::schedule(0);
    activate(0, context);
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_NEXT, output, ipc::EVENT_SIZE as u64, 0),
        1,
        "owner exit must wake peer"
    );
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    assert_eq!(
        ipc::Event::decode(&bytes).unwrap().kind,
        ipc::Kind::PeerClosed
    );
    assert_eq!(
        scheduler::ipc_syscall(ipc::SYS_SEND, a, output + 512, 12),
        ipc::SEND_CLOSED
    );
    assert_eq!(scheduler::ipc_syscall(ipc::SYS_CLOSE, a, 0, 0), 0);
    assert_eq!(scheduler::waitpid(child), 0);
    scheduler::reap_retired();
    assert_eq!(
        scheduler::frame_count(),
        baseline,
        "bound child exit must reclaim all private frames"
    );
}
fn activate(pid: u32, context: &mut u64) {
    for _ in 0..128 {
        if scheduler::current_pid() == Some(pid) {
            return;
        }
        let next = scheduler::on_timer(*context);
        if next != 0 {
            *context = next;
        }
    }
    panic!("process not scheduled");
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}
