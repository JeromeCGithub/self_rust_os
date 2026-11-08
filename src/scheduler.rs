//! Native thread scheduler and PID-owned process resources.
//! A global ready queue is serialized by the scheduler lock. Architecture entry
//! holds the kernel gate through the actual stack handoff, preventing a second
//! CPU from resuming a context before its old stack has stopped executing.
use crate::{
    arch::x86_64::{entry, smp},
    gdt,
    memory::RegionFrameAllocator,
    userspace,
};
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::{
    registers::control::{Cr3, Cr3Flags},
    structures::paging::{FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB},
    VirtAddr,
};

/// Internal block sentinel consumed by architecture entry.
pub const PROCESS_BLOCK_SENTINEL: u64 = u64::MAX - 2;
/// Internal timer result requesting the CPU's dedicated idle stack.
pub const IDLE_RSP: u64 = u64::MAX;
const ERROR: u64 = u64::MAX - 1;
mod channels;
pub use channels::{can_connect_stdio, connect_stdio, stdio, stdio_selected};
use channels::{reap_connection_locked, stop_owned_child_locked, STDIO_QUEUE_CAPACITY};
const MAX_THREADS: usize = 1024;

pub use crate::process::{BlockReason, Pid, Process, ProcessState, ThreadState, Tid};
use crate::process::{ProcessTable, Thread};

/// Canonical frame shared with architecture entry.
pub use crate::arch::x86_64::context::SavedContext;

struct Scheduler {
    processes: ProcessTable,
    threads: Vec<Option<Thread>>,
    retired: Vec<Process>,
    ready_queue: VecDeque<Tid>,
    current: [Option<Tid>; 32],
    next_pid: Pid,
    init_pid: Option<Pid>,
    generation: u64,
    phys_offset: u64,
    frame_allocator: Option<RegionFrameAllocator>,
}
impl Scheduler {
    const fn new() -> Self {
        Self {
            processes: ProcessTable::new(),
            threads: Vec::new(),
            retired: Vec::new(),
            ready_queue: VecDeque::new(),
            current: [None; 32],
            next_pid: 0,
            init_pid: None,
            generation: 1,
            phys_offset: 0,
            frame_allocator: None,
        }
    }
    fn thread(&self, tid: Tid) -> Option<&Thread> {
        self.threads
            .get((tid & 65535) as usize)?
            .as_ref()
            .filter(|t| t.tid == tid)
    }
    fn thread_mut(&mut self, tid: Tid) -> Option<&mut Thread> {
        self.threads
            .get_mut((tid & 65535) as usize)?
            .as_mut()
            .filter(|t| t.tid == tid)
    }
    fn current_tid(&self) -> Option<Tid> {
        self.current[smp::current_cpu()]
    }
    fn current_pid(&self) -> Option<Pid> {
        self.current_tid()
            .and_then(|tid| self.thread(tid).map(|t| t.pid))
    }
    fn reserve_tid(&self) -> Result<Tid, &'static str> {
        let slot = self
            .threads
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.threads.len());
        if slot >= MAX_THREADS || self.generation >= 1u64 << 47 {
            return Err("thread quota exhausted");
        }
        Ok((self.generation << 16) | slot as u64)
    }
    fn insert(&mut self, thread: Thread) {
        let slot = (thread.tid & 65535) as usize;
        if slot == self.threads.len() {
            self.threads.push(None);
        }
        self.ready_queue.push_back(thread.tid);
        self.threads[slot] = Some(thread);
        self.generation += 1;
    }
}
static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

mod supervision;
pub use supervision::{child_next_user, terminate_child, configure_native_init, fault_current};
mod events;
mod placement;
pub use events::wait_events;

