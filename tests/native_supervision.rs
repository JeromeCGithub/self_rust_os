//! Native PID 1 parent authority and durable orphan completion ownership.
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

/// Native init owns orphan notifications and cannot be controlled by descendants.
#[test_case]
fn native_ownership_and_orphans() {
    use rustos_process_abi::supervision::{ChildEvent, ProcessKey};
    let init = scheduler::spawn_process(&[0xc3], 4096).unwrap();
    assert_eq!(init, 1);
    assert_eq!(
        scheduler::surface_status(init),
        0,
        "native PID 1 owns private RAM, not the display MMIO mapping"
    );
    let mut context = scheduler::schedule(0);
    let parent = scheduler::spawn_process(&[0xc3], 0).unwrap();
    activate(parent, &mut context);
    assert_eq!(scheduler::kill(init), None, "child cannot kill PID 1");
    assert_eq!(
        scheduler::waitpid(init),
        u64::MAX - 1,
        "child cannot reap PID 1"
    );
    let orphan = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let zombie = scheduler::spawn_process(&[0xc3], 0).unwrap();
    scheduler::kill(zombie).unwrap();
    activate(init, &mut context);
    assert_eq!(scheduler::kill(orphan), None, "grandchild is not yet owned");
    scheduler::kill(parent).unwrap();
    let output = 0x3ffff_ff00;
    let mut seen_live = false;
    let mut seen_zombie = false;
    let mut seen_parent = false;
    for _ in 0..3 {
        assert_eq!(scheduler::wait_events(32, 1), 32);
        assert_eq!(scheduler::child_next_user(output, 40, 0), 1);
        let mut bytes = [0; 40];
        assert!(self_rust_os::userspace::usercopy::from_user(
            output, &mut bytes
        ));
        match ChildEvent::decode(&bytes).unwrap() {
            ChildEvent::Adopted(key) => {
                assert_eq!(key.pid, orphan);
                seen_live = true;
            }
            ChildEvent::Completed { process, .. } if process.pid == zombie => seen_zombie = true,
            ChildEvent::Completed { process, .. } if process.pid == parent => seen_parent = true,
            other => panic!("unexpected event {:?}", other),
        }
    }
    assert!(seen_live && seen_zombie && seen_parent);
    assert_eq!(scheduler::child_next_user(output, 40, 0), 0);
    assert_eq!(
        scheduler::terminate_child(
            ProcessKey {
                pid: orphan,
                generation: 1
            }
            .encode(),
            0,
            0
        ),
        0
    );
    assert_eq!(scheduler::child_next_user(output, 40, 0), 1);
    assert_eq!(scheduler::child_next_user(output, 40, 0), 0);
    connected_completions(&mut context);
    assert!(
        scheduler::configure_native_init().is_err(),
        "profile cannot change after publication"
    );
}

fn connected_completions(context: &mut u64) {
    use rustos_process_abi::supervision::{Cause, ChildEvent};
    let host = scheduler::spawn_process(&[0xc3], 0).unwrap();
    activate(host, context);
    let worker = scheduler::spawn_process(&[0xc3], 0).unwrap();
    assert!(scheduler::connect_stdio(worker));
    activate(1, context);
    scheduler::kill(host).unwrap();
    let output = 0x3ffff_ff00;
    let mut seen_host = false;
    let mut seen_worker = false;
    for _ in 0..2 {
        assert_eq!(
            scheduler::child_next_user(output, 40, 0),
            1,
            "reaping a connected host cannot silently discard adopted completion"
        );
        let mut bytes = [0; 40];
        assert!(self_rust_os::userspace::usercopy::from_user(
            output, &mut bytes
        ));
        match ChildEvent::decode(&bytes).unwrap() {
            ChildEvent::Completed {
                process,
                cause: Cause::Killed,
                status: 9,
            } => {
                if process.pid == host {
                    seen_host = true;
                } else {
                    assert_eq!(process.pid, worker);
                    seen_worker = true;
                }
            }
            other => panic!("unexpected {:?}", other),
        }
    }
    assert!(seen_host && seen_worker);
    assert_eq!(scheduler::child_next_user(output, 40, 0), 0);
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
