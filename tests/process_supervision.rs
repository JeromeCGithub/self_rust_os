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

/// A live first child must not prevent observing any later child's exit.
#[test_case]
fn child_events_park_without_polling() {
    scheduler::spawn_process(&[0xc3], 4096).expect("initial owner");
    let mut context = scheduler::schedule(0);
    let first = scheduler::spawn_process(&[0xc3], 0).expect("first child");
    let second = scheduler::spawn_process(&[0xc3], 0).expect("second child");
    assert_ne!(first, second);
    assert_eq!(
        scheduler::wait_events(32, 0),
        scheduler::PROCESS_BLOCK_SENTINEL,
        "child-event wait must park while both children are live"
    );
    context = scheduler::schedule(context);
    activate(second, &mut context);
    scheduler::exit_current(23);
    context = scheduler::schedule(0);
    activate(0, &mut context);
    assert_eq!(
        scheduler::wait_events(32, 1),
        32,
        "second child's completion must wake owner"
    );
    assert_eq!(scheduler::waitpid(second), 23);
    consuming_events(&mut context);
    typed_termination(&mut context);
    scheduler::kill(first).unwrap();
    assert_eq!(scheduler::waitpid(first), 9);
}

fn consuming_events(context: &mut u64) {
    let child = scheduler::spawn_process(&[0xc3], 0).unwrap();
    activate(child, context);
    scheduler::exit_current(9);
    *context = scheduler::schedule(0);
    activate(0, context);
    let output = 0x3ffff_ff00;
    let error = u64::MAX - 1;
    for (pointer, size, reserved) in [
        (0, 40, 0),
        (0x600000, 40, 0),
        (u64::MAX - 10, 40, 0),
        (output, 39, 0),
        (output, 40, 1),
        (0x400000, 40, 0),
    ] {
        assert_eq!(scheduler::child_next_user(pointer, size, reserved), error);
    }
    assert_eq!(
        scheduler::child_next_user(output, 40, 0),
        1,
        "valid consume must retain completion through invalid output attempts"
    );
    let mut bytes = [0; 40];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), child);
    assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(bytes[24..32].try_into().unwrap()), 9);
    assert_eq!(scheduler::child_next_user(output, 40, 0), 0);
    assert_eq!(
        scheduler::waitpid(child),
        error,
        "completion is consumed once"
    );
}

fn typed_termination(context: &mut u64) {
    use rustos_process_abi::supervision::{Cause, ChildEvent, ProcessKey};
    let child = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let peer = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let identity = ProcessKey {
        pid: child,
        generation: 1,
    };
    let output = 0x3ffff_ff00;
    let error = u64::MAX - 1;
    activate(peer, context);
    assert_eq!(scheduler::terminate_child(identity.encode(), 0, 0), error);
    assert_eq!(scheduler::child_next_user(output, 40, 0), 0);
    activate(0, context);
    for (key, second, third) in [
        (child as u64, 0, 0),
        (identity.encode() + (1u64 << 32), 0, 0),
        (identity.encode(), 1, 0),
        (identity.encode(), 0, 1),
    ] {
        assert_eq!(scheduler::terminate_child(key, second, third), error);
    }
    assert_eq!(
        scheduler::terminate_child(identity.encode(), 0, 0),
        0,
        "owner may terminate exact live child identity"
    );
    assert_eq!(scheduler::child_next_user(output, 40, 0), 1);
    let mut bytes = [0; 40];
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    assert_eq!(
        ChildEvent::decode(&bytes),
        Some(ChildEvent::Completed {
            process: identity,
            cause: Cause::Killed,
            status: 9
        })
    );
    assert_eq!(scheduler::terminate_child(identity.encode(), 0, 0), error);
    activate(peer, context);
    scheduler::fault_current(14);
    *context = scheduler::schedule(0);
    activate(0, context);
    assert_eq!(scheduler::child_next_user(output, 40, 0), 1);
    assert!(self_rust_os::userspace::usercopy::from_user(
        output, &mut bytes
    ));
    assert_eq!(
        ChildEvent::decode(&bytes),
        Some(ChildEvent::Completed {
            process: ProcessKey {
                pid: peer,
                generation: 1
            },
            cause: Cause::Fault,
            status: 14
        })
    );
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