/// Initialize the shared ready queue and physical-frame ownership pool once.
pub fn init(phys_offset: VirtAddr, mut frame_allocator: RegionFrameAllocator) {
    smp::retain_boot_root();
    frame_allocator.enable_recycling();
    let mut s = SCHEDULER.lock();
    s.processes.reserve();
    s.ready_queue.reserve_exact(MAX_THREADS);
    s.threads.reserve_exact(MAX_THREADS);
    s.retired.reserve_exact(128);
    s.phys_offset = phys_offset.as_u64();
    s.frame_allocator = Some(frame_allocator);
}
fn allocate_kernel_stack() -> Result<Box<[u128]>, &'static str> {
    let mut v = Vec::new();
    v.try_reserve_exact(1024)
        .map_err(|_| "kernel stack allocation failed")?;
    v.resize(1024, 0u128);
    Ok(v.into_boxed_slice())
}
fn make_thread(
    tid: Tid,
    pid: Pid,
    stack: Box<[u128]>,
    rip: u64,
    rsp: u64,
    args: (u64, u64, u64),
    affinity: u64,
) -> Thread {
    let top = stack.as_ptr() as u64 + (stack.len() * 16) as u64;
    let saved = top - core::mem::size_of::<SavedContext>() as u64;
    // SAFETY: u128 storage is 16-byte aligned, uniquely owned and large enough
    // for the canonical frame. All fields are initialized before publication.
    unsafe {
        core::ptr::write(
            saved as *mut SavedContext,
            SavedContext {
                r15: 0,
                r14: 0,
                r13: 0,
                r12: 0,
                r11: 0,
                r10: 0,
                r9: 0,
                r8: 0,
                rbp: 0,
                rdi: args.0,
                rsi: args.1,
                rdx: args.2,
                rcx: 0,
                rbx: 0,
                rax: 0,
                rip,
                cs: u64::from(gdt::user_code_selector().0),
                rflags: 0x202,
                rsp,
                ss: u64::from(gdt::user_data_selector().0),
            },
        );
    }
    Thread {
        tid,
        pid,
        state: ThreadState::Ready,
        reason: BlockReason::None,
        saved_kernel_rsp: saved,
        kernel_stack: stack,
        user_stack: None,
        affinity,
        detached: false,
        joiner: None,
        extended: crate::arch::x86_64::extended::State::new(),
        fs_base: 0,
    }
}
/// Create an isolated static ELF/legacy flat process and its initial native thread.
pub fn spawn_process(binary: &[u8], readonly_size: usize) -> Result<Pid, &'static str> {
    spawn_process_with_environment(binary, readonly_size, &[], &[])
}
/// Compatibility loader for the fixed-size syscall-29 argument record.
pub fn spawn_process_with_args(
    binary: &[u8],
    readonly_size: usize,
    arguments: &rustos_process_abi::Arguments,
) -> Result<Pid, &'static str> {
    let mut argv = [""; rustos_process_abi::MAX_ARGS];
    for (index, slot) in argv.iter_mut().take(arguments.len()).enumerate() {
        *slot = arguments.get(index).ok_or("invalid arguments")?;
    }
    spawn_process_with_environment(binary, readonly_size, &argv[..arguments.len()], &[])
}
/// Create a private process with kernel-owned, bounded argv and environment.
pub fn spawn_process_with_environment(
    binary: &[u8],
    readonly_size: usize,
    argv: &[&str],
    envp: &[&str],
) -> Result<Pid, &'static str> {
    rustos_process_abi::startup_size(argv, envp).map_err(|_| "invalid startup vectors")?;
    x86_64::instructions::interrupts::without_interrupts(|| {
        spawn_process_inner(binary, readonly_size, argv, envp)
    })
}
/// Deep-copies the page-table subtree under `new_p3[0]` (the first GB).
///
/// Must be called right after shallow-copying the P3 entries. It replaces
/// `new_p3[0]`'s P2 — and every present, non-huge P1 beneath it — with
/// private copies. Kernel low-memory mappings keep pointing at the same
/// physical frames (sharing is preserved); only the page *tables* become
/// private, so unmapping/remapping user pages in the new address space
/// cannot corrupt the source process.
fn deep_copy_p3_subtree(
    new_p3: &mut PageTable,
    allocator: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: VirtAddr,
) -> Result<(), &'static str> {
    use x86_64::structures::paging::page_table::PageTableFlags;

    if !new_p3[0].flags().contains(PageTableFlags::PRESENT)
        || new_p3[0].flags().contains(PageTableFlags::HUGE_PAGE)
    {
        return Ok(());
    }
    let cur_p2_frame = new_p3[0].frame().map_err(|_| "P3[0] frame error")?;
    // SAFETY: The frame is a valid P2, readable via the physical offset mapping.
    let cur_p2 = unsafe {
        &*((phys_offset + cur_p2_frame.start_address().as_u64()).as_ptr() as *const PageTable)
    };

    let new_p2_frame = allocator
        .allocate_frame()
        .ok_or("failed to allocate P2 copy")?;
    // SAFETY: Freshly allocated frame, writable via the physical offset mapping.
    let new_p2 = unsafe {
        &mut *((phys_offset + new_p2_frame.start_address().as_u64()).as_mut_ptr() as *mut PageTable)
    };
    for j in 0..512 {
        new_p2[j] = cur_p2[j].clone();
    }

    for j in 0..512 {
        // Replace the entire reserved user region with private, empty P1
        // tables. This removes GRUB identity huge pages and all inherited
        // application mappings, including the larger 1080p buffers. Entries
        // below USER_CODE_START retain supervisor kernel mappings.
        const PAGE_2MIB: u64 = 2 * 1024 * 1024;
        let user_start = (userspace::USER_CODE_START / PAGE_2MIB) as usize;
        let user_end = (userspace::USER_REGION_END / PAGE_2MIB) as usize;
        let is_user_p2 = (user_start..user_end).contains(&j);

        if is_user_p2 {
            new_p2[j].set_unused();
            continue;
        }

        if !new_p2[j].flags().contains(PageTableFlags::PRESENT)
            || new_p2[j].flags().contains(PageTableFlags::HUGE_PAGE)
        {
            continue;
        }
        let cur_p1_frame = new_p2[j].frame().map_err(|_| "P2 entry frame error")?;
        // SAFETY: The frame is a valid P1, readable via the physical offset mapping.
        let cur_p1 = unsafe {
            &*((phys_offset + cur_p1_frame.start_address().as_u64()).as_ptr() as *const PageTable)
        };
        let new_p1_frame = allocator
            .allocate_frame()
            .ok_or("failed to allocate P1 copy")?;
        // SAFETY: Freshly allocated frame, writable via the physical offset mapping.
        let new_p1 = unsafe {
            &mut *((phys_offset + new_p1_frame.start_address().as_u64()).as_mut_ptr()
                as *mut PageTable)
        };
        for k in 0..512 {
            new_p1[k] = cur_p1[k].clone();
        }
        let p2_flags = new_p2[j].flags();
        new_p2[j].set_frame(new_p1_frame, p2_flags);
    }

    let p3_flags = new_p3[0].flags();
    new_p3[0].set_frame(new_p2_frame, p3_flags);
    Ok(())
}

