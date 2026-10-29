//! Full-size, padded-pitch surfaces and large compositor process mappings.
#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use bootloader::BootInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use self_rust_os::{allocator, gfx::FbInfo, memory, scheduler};
use x86_64::{structures::paging::FrameAllocator, VirtAddr};

const PITCH: usize = 1920 * 4 + 64;
const BYTES: usize = PITCH * 1080;
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
            width: 1920,
            height: 1080,
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

/// Clip at the 1080p edges without writing scanline padding or trailing guards.
#[test_case]
fn drawing_at_display_edges_preserves_padding_and_guards() {
    let display = DISPLAY_ADDR.load(Ordering::Relaxed) as *mut u32;
    // SAFETY: main keeps the owned heap buffer alive for the whole harness.
    // These aligned u32 offsets cover the last two rows and four trailing
    // guard words, all within its allocation. Tests run sequentially.
    unsafe {
        for row in 1078..1080 {
            for x in 1920..PITCH / 4 {
                display.add(row * (PITCH / 4) + x).write_volatile(0xaabbcc);
            }
        }
        for i in BYTES / 4..BYTES / 4 + 4 {
            display.add(i).write_volatile(0xaabbcc);
        }
    }
    self_rust_os::gfx::with_framebuffer(|fb| {
        fb.fill_rect(1918, 1078, 20, 20, 0x112233);
        fb.set_pixel(1920, 1079, 0xff);
        fb.set_pixel(1919, 1080, 0xff);
    });
    // SAFETY: Same live buffer and in-bounds offsets as above, read only
    // after the framebuffer driver's exclusive lock has been released.
    unsafe {
        for row in 1078..1080 {
            for x in 1918..1920 {
                assert_eq!(display.add(row * (PITCH / 4) + x).read_volatile(), 0x112233);
            }
            for x in 1920..PITCH / 4 {
                assert_eq!(display.add(row * (PITCH / 4) + x).read_volatile(), 0xaabbcc);
            }
        }
        for i in BYTES / 4..BYTES / 4 + 4 {
            assert_eq!(display.add(i).read_volatile(), 0xaabbcc);
        }
    }
}

