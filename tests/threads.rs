//! Native thread identities, process-private waits and resource reclamation.
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

/// Process-private TIDs, word waits and frame transactions preserve ownership.
#[test_case]
fn ownership_and_wait_boundaries() {
    use self_rust_os::memory::owned::FrameTransaction;
    use x86_64::structures::paging::FrameAllocator;
    static SMALL: [memory::MemRegion; 1] = [memory::MemRegion {
        start: 0x1000,
        end: 0x3000,
    }];
    // SAFETY: This local allocator only exercises scalar frame identities; no
    // physical memory is dereferenced and no real allocator shares this state.
    let mut pool = unsafe { memory::RegionFrameAllocator::new(&SMALL) };
    pool.enable_recycling();
    {
        let mut transaction = FrameTransaction::new(&mut pool);
        assert!(transaction.allocate_frame().is_some());
        assert!(transaction.allocate_frame().is_some());
        assert!(transaction.allocate_frame().is_none());
    }
    assert_eq!(
        pool.allocated(),
        0,
        "failed unpublished construction returns all frames"
    );
    let a = pool.allocate_frame().unwrap();
    let b = pool.allocate_frame().unwrap();
    assert_ne!(a, b);
    assert!(pool.allocate_frame().is_none());

    scheduler::spawn_process(&[0xc3], 0).unwrap();
    let mut context = scheduler::schedule(0);
    let parent = scheduler::current_tid().unwrap();
    let baseline = scheduler::frame_count();
    let child = scheduler::spawn_process(&[0xc3], 0).unwrap();
    assert_eq!(scheduler::wait_message(u64::MAX), u64::MAX - 1);
    assert_eq!(
        scheduler::wait_message(0),
        scheduler::PROCESS_BLOCK_SENTINEL
    );
    context = scheduler::schedule(context);
    assert_eq!(scheduler::current_pid(), Some(child));
    assert_eq!(scheduler::send_message(0, 0x4d424f58), 0);
    context = scheduler::on_timer(context);
    assert_eq!(scheduler::current_tid(), Some(parent));
    // SAFETY: The single-CPU scheduler selected this process's owned live
    // canonical frame. No context has been retired or concurrently restored.
    assert_eq!(
        unsafe { (*(context as *const scheduler::SavedContext)).rax },
        1
    );
    assert_eq!(
        scheduler::wait_message(0),
        1,
        "enqueue-before-park stays observable"
    );
    assert_eq!(
        scheduler::receive_message(),
        ((u64::from(child) + 1) << 32) | 0x4d424f58
    );
    activate_pid(child, &mut context);
    let foreign = scheduler::current_tid().unwrap();
    assert_eq!(
        scheduler::wait_word(0x400000, 0xc3, 0),
        scheduler::PROCESS_BLOCK_SENTINEL
    );
    scheduler::schedule(context);
    assert_eq!(scheduler::current_tid(), Some(parent));
    assert_eq!(
        scheduler::join_thread(foreign),
        u64::MAX - 1,
        "foreign thread join rejected"
    );
    assert_eq!(
        scheduler::detach_thread(foreign),
        u64::MAX - 1,
        "foreign detach rejected"
    );
    assert_eq!(
        scheduler::wake_word(0x400000, 1),
        0,
        "same virtual address in another PID is a different wait key"
    );
    assert!(scheduler::kill(child).is_some());
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    assert_eq!(
        scheduler::frame_count(),
        baseline,
        "full process frame tree is reclaimed"
    );
    assert_eq!(
        scheduler::join_thread(parent),
        u64::MAX - 1,
        "self join rejected"
    );
    assert_eq!(
        scheduler::join_thread(foreign),
        u64::MAX - 1,
        "reclaimed generation is stale"
    );
    assert!(scheduler::current_is_live());
    let heap_baseline = allocator::allocated_bytes();
    for _ in 0..8 {
        let child = scheduler::spawn_process(&[0xc3], 0).unwrap();
        scheduler::kill(child).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        assert_eq!(scheduler::frame_count(), baseline);
    }
    assert_eq!(
        allocator::allocated_bytes(),
        heap_baseline,
        "repeated process creation must reuse metadata slots"
    );
}
fn activate_pid(pid: u32, context: &mut u64) {
    for _ in 0..64 {
        if scheduler::current_pid() == Some(pid) {
            return;
        }
        let next = scheduler::on_timer(*context);
        if next != 0 {
            *context = next;
        }
    }
    panic!("process did not run");
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}