fn spawn_process_inner(
    binary: &[u8],
    readonly_size: usize,
    argv: &[&str],
    envp: &[&str],
) -> Result<Pid, &'static str> {
    if binary.len() as u64 > rustos_thread_abi::IMAGE_END - userspace::USER_CODE_START {
        return Err("user binary overlaps stack");
    }
    let image = rustos_exec_image::Image::parse(
        binary,
        userspace::USER_CODE_START,
        rustos_thread_abi::IMAGE_END,
    )
    .map_err(|_| "invalid or unsupported executable image")?;
    let tls_image = if let Some(template) = image.tls() {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(template.bytes.len())
            .map_err(|_| "TLS prototype allocation failed")?;
        bytes.extend_from_slice(template.bytes);
        Some(crate::process::TlsImage {
            bytes,
            memory_size: template.memory_size as usize,
            alignment: template.alignment as usize,
        })
    } else {
        None
    };
    let mut sched = SCHEDULER.lock();
    if sched.processes.iter().flatten().count() + sched.retired.len() >= 128
        || sched.next_pid == u32::MAX
    {
        return Err("process quota exhausted");
    }
    let kernel_stack = allocate_kernel_stack()?;
    let tid = sched.reserve_tid()?;
    let phys_offset = VirtAddr::new(sched.phys_offset);
    // Read next_pid before the mutable borrow of allocator.
    let is_first_process = sched.next_pid == 0;
    let pool = sched
        .frame_allocator
        .as_mut()
        .ok_or("scheduler not initialized")?;
    let mut transaction = crate::memory::owned::FrameTransaction::new(pool);
    let allocator = &mut transaction;

    // 1. Allocate and set up a new P4 page table.
    let p4_frame = allocator
        .allocate_frame()
        .ok_or("failed to allocate P4 frame")?;
    let p4_virt = phys_offset + p4_frame.start_address().as_u64();
    // SAFETY: The frame was just allocated, and phys_offset maps all RAM.
    let p4_table = unsafe { &mut *(p4_virt.as_mut_ptr() as *mut PageTable) };

    // Copy P4[1..512] (kernel half and everything except P4[0]).
    // For P4[0] (user half, 0..512GB), allocate a PRIVATE P3 that is a copy
    // of the current P3. We then unmap the user regions from it, so each
    // process has isolated user mappings but shares kernel low-memory
    // mappings (e.g., kernel stack).
    let (cur_frame, _) = Cr3::read();
    let cur_virt = phys_offset + cur_frame.start_address().as_u64();
    // SAFETY: Reading the current P4 via the physical offset mapping.
    let cur_table = unsafe { &*(cur_virt.as_ptr() as *const PageTable) };
    for i in 1..512 {
        p4_table[i] = cur_table[i].clone();
    }
    // Allocate a private P3 for P4[0]: copy current P3's entries.
    {
        use x86_64::structures::paging::page_table::PageTableFlags;
        // Get current P3 (P4[0]'s target).
        let cur_p3_frame = cur_table[0].frame().map_err(|_| "P4[0] not mapped")?;
        let cur_p3_virt = phys_offset + cur_p3_frame.start_address().as_u64();
        let cur_p3 = unsafe { &*(cur_p3_virt.as_ptr() as *const PageTable) };
        // Allocate fresh P3 and copy entries.
        let new_p3_frame = allocator.allocate_frame().ok_or("failed to allocate P3")?;
        let new_p3_virt = phys_offset + new_p3_frame.start_address().as_u64();
        let new_p3 = unsafe { &mut *(new_p3_virt.as_mut_ptr() as *mut PageTable) };
        new_p3[0] = cur_p3[0].clone();
        for i in 1..512 {
            new_p3[i].set_unused();
        }
        // Deep-copy P3[0]'s subtree (first GB, home of all user mappings).
        // The entry copy above is shallow: new P3[0] still points at the
        // source process's P2, whose P1 tables are shared. Unmapping and
        // remapping the new process's user pages below would then modify
        // the source's mappings too (its binary/stack would point at the
        // new process's frames), corrupting the source process. The deep
        // copy gives the new process private page tables while keeping the
        // same physical frames for kernel low-memory mappings.
        deep_copy_p3_subtree(new_p3, allocator, phys_offset)?;
        let flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        p4_table[0].set_frame(new_p3_frame, flags);
    }

    // 2. Create a mapper for the new address space.
    // SAFETY: p4_table is a valid, freshly allocated P4.
    let mut new_mapper = unsafe { OffsetPageTable::new(p4_table, phys_offset) };

    // 3. Map user binary, stack, and framebuffer into the new P4.
    // We do NOT switch CR3 here. The mapper writes page tables via p4_table
    // (virtual address, valid in current address space). Binary content is
    // copied via physical addresses (phys_offset + frame).
    let map_result = (|| {
        crate::userspace::process::map_user_image_via_phys(
            &image,
            readonly_size,
            &mut new_mapper,
            allocator,
            phys_offset,
        )
        .map_err(|_| "failed to map user binary")?;
        crate::userspace::process::map_user_stack_via_phys(&mut new_mapper, allocator, phys_offset)
            .map_err(|_| "failed to map user stack")?;
        // Worker stacks are later mapped into this live address space. Reserve
        // their private parent tables now so partial worker rollback never
        // leaves parent links pointing at transaction-recycled frames.
        let mut parents = Vec::new();
        for slot in 0..rustos_thread_abi::STACK_SLOTS {
            let address =
                rustos_thread_abi::STACK_ARENA + slot as u64 * rustos_thread_abi::STACK_STRIDE;
            for boundary in [address, address + rustos_thread_abi::STACK_STRIDE - 1] {
                // SAFETY: Unpublished private root; all newly allocated parent
                // tables belong to this construction transaction. No CPU sees
                // the tree, and failure discards it before recycling all frames.
                unsafe {
                    crate::memory::user::ensure_tables(
                        new_mapper.level_4_table(),
                        boundary,
                        allocator,
                        phys_offset,
                        &mut parents,
                    )
                }
                .map_err(|_| "worker stack parent allocation failed")?;
            }
        }
        let mut fs_base = 0;
        if let Some(template) = tls_image.as_ref() {
            for slot in 0..=rustos_thread_abi::STACK_SLOTS {
                let (base, _, pages, _) = crate::userspace::tls::region(template, slot);
                for boundary in [base, base + pages as u64 * 4096 - 1] {
                    // SAFETY: Unpublished private child; every parent allocation
                    // belongs to the enclosing frame transaction and rollback.
                    unsafe {
                        crate::memory::user::ensure_tables(
                            new_mapper.level_4_table(),
                            boundary,
                            allocator,
                            phys_offset,
                            &mut parents,
                        )
                    }
                    .map_err(|_| "TLS parent allocation failed")?;
                }
            }
            let mut mapped = 0;
            fs_base = crate::userspace::tls::map(
                template,
                0,
                &mut new_mapper,
                allocator,
                phys_offset,
                &mut mapped,
            )
            .map_err(|_| "initial TLS mapping failed")?
            .0;
        }
        let startup =
            crate::userspace::process::prepare_arguments(&new_mapper, phys_offset, argv, envp)?;
        let surface = crate::userspace::process::map_user_framebuffer(
            &mut new_mapper,
            allocator,
            is_first_process,
            phys_offset,
        )
        .map_err(|_| "failed to map framebuffer")?;
        Ok::<_, &'static str>((surface, startup, fs_base))
    })();
    let (surface, startup, fs_base) = map_result?;

    let owned_frames = transaction.commit();
    let pid = sched.next_pid;
    let cwd = sched
        .current_pid()
        .and_then(|parent| sched.processes.get(parent as usize))
        .and_then(Option::as_ref)
        .map(|p| p.files.cwd.clone())
        .unwrap_or_else(|| alloc::string::String::from("/"));
    let process = Process {
        pid,
        parent: sched.current_pid(),
        adoption_pending: false,
        state: ProcessState::Live,
        p4_frame,
        exit_code: 0,
        exit_cause: rustos_process_abi::supervision::Cause::Exited,
        surface,
        text_cursor: (8, 8),
        console_enabled: true,
        surface_revision: 0,
        owned_frames,
        stack_slots: 0,
        inbox: [0; 16],
        inbox_head: 0,
        inbox_len: 0,
        io_peer: None,
        io_inbox: VecDeque::new(),
        io_nested_peer: None,
        io_nested_inbox: VecDeque::new(),
        pointer_seen: crate::drivers::input::mouse::revision(),
        files: crate::fs::handles::Files::new(cwd),
        heap_break: rustos_thread_abi::HEAP_BASE,
        worker_heap_limit: None,
        tls: tls_image,
    };
    let mut thread = make_thread(
        tid,
        pid,
        kernel_stack,
        image.entry(),
        startup.stack_pointer,
        (startup.argc, startup.argv_pointer, startup.env_pointer),
        u64::MAX,
    );
    thread.fs_base = fs_base;
    sched.processes.insert(process);
    sched.insert(thread);
    sched.next_pid += 1;
    crate::serial_println!("[sched] spawned process {} ({} bytes)", pid, binary.len());
    Ok(pid)
}