/// Exercise a compositor-sized image, full padded snapshots and repeated reaping.
#[test_case]
fn large_binary_and_padded_surfaces_survive_spawn_snapshot_and_reap() {
    let binary = alloc::vec![0xc3u8; 2 * 1920 * 1080 * 4 + 65536];
    scheduler::spawn_process(&binary, 0).expect("1080p compositor mapping");
    drop(binary);
    // Establish PID 0 as current (and activate its page tables). Keep running
    // this kernel harness rather than restoring the returned Ring 3 context.
    let mut context = scheduler::schedule(0);
    assert_eq!(scheduler::current_pid(), Some(0));
    let mut pixels = alloc::vec![0xa5u8; BYTES];
    assert_eq!(scheduler::snapshot(u32::MAX, &mut pixels), u64::MAX - 1);
    assert_eq!(
        scheduler::snapshot(0, &mut pixels),
        u64::MAX - 1,
        "the display owner has no private surface"
    );
    for _ in 0..3 {
        let first = scheduler::spawn_process(&[0xc3], 0).expect("private surface mapping");
        assert_eq!(scheduler::surface_status(u32::MAX), u64::MAX - 1);
        assert_eq!(scheduler::surface_status(0), u64::MAX - 1);
        assert_eq!(scheduler::surface_status(first), 0);
        assert_eq!(scheduler::snapshot(first, &mut pixels), 0);
        assert!(
            pixels.iter().all(|byte| *byte == 0),
            "surface must be entirely zeroed, including its last partial page"
        );
        activate(first, &mut context);
        assert_eq!(scheduler::receive_message(), 0, "new clients have empty inboxes");
        assert_eq!(scheduler::send_message(0, 0x12345678), 0, "clients can contact the display owner");
        activate(0, &mut context);
        assert_eq!(scheduler::receive_message(), ((u64::from(first) + 1) << 32) | 0x12345678,
            "transport supplies the actual sender");
        assert_eq!(scheduler::receive_message(), 0, "messages are consumed once");
        for payload in 0..16 {
            assert_eq!(scheduler::send_message(first, payload), 0);
        }
        assert_eq!(scheduler::send_message(first, 99), 1, "full inbox applies backpressure");
        activate(first, &mut context);
        for payload in 0..16 {
            assert_eq!(scheduler::receive_message(), (1u64 << 32) | payload);
        }
        assert_eq!(scheduler::receive_message(), 0);
        assert_eq!(scheduler::send_message(u32::MAX, 0), u64::MAX - 1);
        assert_eq!(scheduler::present_surface(), 0);
        assert_eq!(scheduler::surface_status(first), 2, "publication advances revision");
        scheduler::surface_write(b"x");
        assert_eq!(scheduler::surface_status(first), 4, "console output publishes too");
        scheduler::graphics_mode();
        scheduler::surface_write(b"diagnostic");
        assert_eq!(scheduler::surface_status(first), 4, "graphics logs do not change pixels");
        let last = (self_rust_os::userspace::USER_FRAMEBUFFER + BYTES as u64 - 1) as *mut u8;
        // SAFETY: first is current and owns this fully mapped, private
        // pitch * height surface. No references span a context change.
        unsafe {
            last.write_volatile(0x41);
        }
        // Spawn from a client too: clearing its copied page tables must not
        // keep aliases to the first client's surface or the compositor's code.
        let second = scheduler::spawn_process(&[0xc3], 0).expect("second private surface");
        activate(second, &mut context);
        assert_eq!(scheduler::send_message(first, 0), u64::MAX - 1,
            "clients cannot inject events into another client's inbox");
        // SAFETY: second now owns the active mapping at the same address.
        // The last byte is within its complete surface (partial final page).
        unsafe {
            assert_eq!(last.read_volatile(), 0);
            last.write_volatile(0x42);
        }
        activate(0, &mut context);
        for (pid, marker) in [(first, 0x41), (second, 0x42)] {
            assert_eq!(scheduler::snapshot(pid, &mut pixels), 0);
            assert_eq!(pixels[BYTES - 1], marker, "client surfaces must not alias");
            pixels[BYTES - 1] = 0xff;
            assert_eq!(scheduler::snapshot(pid, &mut pixels), 0);
            assert_eq!(
                pixels[BYTES - 1],
                marker,
                "snapshot must not alias its destination"
            );
            assert!(scheduler::kill(pid).is_some());
            assert_eq!(scheduler::send_message(pid, 0), u64::MAX - 1,
                "dead recipients reject new messages");
            assert_eq!(scheduler::snapshot(pid, &mut pixels), 1);
            assert_eq!(scheduler::surface_status(pid) & 1, 1, "exit is observable without copying pixels");
            scheduler::waitpid(pid);
            assert_eq!(scheduler::surface_status(pid), u64::MAX - 1);
            assert_eq!(scheduler::snapshot(pid, &mut pixels), u64::MAX - 1);
        }
        // SAFETY: PID 0's live code mapping covers this byte. Spawning a
        // child must preserve its physical backing and supervisor mappings.
        let last = self_rust_os::userspace::USER_CODE_START + 2 * 1920 * 1080 * 4 + 65535;
        assert_eq!(unsafe { (last as *const u8).read_volatile() }, 0xc3);
    }
    context = keyboard_wait_checks(context);
    timed_wait_checks(context);
}

