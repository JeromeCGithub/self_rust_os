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
    argument_mapping_checks(&mut context);
    heap_mapping_checks(&mut context);
    restricted_heap_checks(&mut context);
    elf_mapping_checks(&mut context);
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

fn heap_mapping_checks(context: &mut u64) {
    use self_rust_os::userspace::usercopy;
    let parent = scheduler::current_pid().unwrap();
    let baseline = scheduler::frame_count();
    for _ in 0..3 {
        let child = scheduler::spawn_process(&[0xc3], 0).unwrap();
        activate_pid(child, context);
        let base = rustos_thread_abi::HEAP_BASE;
        assert_eq!(scheduler::program_break(0), base);
        assert_eq!(scheduler::program_break(base + 8192), base + 8192);
        let mut zero = [1; 16];
        assert!(usercopy::from_user(base, &mut zero));
        assert_eq!(zero, [0; 16]);
        assert!(usercopy::to_user(base, &[42; 16]));
        assert_eq!(scheduler::program_break(base - 1), base + 8192);
        assert_eq!(scheduler::program_break(base), base);
        assert!(!usercopy::validate(base, 1, true));
        assert_eq!(scheduler::program_break(base + 8192), base + 8192);
        assert!(usercopy::from_user(base, &mut zero));
        assert_eq!(zero, [0; 16]);
        activate_pid(parent, context);
        assert!(!usercopy::validate(base, 1, false));
        scheduler::kill(child).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        assert_eq!(scheduler::frame_count(), baseline);
    }
}

// Verify physical child ownership and the actual saved entry registers, then
// reap repeatedly to cover startup-page staging and frame/heap cleanup.
fn argument_mapping_checks(context: &mut u64) {
    use self_rust_os::userspace::usercopy;
    let parent = scheduler::current_pid().unwrap();
    let baseline = scheduler::frame_count();
    let heap = allocator::allocated_bytes();
    let arguments =
        rustos_process_abi::Arguments::new(&["probe", "", "two words", "café"]).unwrap();
    for _ in 0..3 {
        let child = scheduler::spawn_process_with_args(&[0xc3], 0, &arguments).unwrap();
        activate_pid(child, context);
        // SAFETY: The scheduler selected this live child's canonical frame;
        // this single-CPU harness does not concurrently resume user code.
        let frame = unsafe { &*(*context as *const scheduler::SavedContext) };
        assert_eq!(frame.rdi, 4);
        assert_eq!(frame.rsp % 16, 8);
        assert_eq!(
            frame.rsi,
            self_rust_os::userspace::USER_STACK_TOP - 4096 + 8
        );
        let mut pointer_bytes = [0; 40];
        assert!(usercopy::from_user(frame.rsi, &mut pointer_bytes));
        for (index, expected) in ["probe", "", "two words", "café"].iter().enumerate() {
            let pointer =
                u64::from_le_bytes(pointer_bytes[index * 8..index * 8 + 8].try_into().unwrap());
            assert!((self_rust_os::userspace::USER_STACK_TOP - 4096
                ..self_rust_os::userspace::USER_STACK_TOP)
                .contains(&pointer));
            let mut string = [255; 16];
            assert!(usercopy::from_user(
                pointer,
                &mut string[..expected.len() + 1]
            ));
            assert_eq!(&string[..expected.len()], expected.as_bytes());
            assert_eq!(string[expected.len()], 0);
        }
        assert_eq!(&pointer_bytes[32..], &[0; 8]);
        // User virtual addresses coincide, but the initial process's argument
        // page remains empty and cannot alias the newly populated child page.
        activate_pid(parent, context);
        let mut bytes = [255; 8];
        assert!(usercopy::from_user(
            self_rust_os::userspace::USER_STACK_TOP - 4096,
            &mut bytes
        ));
        assert_eq!(bytes, [0; 8]);
        scheduler::kill(child).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        assert_eq!(scheduler::frame_count(), baseline);
    }
    assert_eq!(
        allocator::allocated_bytes(),
        heap,
        "argument staging and process metadata must be reclaimed"
    );
}