/// Start all configured CPUs and restore the initial user thread.
pub fn start() -> ! {
    crate::console::DESKTOP_ACTIVE.store(true, Ordering::Relaxed);
    x86_64::instructions::interrupts::disable();
    entry::enter();
    smp::start();
    let rsp = idle_next();
    if rsp == 0 {
        panic!("scheduler: no initial process");
    }
    // SAFETY: Selection installed its live address space/TSS under the gate.
    unsafe {
        entry::restore(rsp);
    }
}
fn use_boot_space() {
    let physical = smp::boot_cr3();
    if physical != 0 {
        // SAFETY: BSP startup retains this supervisor boot root for CPU lifetime.
        unsafe {
            Cr3::write(
                PhysFrame::containing_address(x86_64::PhysAddr::new(physical)),
                Cr3Flags::empty(),
            );
        }
    }
}
fn pick_next_locked(s: &mut Scheduler) -> Option<u64> {
    let cpu = smp::current_cpu();
    for _ in 0..s.ready_queue.len() {
        let tid = s.ready_queue.pop_front()?;
        let Some(t) = s.thread(tid) else {
            continue;
        };
        if t.state != ThreadState::Ready {
            continue;
        }
        if t.affinity & (1u64 << cpu) == 0 {
            s.ready_queue.push_back(tid);
            continue;
        }
        let pid = t.pid;
        let root = s.processes.get(pid as usize)?.as_ref()?.p4_frame;
        let t = s.thread_mut(tid)?;
        t.state = ThreadState::Running;
        let (top, rsp) = (t.kernel_stack_top(), t.saved_kernel_rsp);
        // SAFETY: This CPU was initialized for x87/SSE2, this incoming thread
        // exclusively owns a valid aligned snapshot, and kernel code uses no
        // FP/SIMD or FS-relative TLS between here and final user restoration.
        unsafe {
            t.extended.restore();
            crate::arch::x86_64::extended::set_fs(t.fs_base);
        }
        s.current[cpu] = Some(tid);
        if Cr3::read().0 != root {
            // SAFETY: The selected process owns root. Kernel mappings/stacks
            // are supervisor and common to all address spaces. Gate prevents
            // teardown; mapping changes use explicit cross-CPU invalidation.
            unsafe {
                Cr3::write(root, Cr3Flags::empty());
            }
        }
        gdt::set_kernel_stack(VirtAddr::new(top));
        return Some(rsp);
    }
    s.current[cpu] = None;
    use_boot_space();
    None
}
/// Timer or reschedule entry. Gate handoff finishes on the incoming stack.
pub extern "C" fn on_timer(saved: u64) -> u64 {
    x86_64::instructions::interrupts::without_interrupts(|| on_timer_inner(saved))
}
fn on_timer_inner(saved: u64) -> u64 {
    if saved == 0 {
        return 0;
    }
    let mut s = SCHEDULER.lock();
    let cpu = smp::current_cpu();
    let Some(tid) = s.current[cpu] else {
        return 0;
    };
    if s.ready_queue.is_empty()
        && s.thread(tid)
            .map(|t| t.state == ThreadState::Running)
            .unwrap_or(false)
    {
        // Nothing else can use this CPU. Preserve the current frame and TLB
        // instead of enqueueing, selecting and flushing this same context.
        return 0;
    }
    if let Some(t) = s.thread_mut(tid) {
        if t.state == ThreadState::Running {
            t.saved_kernel_rsp = saved;
            t.state = ThreadState::Ready;
            s.ready_queue.push_back(tid);
        }
    }
    pick_next_locked(&mut s).unwrap_or(IDLE_RSP)
}
/// Save a blocked context or leave an exited thread, then select another.
pub extern "C" fn switch_next(saved: u64) -> u64 {
    x86_64::instructions::interrupts::without_interrupts(|| schedule_inner(saved))
}
fn schedule_inner(saved: u64) -> u64 {
    let mut s = SCHEDULER.lock();
    if saved != 0 {
        if let Some(tid) = s.current_tid() {
            if let Some(t) = s.thread_mut(tid) {
                t.saved_kernel_rsp = saved;
                if !matches!(t.reason, BlockReason::WaitPid(_)) {
                    // SAFETY: Entry supplies this thread's exclusively owned frame.
                    unsafe {
                        (*(saved as *mut SavedContext)).rax = 0;
                    }
                }
            }
        }
    }
    pick_next_locked(&mut s).unwrap_or(0)
}
/// Called only after reaching a CPU-owned idle stack, with the entry gate held.
pub fn idle_next() -> u64 {
    wake_timers();
    reap_retired();
    pick_next_locked(&mut SCHEDULER.lock()).unwrap_or(0)
}
/// PID of the calling CPU's current thread.
pub fn current_pid() -> Option<Pid> {
    SCHEDULER.lock().current_pid()
}
/// Generation-checked TID of the calling CPU's execution context.
pub fn current_tid() -> Option<Tid> {
    SCHEDULER.lock().current_tid()
}