/// A blocking keyboard read resumes with zero so its caller retries the buffer
/// read, instead of interpreting the old syscall number as newly read bytes.
fn keyboard_wait_checks(mut context: u64) -> u64 {
    x86_64::instructions::interrupts::disable();
    let child = scheduler::spawn_process(&[0xc3], 0).expect("keyboard child");
    activate(child, &mut context);
    // SAFETY: activate returned this live child's aligned, owned kernel-stack
    // SavedContext. Only this single-CPU harness accesses it, with IRQs masked.
    unsafe {
        (*(context as *mut scheduler::SavedContext)).rax = 2;
    }
    assert_eq!(
        scheduler::block_on_keyboard(),
        scheduler::PROCESS_BLOCK_SENTINEL
    );
    context = scheduler::schedule(context);
    assert_eq!(scheduler::current_pid(), Some(0));
    scheduler::wake_keyboard_waiters();
    activate(child, &mut context);
    // SAFETY: This is again the current child's live saved context; the previous
    // exclusive access ended before scheduling. IRQs remain disabled.
    assert_eq!(
        unsafe { (*(context as *const scheduler::SavedContext)).rax },
        0,
        "keyboard wake must not replay stale user-buffer bytes"
    );
    assert_eq!(
        scheduler::block_on_keyboard(),
        scheduler::PROCESS_BLOCK_SENTINEL
    );
    context = scheduler::schedule(context);
    assert!(scheduler::kill(child).is_some());
    scheduler::wake_keyboard_waiters();
    for _ in 0..3 {
        let next = scheduler::on_timer(context);
        if next != 0 {
            context = next;
        }
    }
    assert_eq!(
        scheduler::current_pid(),
        Some(0),
        "killed keyboard waiter cannot wake"
    );
    assert_eq!(scheduler::waitpid(child), 9);
    context
}

/// A sleeping compositor yields, wakes from a kernel-mode timer, and can be killed.
fn timed_wait_checks(mut context: u64) {
    // Match syscall-entry interrupt state when calling scheduler primitives.
    x86_64::instructions::interrupts::disable();
    assert_eq!(scheduler::current_pid(), Some(0));
    assert_eq!(scheduler::sleep_ticks(0), 0, "zero duration must not block");
    // Make a representable deadline overflow irrespective of the current tick.
    // The API rejects durations above i64::MAX as well as overflowing deadlines.
    assert_eq!(scheduler::sleep_ticks(u64::MAX), u64::MAX - 1);
    let child = scheduler::spawn_process(&[0xc3], 0).expect("timer child");
    let before = self_rust_os::interrupts::ticks();
    assert_eq!(scheduler::sleep_ticks(1), scheduler::PROCESS_BLOCK_SENTINEL);
    context = scheduler::schedule(context);
    assert_eq!(scheduler::current_pid(), Some(child), "sleeper must yield");
    assert_eq!(scheduler::sleep_ticks(1), scheduler::PROCESS_BLOCK_SENTINEL);
    // With both processes blocked, schedule must idle and the timer must wake
    // them even though it interrupted Ring 0 rather than a user context.
    // SAFETY: IDT and both contexts are initialized; this test deliberately
    // unmasks IRQ0 only after blocking has made a valid idle context available.
    unsafe { self_rust_os::interrupts::PICS.lock().write_masks(0xf8, 0xef); }
    context = scheduler::schedule(context);
    assert!(self_rust_os::interrupts::ticks() > before);
    // SAFETY: Returned context points into the live process's owned kernel
    // stack, just restored by schedule with IRQs disabled. Read only its RAX.
    assert_eq!(unsafe { (*(context as *const scheduler::SavedContext)).rax }, 0,
        "sleep must resume with the ABI success return value");
    // SAFETY: Return to deterministic manual scheduling for the rest of the test.
    unsafe { self_rust_os::interrupts::PICS.lock().write_masks(0xf9, 0xef); }
    activate(child, &mut context);
    assert_eq!(scheduler::sleep_ticks(1), scheduler::PROCESS_BLOCK_SENTINEL);
    context = scheduler::schedule(context);
    assert_eq!(scheduler::current_pid(), Some(0));
    assert!(scheduler::kill(child).is_some());
    // SAFETY: The live PID 0 context is established, as above.
    unsafe { self_rust_os::interrupts::PICS.lock().write_masks(0xf8, 0xef); }
    assert_eq!(scheduler::sleep_ticks(2), scheduler::PROCESS_BLOCK_SENTINEL);
    scheduler::schedule(context);
    // SAFETY: Mask IRQ0 again before returning to the harness.
    unsafe { self_rust_os::interrupts::PICS.lock().write_masks(0xf9, 0xef); }
    assert_eq!(scheduler::current_pid(), Some(0), "killed sleeper cannot run");
    assert_eq!(scheduler::waitpid(child), 9);
    x86_64::instructions::interrupts::enable();
}

// Use the scheduler's real page-table switch without restoring a Ring 3
// context. The harness continues on shared supervisor code/stack mappings.
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