fn elf_mapping_checks(context: &mut u64) {
    use self_rust_os::userspace::usercopy;
    let parent = scheduler::current_pid().unwrap();
    let baseline = scheduler::frame_count();
    let mut bytes = alloc::vec![0u8; 0x2004];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    for (offset, value) in [(16, 2u16), (18, 62), (52, 64), (54, 56), (56, 2)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x400001u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    for (i, address, flags, memory) in [(0, 0x400000u64, 5u32, 4u64), (1, 0x401000, 6, 0x5001)] {
        let p = 64 + i * 56;
        bytes[p..p + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[p + 4..p + 8].copy_from_slice(&flags.to_le_bytes());
        for (field, value) in [
            (8, (i as u64 + 1) * 0x1000),
            (16, address),
            (32, 4),
            (40, memory),
            (48, 4096),
        ] {
            bytes[p + field..p + field + 8].copy_from_slice(&value.to_le_bytes());
        }
    }
    bytes[0x1000..0x1004].copy_from_slice(&[0x90, 0xc3, 0x90, 0x90]);
    bytes[0x2000..].copy_from_slice(b"data");
    for _ in 0..3 {
        let child = scheduler::spawn_process(&bytes, 0).unwrap();
        activate_pid(child, context);
        let mut code = [0; 4];
        assert!(usercopy::from_user(0x400000, &mut code));
        assert_eq!(
            code,
            [0x90, 0xc3, 0x90, 0x90],
            "ELF headers are not application instructions"
        );
        assert!(
            !usercopy::validate(0x400000, 4, true),
            "ELF code is read only"
        );
        assert!(usercopy::validate(0x401000, 0x6000, true));
        let mut data = [0; 4];
        assert!(usercopy::from_user(0x401000, &mut data));
        assert_eq!(&data, b"data");
        let mut zero = [0xa5; 4096];
        assert!(usercopy::from_user(0x406000, &mut zero));
        assert_eq!(
            zero, [0; 4096],
            "zero fill includes padding on the final BSS page"
        );
        assert!(
            !usercopy::validate(0x407000, 1, false),
            "unmapped segment guard"
        );
        // SAFETY: activate selected the child's owned initialized kernel frame.
        assert_eq!(
            unsafe { (*(*context as *const scheduler::SavedContext)).rip },
            0x400001
        );
        assert!(usercopy::to_user(0x401000, b"edit"));
        activate_pid(parent, context);
        assert!(
            !usercopy::validate(0x401000, 1, false),
            "child mapping cannot alter its parent"
        );
        scheduler::kill(child).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        assert_eq!(scheduler::frame_count(), baseline);
    }
    let mut shared = bytes.clone();
    shared.resize(0x200c, 0);
    shared[56..58].copy_from_slice(&3u16.to_le_bytes());
    shared[64 + 56 + 4..64 + 56 + 8].copy_from_slice(&4u32.to_le_bytes());
    shared[64 + 56 + 40..64 + 56 + 48].copy_from_slice(&4u64.to_le_bytes());
    let p = 64 + 2 * 56;
    shared[p..p + 4].copy_from_slice(&1u32.to_le_bytes());
    shared[p + 4..p + 8].copy_from_slice(&6u32.to_le_bytes());
    for (field, value) in [(8, 0x2004u64), (16, 0x401004), (32, 8), (40, 8), (48, 4096)] {
        shared[p + field..p + field + 8].copy_from_slice(&value.to_le_bytes());
    }
    shared[0x2004..].copy_from_slice(b"got-data");
    let child = scheduler::spawn_process(&shared, 0).unwrap();
    activate_pid(child, context);
    let mut combined = [0; 12];
    assert!(usercopy::from_user(0x401000, &mut combined));
    assert_eq!(
        &combined, b"datagot-data",
        "shared page preserves both segment payloads"
    );
    assert!(
        usercopy::validate(0x401000, 4096, true),
        "shared GOT page unions writable permission"
    );
    assert!(
        !usercopy::validate(0x400000, 1, true),
        "separate code page stays read only"
    );
    activate_pid(parent, context);
    scheduler::kill(child).unwrap();
    assert_eq!(scheduler::waitpid(child), 9);
    scheduler::reap_retired();
    assert_eq!(scheduler::frame_count(), baseline);
    bytes[64 + 40..64 + 48].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(scheduler::spawn_process(&bytes, 0).is_err());
    assert_eq!(
        scheduler::frame_count(),
        baseline,
        "malformed image must allocate no frames"
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

// The worker budget restricts complete owned pages; failures preserve mappings.
fn restricted_heap_checks(context: &mut u64) {
    let parent = scheduler::current_pid().unwrap();
    assert_eq!(scheduler::restrict_worker(1, 65536, 0), u64::MAX - 1);
    let baseline = scheduler::frame_count();
    for _ in 0..3 {
        let child = scheduler::spawn_process(&[0xc3], 0).unwrap();
        assert!(scheduler::connect_stdio(child));
        activate_pid(child, context);
        assert_eq!(scheduler::restrict_worker(1, 65537, 0), u64::MAX - 1);
        assert_eq!(scheduler::restrict_worker(1, 65536, 0), 0);
        let base = rustos_thread_abi::HEAP_BASE;
        assert_eq!(scheduler::program_break(base + 65536), base + 65536);
        assert_eq!(scheduler::program_break(base + 65537), base + 65536);
        assert!(self_rust_os::userspace::usercopy::validate(
            base + 65535,
            1,
            true
        ));
        assert!(!self_rust_os::userspace::usercopy::validate(
            base + 65536,
            1,
            true
        ));
        assert_eq!(scheduler::restrict_worker(1, 131072, 0), u64::MAX - 1);
        activate_pid(parent, context);
        assert!(!self_rust_os::userspace::usercopy::validate(base, 1, false));
        scheduler::kill(child).unwrap();
        assert_eq!(scheduler::waitpid(child), 9);
        scheduler::reap_retired();
        assert_eq!(scheduler::frame_count(), baseline);
    }
}