fn set_return(t: &mut Thread, value: u64) {
    // SAFETY: Gate serializes wakeup after a completed save/handoff, scheduler
    // lock owns the blocked thread record; no CPU is restoring this frame.
    unsafe {
        (*(t.saved_kernel_rsp as *mut SavedContext)).rax = value;
    }
}
fn stop_threads_locked(s: &mut Scheduler, pid: Pid) {
    for t in s.threads.iter_mut().flatten().filter(|t| t.pid == pid) {
        t.state = ThreadState::Zombie;
        t.detached = true;
        t.reason = BlockReason::None;
    }
    s.ready_queue.retain(|tid| {
        s.threads
            .get((*tid & 65535) as usize)
            .and_then(Option::as_ref)
            .map(|t| t.pid != pid)
            .unwrap_or(false)
    });
}
fn retire_process(s: &mut Scheduler, pid: Pid) {
    if let Some(process) = s.processes.get_mut(pid as usize).and_then(Option::take) {
        s.retired.push(process);
        crate::serial_println!("[sched] reaped zombie {}", pid);
    }
}
fn wake_waiters_locked(s: &mut Scheduler, pid: Pid) {
    let Some(code) = s
        .processes
        .get(pid as usize)
        .and_then(Option::as_ref)
        .map(|p| p.exit_code)
    else {
        return;
    };
    let mut reaped = false;
    for t in s.threads.iter_mut().flatten() {
        if t.state == ThreadState::Blocked
            && matches!(t.reason,BlockReason::WaitPid(p) if p==pid || p==u32::MAX)
        {
            set_return(t, if s.init_pid.is_some() && reaped { ERROR } else { code });
            t.state = ThreadState::Ready;
            t.reason = BlockReason::None;
            s.ready_queue.push_back(t.tid);
            reaped = true;
        }
    }
    if reaped {
        reap_connection_locked(s, pid);
        retire_process(s, pid);
    }
}
fn stop_process_locked(s: &mut Scheduler, pid: Pid, code: u64) {
    stop_process_with_cause_locked(s, pid, code, rustos_process_abi::supervision::Cause::Exited);
}
fn stop_process_with_cause_locked(s: &mut Scheduler, pid: Pid, code: u64, cause: rustos_process_abi::supervision::Cause) {
    if s.init_pid == Some(pid) { supervision::fatal_init(cause, code); }
    let Some(p) = s.processes.get_mut(pid as usize).and_then(Option::as_mut) else {
        return;
    };
    if p.state == ProcessState::Zombie { return; }
    p.state = ProcessState::Zombie;
    p.files.clear();
    p.exit_code = code;
    p.exit_cause = cause;
    stop_threads_locked(s, pid);
    smp::reschedule_all();
    crate::net::cleanup(pid);
    crate::audio::cleanup(pid);
    stop_owned_child_locked(s, pid);
    supervision::adopt_orphans_locked(s);
    events::wake_locked(s);
    wake_waiters_locked(s, pid);
}
/// Process exit stops every sibling before allowing shared resource reclamation.
pub fn exit_current(code: u64) {
    let mut s = SCHEDULER.lock();
    if let Some(pid) = s.current_pid() {
        crate::serial_println!("[sched] process {} exited with code {}", pid, code);
        stop_process_locked(&mut s, pid, code);
    }
}
/// Stop a PID, including its remote threads. Destruction is deferred until quiescent.
pub fn kill(pid: Pid) -> Option<u64> {
    let mut s = SCHEDULER.lock();
    let target = s.processes.get(pid as usize)?.as_ref()?;
    if s.init_pid.is_some() && s.current_pid() != Some(pid) && target.parent != s.current_pid() { return None; }
    if target.state == ProcessState::Zombie {
        return None;
    }
    let caller = s.current_tid();
    stop_process_with_cause_locked(&mut s, pid, 9, rustos_process_abi::supervision::Cause::Killed);
    crate::serial_println!("[sched] process {} killed", pid);
    Some(
        if caller
            .and_then(|tid| s.thread(tid))
            .map(|t| t.state == ThreadState::Zombie)
            .unwrap_or(caller.is_some())
        {
            PROCESS_BLOCK_SENTINEL
        } else {
            0
        },
    )
}
/// Wait for and reap a process, preserving the existing PID-level ABI.
pub fn waitpid(pid: Pid) -> u64 {
    let mut s = SCHEDULER.lock();
    let Some(tid) = s.current_tid() else {
        return ERROR;
    };
    if s.current_pid() == Some(pid) {
        return ERROR;
    }
    let Some(p) = s.processes.get(pid as usize).and_then(Option::as_ref) else {
        return ERROR;
    };
    if s.init_pid.is_some() && p.parent != s.current_pid() { return ERROR; }
    if p.state == ProcessState::Zombie {
        let code = p.exit_code;
        reap_connection_locked(&mut s, pid);
        retire_process(&mut s, pid);
        return code;
    }
    let t = s.thread_mut(tid).expect("current live thread");
    t.state = ThreadState::Blocked;
    t.reason = BlockReason::WaitPid(pid);
    PROCESS_BLOCK_SENTINEL
}
/// Sleep this thread while siblings and other CPUs continue running.
pub fn sleep_ticks(duration: u64) -> u64 {
    if duration == 0 {
        return 0;
    }
    if duration > i64::MAX as u64 {
        return ERROR;
    }
    let Some(deadline) = crate::interrupts::ticks().checked_add(duration) else {
        return ERROR;
    };
    let mut s = SCHEDULER.lock();
    let Some(tid) = s.current_tid() else {
        return ERROR;
    };
    let Some(t) = s.thread_mut(tid) else {
        return ERROR;
    };
    t.state = ThreadState::Blocked;
    t.reason = BlockReason::Timer(deadline);
    PROCESS_BLOCK_SENTINEL
}
/// Park the calling thread until its PID-owned mailbox is nonempty. Zero
/// timeout waits indefinitely; readiness is a hint, so consumers recheck.
pub fn wait_message(timeout: u64) -> u64 {
    if timeout > i64::MAX as u64 {
        return ERROR;
    }
    let deadline = if timeout == 0 {
        None
    } else {
        let Some(end) = crate::interrupts::ticks().checked_add(timeout) else {
            return ERROR;
        };
        Some(end)
    };
    let mut s = SCHEDULER.lock();
    let Some(pid) = s.current_pid() else {
        return ERROR;
    };
    if s.processes
        .get(pid as usize)
        .and_then(Option::as_ref)
        .map(|p| p.inbox_len > 0)
        .unwrap_or(false)
    {
        return 1;
    }
    let Some(tid) = s.current_tid() else {
        return ERROR;
    };
    let t = s.thread_mut(tid).expect("mailbox caller");
    t.state = ThreadState::Blocked;
    t.reason = BlockReason::Mailbox(deadline);
    PROCESS_BLOCK_SENTINEL
}
/// Wake expired thread waits without allocation or device I/O.
pub fn wake_timers() {
    if crate::interrupts::take_pending_keyboard() {
        wake_keyboard_waiters();
    }
    let now = crate::interrupts::ticks();
    let mut s = SCHEDULER.lock();
    events::wake_locked(&mut s);
    let Scheduler {
        threads,
        ready_queue,
        ..
    } = &mut *s;
    for t in threads
        .iter_mut()
        .flatten()
        .filter(|t| t.state == ThreadState::Blocked)
    {
        let (deadline, result) = match t.reason {
            BlockReason::Timer(d) => (d, 0),
            BlockReason::Word(_, Some(d)) => (d, 2),
            BlockReason::Mailbox(Some(d)) => (d, 0),
            _ => continue,
        };
        if now.wrapping_sub(deadline) as i64 >= 0 {
            set_return(t, result);
            t.state = ThreadState::Ready;
            t.reason = BlockReason::None;
            ready_queue.push_back(t.tid);
        }
    }
}
/// Park only the calling thread for focused keyboard input.
pub fn block_on_keyboard() -> u64 {
    let mut s = SCHEDULER.lock();
    let Some(tid) = s.current_tid() else {
        return ERROR;
    };
    let t = s.thread_mut(tid).expect("current thread");
    t.state = ThreadState::Blocked;
    t.reason = BlockReason::Keyboard;
    PROCESS_BLOCK_SENTINEL
}
/// IRQ-safe wakeup: all queue capacity was reserved at scheduler initialization.
pub fn wake_keyboard_waiters() {
    let mut s = SCHEDULER.lock();
    events::wake_locked(&mut s);
    let Scheduler {
        threads,
        ready_queue,
        ..
    } = &mut *s;
    for t in threads.iter_mut().flatten() {
        if t.state == ThreadState::Blocked && t.reason == BlockReason::Keyboard {
            set_return(t, 0);
            t.state = ThreadState::Ready;
            t.reason = BlockReason::None;
            ready_queue.push_back(t.tid);
        }
    }
}
/// Loads a program binary from the FAT32 filesystem.
///
/// Returns the binary bytes, or an error string.
pub fn load_program_from_disk(path: &str) -> Result<Vec<u8>, &'static str> {
    // Persistent resources such as socket buffers can fragment the heap. Stage
    // only the actual file, rather than demanding a contiguous maximum-size
    // user image on every spawn. Hold the FS lock across metadata and read.
    crate::fs::with_fs(|fs| {
        let size = fs
            .file_size(path)
            .map_err(|_| "failed to query program size")?;
        if size == 0 {
            return Err("program file is empty");
        }
        if size as u64 > rustos_thread_abi::IMAGE_END - userspace::USER_CODE_START {
            return Err("user binary overlaps stack");
        }
        let mut binary = Vec::new();
        binary
            .try_reserve_exact(size)
            .map_err(|_| "not enough memory to stage program")?;
        binary.resize(size, 0);
        let read = fs
            .read_file(path, &mut binary)
            .map_err(|_| "failed to read program file")?;
        if read != size {
            return Err("short program read");
        }
        Ok(binary)
    })
    .ok_or("filesystem not initialized")?
}

