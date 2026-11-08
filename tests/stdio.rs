//! Bounded connected standard-I/O endpoints and owned child cleanup.
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

/// Duplex endpoints isolate terminal traffic from compositor messages.
#[test_case]
fn stdio_checks() {
    scheduler::spawn_process(&[0xc3], 0).expect("display owner");
    let mut context = scheduler::schedule(0);
    let host = scheduler::spawn_process(&[0xc3], 0).expect("host");
    let child = scheduler::spawn_process(&[0xc3], 0).expect("shell");
    let stranger = scheduler::spawn_process(&[0xc3], 0).expect("stranger");
    activate(host, &mut context);
    assert!(
        scheduler::connect_stdio(child),
        "spawned shell must connect to host"
    );
    assert!(
        !scheduler::connect_stdio(stranger),
        "one owned connection per host"
    );
    assert_eq!(scheduler::stdio(0, 0), (u64::from(child) + 1) << 1);
    assert_eq!(scheduler::stdio(2, 0), 0);
    readiness_checks(host, child, &mut context);
    for word in 0u32..4096 {
        assert_eq!(scheduler::stdio(1, word), 0);
    }
    assert_eq!(
        scheduler::stdio(1, 99),
        1,
        "backpressure must preserve queued input"
    );
    activate(stranger, &mut context);
    assert_eq!(scheduler::stdio(0, 0), 0);
    assert_eq!(scheduler::stdio(1, 0), u64::MAX - 1);
    assert_eq!(scheduler::stdio(2, 0), u64::MAX - 1);
    activate(child, &mut context);
    assert_eq!(scheduler::stdio(0, 0), ((u64::from(host) + 1) << 1) | 1);
    for word in 0u32..4096 {
        assert_eq!(scheduler::stdio(2, 0), u64::from(word) + 1);
    }
    assert_eq!(scheduler::stdio(2, 0), 0);
    assert_eq!(scheduler::stdio(1, u32::MAX), 0);
    assert_eq!(scheduler::stdio(3, 0), u64::MAX - 1);
    activate(host, &mut context);
    assert_eq!(scheduler::stdio(2, 0), u64::from(u32::MAX) + 1);
    assert!(scheduler::kill(child).is_some());
    assert_eq!(
        scheduler::stdio(2, 0),
        u64::MAX - 1,
        "closed stream is not an empty live stream"
    );
    assert_eq!(scheduler::waitpid(child), 9);
    assert!(
        scheduler::connect_stdio(stranger),
        "reaping shell releases host connection"
    );
    activate(stranger, &mut context);
    assert_eq!(
        scheduler::kill(host),
        Some(scheduler::PROCESS_BLOCK_SENTINEL),
        "killing host cannot return into its stopped child"
    );
    context = scheduler::schedule(0);
    activate(0, &mut context);
    assert_eq!(
        scheduler::surface_status(stranger) & 1,
        1,
        "host death stops owned child"
    );
    assert_eq!(scheduler::waitpid(host), 9);
    assert_eq!(
        scheduler::surface_status(stranger),
        u64::MAX - 1,
        "reap owned child with host"
    );
    connected_waiter_checks(&mut context);
    foreign_waiter_checks(&mut context);
    readiness_eof_check(&mut context);
    nested_stdio_checks(&mut context);
    nested_exit_status_checks(&mut context);
}

fn readiness_eof_check(context: &mut u64) {
    let host = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let child = scheduler::spawn_process(&[0xc3], 0).unwrap();
    activate(host, context);
    assert!(scheduler::connect_stdio(child));
    assert_eq!(
        scheduler::wait_events(2, 0),
        scheduler::PROCESS_BLOCK_SENTINEL
    );
    *context = scheduler::schedule(*context);
    activate(child, context);
    scheduler::exit_current(0);
    *context = scheduler::schedule(0);
    activate(host, context);
    // SAFETY: This is the selected host's live, exclusively owned saved frame.
    assert_eq!(
        unsafe { (*(*context as *const scheduler::SavedContext)).rax },
        2,
        "peer exit wakes an empty reader for EOF without waiting for a tick"
    );
    assert_eq!(scheduler::stdio(2, 0), u64::MAX - 1);
    assert_eq!(scheduler::waitpid(child), 0);
    activate(0, context);
    scheduler::kill(host).unwrap();
    assert_eq!(scheduler::waitpid(host), 9);
}