/// PID receiving keyboard input; only the compositor can change it.
pub static INPUT_FOCUS: AtomicU64 = AtomicU64::new(0);

/// Whether this process owns keyboard focus.
pub fn has_input_focus() -> bool {
    current_pid().map(u64::from) == Some(INPUT_FOCUS.load(Ordering::Relaxed))
}

/// Sends an opaque scalar message between a client and the display owner.
pub fn send_message(target: u32, payload: u32) -> u64 {
    // The entry gate and scheduler lock serialize the multi-field queue update.
    // No allocation or borrowed user memory is involved.
    let mut sched = SCHEDULER.lock();
    let Some(sender) = sched.current_pid() else {
        return u64::MAX - 1;
    };
    if sender != 0 && target != 0 {
        return u64::MAX - 1;
    }
    let Some(proc) = sched
        .processes
        .get_mut(target as usize)
        .and_then(Option::as_mut)
    else {
        return u64::MAX - 1;
    };
    if proc.state == ProcessState::Zombie {
        return u64::MAX - 1;
    }
    if proc.inbox_len == proc.inbox.len() {
        return 1;
    }
    let tail = (proc.inbox_head + proc.inbox_len) % proc.inbox.len();
    proc.inbox[tail] = ((u64::from(sender) + 1) << 32) | u64::from(payload);
    proc.inbox_len += 1;
    events::wake_locked(&mut sched);
    let Scheduler {
        threads,
        ready_queue,
        ..
    } = &mut *sched;
    for thread in threads.iter_mut().flatten() {
        if thread.pid == target
            && thread.state == ThreadState::Blocked
            && matches!(thread.reason, BlockReason::Mailbox(_))
        {
            set_return(thread, 1);
            thread.state = ThreadState::Ready;
            thread.reason = BlockReason::None;
            ready_queue.push_back(thread.tid);
        }
    }
    0
}

/// Receives an authenticated sender/payload pair, or zero for an empty queue.
pub fn receive_message() -> u64 {
    let mut sched = SCHEDULER.lock();
    let Some(pid) = sched.current_pid() else {
        return u64::MAX - 1;
    };
    let Some(proc) = sched
        .processes
        .get_mut(pid as usize)
        .and_then(Option::as_mut)
    else {
        return u64::MAX - 1;
    };
    if proc.inbox_len == 0 {
        return 0;
    }
    let message = proc.inbox[proc.inbox_head];
    proc.inbox[proc.inbox_head] = 0;
    proc.inbox_head = (proc.inbox_head + 1) % proc.inbox.len();
    proc.inbox_len -= 1;
    message
}

/// Record the coherent revision returned by this process's mouse sample. A
/// concurrent IRQ after the sample remains visible to the next readiness wait.
pub fn observe_pointer(revision: u64) {
    let mut s = SCHEDULER.lock();
    if let Some(pid) = s.current_pid() {
        if let Some(process) = s.processes.get_mut(pid as usize).and_then(Option::as_mut) {
            process.pointer_seen = revision;
        }
    }
    events::wake_locked(&mut s);
}

/// Publishes a completed rendering update from the current application.
pub fn present_surface() -> u64 {
    let mut sched = SCHEDULER.lock();
    let Some(pid) = sched.current_pid() else {
        return u64::MAX - 1;
    };
    let Some(proc) = sched.processes[pid as usize].as_mut() else {
        return u64::MAX - 1;
    };
    if proc.surface.is_empty() {
        return u64::MAX - 1;
    }
    proc.surface_revision = (proc.surface_revision + 1) & ((1u64 << 62) - 1);
    0
}

/// Returns revision * 2 + exit flag, or SYSCALL_ERROR for invalid targets.
/// The dispatcher restricts this metadata query to the display owner.
pub fn surface_status(pid: Pid) -> u64 {
    let sched = SCHEDULER.lock();
    let Some(proc) = sched.processes.get(pid as usize).and_then(Option::as_ref) else {
        return u64::MAX - 1;
    };
    if proc.surface.is_empty() {
        return u64::MAX - 1;
    }
    (proc.surface_revision << 1) | u64::from(proc.state == ProcessState::Zombie)
}

/// Copies a private surface to the compositor. Returns 1 for an exited process.
/// The caller validates the destination and restricts access to PID 0.
pub fn snapshot(pid: Pid, dest: &mut [u8]) -> u64 {
    let sched = SCHEDULER.lock();
    let Some(proc) = sched.processes.get(pid as usize).and_then(Option::as_ref) else {
        return u64::MAX - 1;
    };
    if proc.surface.len() * 4096 < dest.len() {
        return u64::MAX - 1;
    }
    for (chunk, frame) in dest.chunks_mut(4096).zip(&proc.surface) {
        // SAFETY: Surface frames belong to this live PCB; destination is validated user memory.
        unsafe {
            userspace::usercopy::copy_raw(
                chunk.as_mut_ptr(),
                (sched.phys_offset + frame.start_address().as_u64()) as *const u8,
                chunk.len(),
            );
        }
    }
    u64::from(proc.state == ProcessState::Zombie)
}

/// Renders console output into the calling application's private surface.
pub fn surface_write(bytes: &[u8]) {
    use font8x8::{UnicodeFonts, BASIC_FONTS};
    let mut sched = SCHEDULER.lock();
    let offset = sched.phys_offset;
    let Some(pid) = sched.current_pid() else {
        return;
    };
    let Some(proc) = sched.processes[pid as usize].as_mut() else {
        return;
    };
    if proc.surface.is_empty() || !proc.console_enabled {
        return;
    }
    if !bytes.is_empty() {
        proc.surface_revision = (proc.surface_revision + 1) & ((1u64 << 62) - 1);
    }
    let info = crate::drivers::video::framebuffer::fb_info();
    for &byte in bytes {
        if byte == b'\n' || proc.text_cursor.0 + 8 > info.width {
            proc.text_cursor.0 = 8;
            proc.text_cursor.1 += 10;
        }
        if proc.text_cursor.1 + 8 > info.height {
            proc.text_cursor.1 = 8;
        }
        if byte == b'\n' {
            continue;
        }
        if let Some(glyph) = BASIC_FONTS.get(char::from(byte)) {
            for (y, bits) in glyph.iter().enumerate() {
                for x in 0..8 {
                    let index =
                        (proc.text_cursor.1 + y) * info.pitch + (proc.text_cursor.0 + x) * 4;
                    let frame = proc.surface[index / 4096];
                    let ptr = (offset + frame.start_address().as_u64() + (index % 4096) as u64)
                        as *mut u32;
                    let pixel: u32 = if bits & (1 << x) != 0 {
                        0xe1eaf5
                    } else {
                        0x101827
                    };
                    // SAFETY: The gate keeps these owned, aligned surface frames
                    // mapped and in bounds. Assembly forms no Rust reference or
                    // memory-model assumption about concurrent client drawing.
                    unsafe {
                        core::arch::asm!("mov dword ptr [{pointer}],{pixel:e}",pointer=in(reg)ptr,pixel=in(reg)pixel,options(nostack));
                    }
                }
            }
        }
        proc.text_cursor.0 += 8;
    }
}

/// Transfers an application surface from console text to client-side drawing.
pub fn graphics_mode() {
    let mut sched = SCHEDULER.lock();
    if let Some(pid) = sched.current_pid() {
        if let Some(proc) = sched.processes[pid as usize].as_mut() {
            proc.console_enabled = false;
        }
    }
}