// Exercise atomic check/park, wake return value and FIFO through real contexts.
fn readiness_checks(host: u32, child: u32, context: &mut u64) {
    const READ: u64 = 2;
    const WRITE: u64 = 4;
    let error = u64::MAX - 1;
    assert_eq!(
        scheduler::wait_events(16, 0),
        error,
        "pointer readiness belongs to the display owner"
    );
    for (mask, timeout) in [(0, 0), (32, 0), (READ, u64::MAX)] {
        assert_eq!(scheduler::wait_events(mask, timeout), error);
    }
    assert_eq!(scheduler::wait_events(WRITE, 0), WRITE);
    assert_eq!(
        scheduler::wait_events(READ, 0),
        scheduler::PROCESS_BLOCK_SENTINEL,
        "empty live stream must park rather than poll a timer"
    );
    *context = scheduler::schedule(*context);
    activate(child, context);
    assert_eq!(scheduler::stdio(1, 123), 0);
    activate(host, context);
    // SAFETY: activate selects this live thread's kernel-owned saved frame;
    // the supervisor harness does not restore or concurrently modify it.
    assert_eq!(
        unsafe { (*(*context as *const scheduler::SavedContext)).rax },
        READ,
        "enqueue must wake a reader immediately with its readiness bits"
    );
    assert_eq!(
        scheduler::wait_events(READ, 0),
        READ,
        "enqueue before wait must not lose a wake"
    );
    assert_eq!(scheduler::stdio(2, 0), 124);
    for word in 0..4096 {
        assert_eq!(scheduler::stdio(1, word), 0);
    }
    assert_eq!(
        scheduler::wait_events(WRITE, 0),
        scheduler::PROCESS_BLOCK_SENTINEL,
        "a full peer queue must park its writer"
    );
    *context = scheduler::schedule(*context);
    activate(child, context);
    assert_eq!(scheduler::stdio(2, 0), 1);
    activate(host, context);
    // SAFETY: Same CPU-owned saved frame invariant as the reader check above.
    assert_eq!(
        unsafe { (*(*context as *const scheduler::SavedContext)).rax },
        WRITE,
        "dequeue must immediately wake backpressured writers"
    );
    activate(child, context);
    for word in 1..4096 {
        assert_eq!(scheduler::stdio(2, 0), word + 1);
    }
    activate(host, context);
}

// Main completes waitpid by writing the exit code and reaping during wakeup.
fn connected_waiter_checks(context: &mut u64) {
    let host = scheduler::spawn_process(&[0xc3], 0).expect("waiting host");
    let child = scheduler::spawn_process(&[0xc3], 0).expect("waited child");
    activate(host, context);
    assert!(scheduler::connect_stdio(child));
    assert_eq!(scheduler::waitpid(child), scheduler::PROCESS_BLOCK_SENTINEL);
    *context = scheduler::schedule(*context);
    activate(child, context);
    scheduler::exit_current(23);
    *context = scheduler::schedule(0);
    activate(host, context);
    assert_eq!(
        scheduler::stdio(0, 0),
        0,
        "automatic waitpid reap must release the host endpoint"
    );
    // SAFETY: activate selected the live host's aligned SavedContext on its
    // owned kernel stack. This supervisor harness never restores user execution.
    assert_eq!(
        unsafe { (*(*context as *const scheduler::SavedContext)).rax },
        23
    );
    activate(0, context);
    assert!(scheduler::kill(host).is_some());
    assert_eq!(scheduler::waitpid(host), 9);
}

// A foreign waiter can reap the stopped caller during owner teardown.
fn foreign_waiter_checks(context: &mut u64) {
    let host = scheduler::spawn_process(&[0xc3], 0).expect("foreign-wait host");
    let child = scheduler::spawn_process(&[0xc3], 0).expect("foreign-wait child");
    activate(host, context);
    assert!(scheduler::connect_stdio(child));
    activate(0, context);
    assert_eq!(scheduler::waitpid(child), scheduler::PROCESS_BLOCK_SENTINEL);
    *context = scheduler::schedule(*context);
    activate(child, context);
    assert_eq!(
        scheduler::kill(host),
        Some(scheduler::PROCESS_BLOCK_SENTINEL),
        "auto-reaping the stopped caller must still schedule away"
    );
    *context = scheduler::schedule(0);
    activate(0, context);
    assert_eq!(scheduler::waitpid(host), 9);
    assert_eq!(scheduler::surface_status(child), u64::MAX - 1);
}

// Advance real contexts while remaining in the shared supervisor harness.
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