/// Copy a surface prefix with stable owned frames and a fully validated user
/// destination. Raw assembly never borrows either application's mutable RAM.
pub fn snapshot_user(pid: Pid, destination: u64, len: usize) -> u64 {
    if !userspace::usercopy::validate(destination, len as u64, true) {
        return ERROR;
    }
    let s = SCHEDULER.lock();
    let Some(p) = s.processes.get(pid as usize).and_then(Option::as_ref) else {
        return ERROR;
    };
    if p.surface.len() * 4096 < len {
        return ERROR;
    }
    let mut written = 0;
    for frame in &p.surface {
        let count = (len - written).min(4096);
        if count == 0 {
            break;
        }
        // SAFETY: The entry gate stabilizes all validated destination pages;
        // this locked PCB owns the complete source frames. Physical surfaces
        // are private and disjoint from the compositor's destination memory.
        // Assembly tolerates client writes without borrowing application RAM.
        unsafe {
            userspace::usercopy::copy_raw(
                (destination + written as u64) as *mut u8,
                (s.phys_offset + frame.start_address().as_u64()) as *const u8,
                count,
            );
        }
        written += count;
    }
    u64::from(p.state == ProcessState::Zombie)
}
/// Idle CPU set requiring a wake after ready work is published under the gate.
pub fn idle_targets() -> u32 {
    let s = SCHEDULER.lock();
    if s.ready_queue.is_empty() {
        return 0;
    }
    let idle = s.current.iter().enumerate().fold(0u32, |mask, (i, t)| {
        mask | if t.is_none() { 1u32 << i } else { 0 }
    });
    placement::wake_targets(
        idle & !(1 << smp::current_cpu()),
        s.ready_queue
            .iter()
            .filter_map(|tid| s.thread(*tid))
            .filter(|t| t.state == ThreadState::Ready)
            .map(|t| t.affinity as u32),
    )
}

/// Whether a user entry still belongs to a live, running execution context.
pub fn current_is_live() -> bool {
    let s = SCHEDULER.lock();
    s.current_tid()
        .and_then(|tid| s.thread(tid))
        .map(|t| {
            t.state == ThreadState::Running
                && s.processes
                    .get(t.pid as usize)
                    .and_then(Option::as_ref)
                    .map(|p| p.state == ProcessState::Live)
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Supervisor compatibility adapter for kernel harnesses. Actual interrupt
/// entry uses switch_next and moves to a CPU-owned idle stack before waiting.
pub extern "C" fn schedule(saved: u64) -> u64 {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let next = schedule_inner(saved);
        if next != 0 {
            return next;
        }
        loop {
            x86_64::instructions::interrupts::enable_and_hlt();
            x86_64::instructions::interrupts::disable();
            wake_timers();
            if let Some(next) = pick_next_locked(&mut SCHEDULER.lock()) {
                return next;
            }
        }
    })
}

mod reclaim;
/// Native thread lifecycle and synchronization mechanisms.
pub mod threads;
pub use reclaim::reap_retired;
pub use threads::{
    create_thread, detach_thread, exit_thread, frame_count, join_thread, thread_pages,
    thread_pointer, wait_word, wake_word, yield_current,
};

/// Access only short process-local filesystem metadata under the scheduler
/// lock. The closure must not take service locks or perform I/O.
pub(crate) fn with_files<R>(f: impl FnOnce(&mut crate::fs::handles::Files) -> R) -> Option<R> {
    let mut scheduler = SCHEDULER.lock();
    let pid = scheduler.current_pid()?;
    let process = scheduler.processes.get_mut(pid as usize)?.as_mut()?;
    if process.state != ProcessState::Live {
        return None;
    }
    Some(f(&mut process.files))
}

/// Process-private program-break memory service.
mod heap;
pub use heap::program_break;

/// Snapshot the current CPU's user extended state exactly once on kernel entry.
/// Entry gate and scheduler lock prevent migration until this context is saved.
pub(crate) fn save_extended_state() {
    let mut scheduler = SCHEDULER.lock();
    if let Some(tid) = scheduler.current_tid() {
        if let Some(thread) = scheduler.thread_mut(tid) {
            // SAFETY: User entry owns this outgoing CPU state; the thread cannot
            // run elsewhere while marked current. Storage is aligned and private.
            unsafe {
                thread.extended.save();
                thread.fs_base = crate::arch::x86_64::extended::fs();
            }
        }
    }
}

/// Irreversibly restrict a connected child before processing untrusted input.
/// Entry gate + scheduler lock serialize the principal against every syscall.
pub fn restrict_worker(profile: u64, bytes: u64, reserved: u64) -> u64 {
    use rustos_process_abi::isolation as abi;
    if profile != abi::WORKER || reserved != 0 || !abi::valid_budget(bytes) {
        return ERROR;
    }
    let mut scheduler = SCHEDULER.lock();
    let Some(pid) = scheduler.current_pid() else {
        return ERROR;
    };
    if pid == 0
        || scheduler
            .threads
            .iter()
            .flatten()
            .filter(|t| t.pid == pid)
            .count()
            != 1
    {
        return ERROR;
    }
    let Some(process) = scheduler
        .processes
        .get_mut(pid as usize)
        .and_then(Option::as_mut)
    else {
        return ERROR;
    };
    if process.worker_heap_limit.is_some()
        || process.io_nested_peer.is_some()
        || !process.io_peer.is_some_and(|(_, child)| child)
        || process.heap_break > rustos_thread_abi::HEAP_BASE + bytes
    {
        return ERROR;
    }
    process.worker_heap_limit = Some(rustos_thread_abi::HEAP_BASE + bytes);
    0
}
/// Check process authority before syscall-specific copying, allocation or I/O.
pub(crate) fn syscall_permitted(number: u64, first: u64) -> bool {
    let scheduler = SCHEDULER.lock();
    let restricted = scheduler
        .current_pid()
        .and_then(|pid| scheduler.processes.get(pid as usize))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.worker_heap_limit.is_some());
    !restricted || rustos_process_abi::isolation::permits(number, first)
}