// An ordinary connected GUI application must own a worker without consuming its
// parent's input/output. Selectors identify only its already-owned child endpoint.
fn nested_stdio_checks(context: &mut u64) {
    let host = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let app = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let worker = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let stranger = scheduler::spawn_process(&[0xc3], 0).unwrap();
    activate(host, context);
    assert!(scheduler::connect_stdio(app));
    assert_eq!(scheduler::stdio(1, 11), 0);
    activate(app, context);
    assert!(
        scheduler::can_connect_stdio(),
        "a connected app can own a separate worker endpoint"
    );
    assert!(scheduler::connect_stdio(worker));
    assert_eq!(
        scheduler::restrict_worker(1, 65536, 0),
        u64::MAX - 1,
        "an app hosting a descendant cannot seal as a worker"
    );
    assert!(
        !scheduler::connect_stdio(stranger),
        "at most one owned child"
    );
    let select = u64::from(worker) + 1;
    assert_eq!(scheduler::stdio_selected(1, 22, select), 0);
    assert_eq!(
        scheduler::stdio(2, 0),
        12,
        "parent input remains independent"
    );
    assert_eq!(scheduler::stdio(1, 33), 0, "stdout still reaches parent");
    activate(worker, context);
    assert_eq!(scheduler::stdio(2, 0), 23);
    assert_eq!(scheduler::stdio(1, 44), 0);
    assert_eq!(
        scheduler::stdio_selected(2, 0, u64::from(app) + 1),
        u64::MAX - 1,
        "a parent is not an owned-child handle"
    );
    activate(stranger, context);
    assert_eq!(
        scheduler::stdio_selected(2, 0, select),
        u64::MAX - 1,
        "foreign selector rejected"
    );
    activate(app, context);
    assert_eq!(scheduler::stdio_selected(2, 0, select), 45);
    activate(worker, context);
    for value in 0..4096 {
        assert_eq!(scheduler::stdio(1, value), 0);
    }
    assert_eq!(scheduler::stdio(1, 99), 1);
    assert_eq!(
        scheduler::wait_events(4, 0),
        scheduler::PROCESS_BLOCK_SENTINEL,
        "nested full output queue parks writer"
    );
    *context = scheduler::schedule(*context);
    activate(app, context);
    assert_eq!(
        scheduler::stdio(2, 0),
        0,
        "worker output cannot become terminal input"
    );
    assert_eq!(scheduler::stdio_selected(2, 0, select), 1);
    activate(worker, context);
    // SAFETY: activate selects this live thread's kernel-owned saved frame;
    // the single-CPU supervisor fixture never concurrently restores it.
    assert_eq!(
        unsafe { (*(*context as *const scheduler::SavedContext)).rax },
        4,
        "draining nested queue wakes writer"
    );
    activate(app, context);
    for value in 1u64..4096 {
        assert_eq!(scheduler::stdio_selected(2, 0, select), value + 1);
    }
    assert!(scheduler::kill(worker).is_some());
    assert_eq!(scheduler::stdio_selected(2, 0, select), u64::MAX - 1);
    assert_eq!(scheduler::waitpid(worker), 9);
    assert!(
        scheduler::can_connect_stdio(),
        "reap releases only worker endpoint"
    );
    assert_eq!(scheduler::stdio(0, 0), ((u64::from(host) + 1) << 1) | 1);
    assert!(scheduler::connect_stdio(stranger));
    activate(host, context);
    assert_eq!(scheduler::stdio(2, 0), 34);
    activate(0, context);
    scheduler::kill(host).unwrap();
    assert_eq!(scheduler::surface_status(app) & 1, 1);
    assert_eq!(
        scheduler::surface_status(stranger) & 1,
        1,
        "host death stops descendants"
    );
    assert_eq!(scheduler::waitpid(host), 9);
    assert_eq!(scheduler::surface_status(app), u64::MAX - 1);
    assert_eq!(
        scheduler::surface_status(stranger),
        u64::MAX - 1,
        "host reap releases descendants"
    );
}

fn nested_exit_status_checks(context: &mut u64) {
    let host = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let app = scheduler::spawn_process(&[0xc3], 0).unwrap();
    let worker = scheduler::spawn_process(&[0xc3], 0).unwrap();
    activate(host, context);
    assert!(scheduler::connect_stdio(app));
    activate(app, context);
    assert!(scheduler::connect_stdio(worker));
    activate(worker, context);
    scheduler::exit_current(23);
    *context = scheduler::schedule(0);
    activate(0, context);
    scheduler::kill(host).unwrap();
    assert_eq!(
        scheduler::waitpid(worker),
        23,
        "host death preserves an already exited descendant status"
    );
    assert_eq!(scheduler::waitpid(host), 9);
    assert_eq!(scheduler::surface_status(app), u64::MAX - 1);
}
