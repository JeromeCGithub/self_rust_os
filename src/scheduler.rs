//! Preemptive round-robin scheduler.
//!
//! This module implements multitasking for self_rust_os. Each process gets:
//! - Its own page table (P4), sharing kernel mappings but with private user mappings.
//! - Its own kernel stack (for interrupts/syscalls).
//! - A Process Control Block (PCB) tracking state, saved registers, and exit code.
//!
//! # Design
//!
//! ## Address spaces
//! User programs are flat binaries linked at `USER_CODE_START` (0x400000).
//! Since they are position-dependent, each process needs its own page table.
//! On process creation, we allocate a new P4 frame, copy all entries from the
//! current P4 (sharing kernel mappings), then clear P4 index 0 (the 512GB
//! region containing all user mappings: binary at 0x400000, stack at
//! 0x2000000, framebuffer at 0x2100000). Fresh user pages are then mapped.
//! The heap (at P4 index 136) is preserved, so kernel heap allocations work
//! in any address space.
//!
//! On context switch, we write the new P4 frame to CR3 (flushing the TLB)
//! and update the TSS RSP0 to the new process's kernel stack.
//!
//! ## Context switching
//! Both the timer IRQ handler and the syscall handler are naked functions
//! that save registers in the same order (rax, rbx, ..., r15), followed by
//! the CPU-pushed interrupt frame (RIP, CS, RFLAGS, RSP, SS). This uniform
//! [`SavedContext`] layout means a context saved by one can be restored by
//! the other.
//!
//! Preemption only happens from user mode. If the timer fires while in
//! kernel mode (e.g., during a syscall), we just increment ticks and return.
//! This avoids preempting code that may hold locks.
//!
//! ## Blocking syscalls
//! `sys_read` (keyboard) and `sys_waitpid` block via the scheduler instead
//! of busy-looping: the process is marked `Blocked`, its context is saved,
//! and another process is scheduled. The keyboard IRQ wakes keyboard-blocked
//! processes; process exit wakes waitpid-blocked processes.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;
use x86_64::{
    registers::control::{Cr3, Cr3Flags},
    structures::paging::{FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB},
    VirtAddr,
};

use crate::{gdt, memory::RegionFrameAllocator, userspace};

/// Process identifier. PIDs are allocated sequentially starting from 0.
pub type Pid = u32;

/// Size of each process's kernel stack (16 KiB).
const KERNEL_STACK_SIZE: usize = 4096 * 4;

/// Timer ticks per time slice. PIT runs at ~18.2Hz (~55ms/tick).
/// Preempt on every tick for a ~55ms quantum.
const TICKS_PER_QUANTUM: u64 = 1;

/// Sentinel: syscall should block the current process and schedule another.
pub const PROCESS_BLOCK_SENTINEL: u64 = u64::MAX - 2;

/// Process states.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcessState {
    /// Ready to run, in the ready queue.
    Ready,
    /// Currently running on the CPU.
    Running,
    /// Blocked waiting for an event (keyboard input, child exit).
    Blocked,
    /// Exited, waiting to be reaped by waitpid.
    Zombie,
}

/// What a blocked process is waiting for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockReason {
    /// Waiting for keyboard input.
    Keyboard,
    /// Waiting for a specific PID to exit (u32::MAX = any child).
    WaitPid(Pid),
    /// Waiting for an absolute PIT tick deadline.
    Timer(u64),
    /// Not blocked.
    None,
}

/// Process Control Block.
pub struct Process {
    /// Process ID.
    pub pid: Pid,
    /// Current state.
    pub state: ProcessState,
    /// Why this process is blocked (if Blocked).
    pub block_reason: BlockReason,
    /// Saved kernel RSP pointing to a [`SavedContext`] on this process's
    /// kernel stack. Valid when state is Ready or Blocked.
    pub kernel_rsp: u64,
    /// Top of this process's kernel stack (for TSS RSP0).
    pub kernel_stack_top: u64,
    /// Physical frame of this process's P4 page table.
    pub p4_frame: PhysFrame,
    /// Exit code (valid when Zombie).
    pub exit_code: u64,
    /// Private application drawing surface (empty for the display owner).
    pub surface: Vec<PhysFrame>,
    /// Text output cursor in pixels.
    pub text_cursor: (usize, usize),
    /// Whether console writes are rendered on this surface.
    pub console_enabled: bool,
    /// Revision of a completed client or console rendering update.
    pub surface_revision: u64,
}

/// Saved CPU context layout.
///
/// Must match the register save order in the naked `timer_entry` and
/// `syscall_entry` handlers: pushes go rax, rbx, rcx, rdx, rsi, rdi, rbp,
/// r8, r9, r10, r11, r12, r13, r14, r15 (so r15 ends up at the lowest
/// address), followed by the CPU-pushed interrupt frame.
#[repr(C)]
pub struct SavedContext {
    /// R15 general-purpose register.
    pub r15: u64,
    /// R14 general-purpose register.
    pub r14: u64,
    /// R13 general-purpose register.
    pub r13: u64,
    /// R12 general-purpose register.
    pub r12: u64,
    /// R11 general-purpose register.
    pub r11: u64,
    /// R10 general-purpose register.
    pub r10: u64,
    /// R9 general-purpose register.
    pub r9: u64,
    /// R8 general-purpose register.
    pub r8: u64,
    /// Base pointer.
    pub rbp: u64,
    /// RDI general-purpose register.
    pub rdi: u64,
    /// RSI general-purpose register.
    pub rsi: u64,
    /// RDX general-purpose register.
    pub rdx: u64,
    /// RCX general-purpose register.
    pub rcx: u64,
    /// RBX general-purpose register.
    pub rbx: u64,
    /// RAX general-purpose register.
    pub rax: u64,
    /// Instruction pointer.
    pub rip: u64,
    /// Code segment selector.
    pub cs: u64,
    /// CPU flags.
    pub rflags: u64,
    /// Stack pointer.
    pub rsp: u64,
    /// Stack segment selector.
    pub ss: u64,
}

/// The scheduler state.
pub struct Scheduler {
    /// Process table, indexed by PID. `None` = PID not in use (reaped).
    processes: Vec<Option<Process>>,
    /// Queue of Ready PIDs (round-robin).
    ready_queue: VecDeque<Pid>,
    /// Currently running PID (if any).
    current: Option<Pid>,
    /// Next PID to allocate.
    next_pid: Pid,
    /// Physical memory offset (for page table manipulation), as raw u64.
    phys_offset: u64,
    /// Frame allocator (for new address spaces).
    frame_allocator: Option<crate::memory::RegionFrameAllocator>,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            processes: Vec::new(),
            ready_queue: VecDeque::new(),
            current: None,
            next_pid: 0,
            phys_offset: 0,
            frame_allocator: None,
        }
    }
}

/// Global scheduler instance.
static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

/// Ticks since last preemption check (for quantum).
static TICKS_SINCE_SWITCH: AtomicU64 = AtomicU64::new(0);

/// Initializes the scheduler.
///
/// Must be called once at boot before creating any processes.
///
/// # Arguments
/// * `phys_offset` - Physical memory offset for page table access.
/// * `frame_allocator` - Frame allocator for new address spaces.
pub fn init(phys_offset: VirtAddr, frame_allocator: RegionFrameAllocator) {
    let mut sched = SCHEDULER.lock();
    sched.phys_offset = phys_offset.as_u64();
    sched.frame_allocator = Some(frame_allocator);
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
    let cur_p2 =
        unsafe { &*((phys_offset + cur_p2_frame.start_address().as_u64()).as_ptr() as *const PageTable) };

    let new_p2_frame = allocator.allocate_frame().ok_or("failed to allocate P2 copy")?;
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
            // Allocate a fresh empty P1 table for user regions.
            let new_p1_frame = allocator.allocate_frame().ok_or("failed to allocate P1 for user")?;
            // SAFETY: Freshly allocated frame, writable via the physical offset mapping.
            // The frame allocator zeroes frames? No — we must clear it manually.
            // Actually, allocate_frame() returns a zeroed frame (frame allocator contract).
            let new_p1 = unsafe {
                &mut *((phys_offset + new_p1_frame.start_address().as_u64()).as_mut_ptr() as *mut PageTable)
            };
            // Ensure it's empty (clear all entries).
            for k in 0..512 {
                new_p1[k] = x86_64::structures::paging::page_table::PageTableEntry::new();
            }
            let p2_flags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE;
            new_p2[j].set_frame(new_p1_frame, p2_flags);
            continue;
        }

        if !new_p2[j].flags().contains(PageTableFlags::PRESENT)
            || new_p2[j].flags().contains(PageTableFlags::HUGE_PAGE)
        {
            continue;
        }
        let cur_p1_frame = new_p2[j].frame().map_err(|_| "P2 entry frame error")?;
        // SAFETY: The frame is a valid P1, readable via the physical offset mapping.
        let cur_p1 =
            unsafe { &*((phys_offset + cur_p1_frame.start_address().as_u64()).as_ptr() as *const PageTable) };
        let new_p1_frame = allocator.allocate_frame().ok_or("failed to allocate P1 copy")?;
        // SAFETY: Freshly allocated frame, writable via the physical offset mapping.
        let new_p1 = unsafe {
            &mut *((phys_offset + new_p1_frame.start_address().as_u64()).as_mut_ptr() as *mut PageTable)
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

/// Creates a new process running the given binary.
///
/// Allocates a fresh address space, maps the binary/stack/framebuffer,
/// sets up the initial context, and adds the process to the ready queue.
///
/// Returns the new PID.
pub fn spawn_process(binary: &[u8], readonly_size: usize) -> Result<Pid, &'static str> {
    // Disable interrupts while holding the scheduler lock to prevent
    // deadlock if the timer IRQ fires during process creation.
    x86_64::instructions::interrupts::without_interrupts(|| {
        spawn_process_inner(binary, readonly_size)
    })
}

fn spawn_process_inner(binary: &[u8], readonly_size: usize) -> Result<Pid, &'static str> {
    if binary.len() as u64 > userspace::USER_STACK_BOTTOM - userspace::USER_CODE_START {
        return Err("user binary overlaps stack");
    }
    let mut sched = SCHEDULER.lock();
    let phys_offset = VirtAddr::new(sched.phys_offset);
    // Read next_pid before the mutable borrow of allocator.
    let is_first_process = sched.next_pid == 0;
    let allocator = sched
        .frame_allocator
        .as_mut()
        .ok_or("scheduler not initialized")?;

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
        for i in 0..512 {
            new_p3[i] = cur_p3[i].clone();
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
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
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
        crate::userspace::process::map_user_binary_via_phys(
            binary,
            readonly_size,
            &mut new_mapper,
            allocator,
            phys_offset,
        )
        .map_err(|_| "failed to map user binary")?;
        crate::userspace::process::map_user_stack_via_phys(&mut new_mapper, allocator, phys_offset)
            .map_err(|_| "failed to map user stack")?;
        let surface = crate::userspace::process::map_user_framebuffer(&mut new_mapper, allocator, is_first_process, phys_offset)
            .map_err(|_| "failed to map framebuffer")?;
        Ok::<Vec<PhysFrame>, &'static str>(surface)
    })();
    let surface = map_result?;

    // 4. Allocate a kernel stack from the heap.
    // The heap is mapped in the kernel half, shared by all address spaces.
    let kstack = alloc::vec![0u8; KERNEL_STACK_SIZE];
    let kstack_box = kstack.into_boxed_slice();
    let kstack_top = kstack_box.as_ptr() as u64 + kstack_box.len() as u64;
    // Leak the stack (processes are not freed in this phase).
    core::mem::forget(kstack_box);

    // 5. Set up the initial SavedContext at the top of the kernel stack.
    let ctx_ptr = (kstack_top - core::mem::size_of::<SavedContext>() as u64) as *mut SavedContext;
    // SAFETY: The kernel stack is valid heap memory, large enough for the context.
    unsafe {
        (*ctx_ptr).r15 = 0;
        (*ctx_ptr).r14 = 0;
        (*ctx_ptr).r13 = 0;
        (*ctx_ptr).r12 = 0;
        (*ctx_ptr).r11 = 0;
        (*ctx_ptr).r10 = 0;
        (*ctx_ptr).r9 = 0;
        (*ctx_ptr).r8 = 0;
        (*ctx_ptr).rbp = 0;
        (*ctx_ptr).rdi = 0;
        (*ctx_ptr).rsi = 0;
        (*ctx_ptr).rdx = 0;
        (*ctx_ptr).rcx = 0;
        (*ctx_ptr).rbx = 0;
        (*ctx_ptr).rax = 0;
        (*ctx_ptr).rip = userspace::USER_CODE_START;
        (*ctx_ptr).cs = u64::from(gdt::user_code_selector().0);
        (*ctx_ptr).rflags = 0x202; // IF set
        (*ctx_ptr).rsp = userspace::USER_STACK_TOP;
        (*ctx_ptr).ss = u64::from(gdt::user_data_selector().0);
    }

    // 6. Create the PCB and enqueue.
    let pid = sched.next_pid;
    sched.next_pid += 1;
    let proc = Process {
        pid,
        state: ProcessState::Ready,
        block_reason: BlockReason::None,
        kernel_rsp: ctx_ptr as u64,
        kernel_stack_top: kstack_top,
        p4_frame,
        exit_code: 0,
        surface,
        text_cursor: (8, 8),
        console_enabled: true,
        surface_revision: 0,
    };
    // Ensure the vector is large enough.
    while sched.processes.len() <= pid as usize {
        sched.processes.push(None);
    }
    sched.processes[pid as usize] = Some(proc);
    // IRQ wakeups must never grow the queue. At most one entry per live
    // process is queued; reserve for the entire table in this non-IRQ path.
    let capacity = sched.processes.len();
    let queued = sched.ready_queue.len();
    sched.ready_queue.reserve(capacity.saturating_sub(queued));
    sched.ready_queue.push_back(pid);

    crate::serial_println!("[sched] spawned process {} ({} bytes)", pid, binary.len());
    Ok(pid)
}

/// Starts the scheduler by switching to the first ready process.
///
/// Never returns.
pub fn start() -> ! {
    crate::console::DESKTOP_ACTIVE.store(true, Ordering::Relaxed);
    let new_rsp = {
        let mut sched = SCHEDULER.lock();
        let pid = sched
            .ready_queue
            .pop_front()
            .expect("scheduler: no processes to run");
        // Extract the fields we need, then release the borrow.
        let (p4_frame, kernel_stack_top, kernel_rsp) = {
            let pcb = sched.processes[pid as usize]
                .as_mut()
                .expect("scheduler: invalid PID");
            pcb.state = ProcessState::Running;
            (pcb.p4_frame, pcb.kernel_stack_top, pcb.kernel_rsp)
        };
        sched.current = Some(pid);
        // Switch to the process's address space.
        // SAFETY: p4_frame is a valid P4 we created.
        unsafe {
            Cr3::write(p4_frame, Cr3Flags::empty());
        }
        // Use the process's kernel stack for interrupts.
        gdt::set_kernel_stack(VirtAddr::new(kernel_stack_top));
        kernel_rsp
    };

    // Switch to the process's kernel stack and restore its context.
    // Disable interrupts during the switch; the iretq frame has IF set,
    // so interrupts will be enabled when we enter user mode.
    x86_64::instructions::interrupts::disable();
    unsafe {
        core::arch::asm!(
            "mov rsp, {0}",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop r11",
            "pop r10",
            "pop r9",
            "pop r8",
            "pop rbp",
            "pop rdi",
            "pop rsi",
            "pop rdx",
            "pop rcx",
            "pop rbx",
            "pop rax",
            "iretq",
            in(reg) new_rsp,
            options(noreturn, nostack)
        );
    }
}

/// Called from the timer interrupt handler (naked asm).
///
/// # Arguments
/// * `save_rsp` - Kernel RSP pointing to the saved context, or 0 if the
///   interrupt came from kernel mode (no preemption).
///
/// # Returns
/// New kernel RSP to switch to, or 0 to resume the current context.
pub extern "C" fn on_timer(save_rsp: u64) -> u64 {
    if save_rsp == 0 {
        // From kernel mode: no preemption.
        return 0;
    }

    // Quantum check.
    let elapsed = TICKS_SINCE_SWITCH.fetch_add(1, Ordering::Relaxed) + 1;
    if elapsed < TICKS_PER_QUANTUM {
        return 0;
    }
    TICKS_SINCE_SWITCH.store(0, Ordering::Relaxed);

    let mut sched = SCHEDULER.lock();

    // Only preempt if there's someone else to run.
    if sched.ready_queue.is_empty() {
        return 0;
    }

    let current = match sched.current {
        Some(pid) => pid,
        None => return 0,
    };

    // Save the current process's context and requeue it.
    if let Some(pcb) = sched
        .processes
        .get_mut(current as usize)
        .and_then(|p| p.as_mut())
    {
        if pcb.state == ProcessState::Running {
            pcb.kernel_rsp = save_rsp;
            pcb.state = ProcessState::Ready;
            sched.ready_queue.push_back(current);
        } else {
            // Current isn't Running (shouldn't happen from user mode).
            return 0;
        }
    }

    match pick_next_locked(&mut sched) {
        Some(new_rsp) => new_rsp,
        None => 0,
    }
}

/// Called from the syscall handler when a process exits or blocks.
///
/// # Arguments
/// * `save_rsp` - Kernel RSP of the saved context. If 0, the current process
///   is dead (Zombie) and its context is not saved. Otherwise, the process
///   is blocking and its context is saved for later resumption.
///
/// # Returns
/// Kernel RSP of the next process to run. Never returns 0; if no process
/// is runnable, idles (hlt) until one wakes.
pub extern "C" fn schedule(save_rsp: u64) -> u64 {
    // Save the current process's context if it's blocking (not dead).
    if save_rsp != 0 {
        let mut sched = SCHEDULER.lock();
        if let Some(pid) = sched.current {
            if let Some(pcb) = sched
                .processes
                .get_mut(pid as usize)
                .and_then(|p| p.as_mut())
            {
                // Only save if still marked Running (syscalls set Blocked before).
                // Actually, blockers set state before returning sentinel, so we
                // just save the RSP here.
                pcb.kernel_rsp = save_rsp;
                if matches!(pcb.block_reason, BlockReason::Timer(_)) {
                    // SAFETY: save_rsp is the aligned SavedContext on this
                    // process's live kernel stack, supplied by syscall_entry.
                    // IRQs are disabled and no other context accesses it.
                    unsafe { (*(save_rsp as *mut SavedContext)).rax = 0; }
                }
            }
        }
    }
    // Else: process is a Zombie, don't save.

    loop {
        {
            let mut sched = SCHEDULER.lock();
            if let Some(new_rsp) = pick_next_locked(&mut sched) {
                TICKS_SINCE_SWITCH.store(0, Ordering::Relaxed);
                return new_rsp;
            }
        }
        // No runnable process: idle with interrupts enabled until one wakes.
        // (e.g., keyboard IRQ will wake a process blocked on input)
        // STI's interrupt shadow makes enabling + halting indivisible with
        // respect to IRQ delivery, avoiding a wakeup between enable and HLT.
        x86_64::instructions::interrupts::enable_and_hlt();
        x86_64::instructions::interrupts::disable();
    }
}

/// Picks the next Ready process from the queue.
///
/// Switches CR3 and TSS RSP0. Returns the new process's saved kernel RSP.
/// Returns None if no Ready process is available.
fn pick_next_locked(sched: &mut Scheduler) -> Option<u64> {
    while let Some(pid) = sched.ready_queue.pop_front() {
        if let Some(pcb) = sched
            .processes
            .get_mut(pid as usize)
            .and_then(|p| p.as_mut())
        {
            if pcb.state == ProcessState::Ready {
                pcb.state = ProcessState::Running;
                sched.current = Some(pid);
                // Switch address space.
                // SAFETY: p4_frame is a valid P4 created by spawn_process.
                unsafe {
                    Cr3::write(pcb.p4_frame, Cr3Flags::empty());
                }
                // Switch kernel stack for interrupts.
                gdt::set_kernel_stack(VirtAddr::new(pcb.kernel_stack_top));
                return Some(pcb.kernel_rsp);
            }
        }
        // Skip stale entries (Zombie/Blocked).
    }
    None
}

/// Marks the current process as exited with the given code.
///
/// Wakes any processes blocked in waitpid on this PID.
/// The caller must then invoke `schedule(0)` to switch away.
pub fn exit_current(code: u64) {
    let mut sched = SCHEDULER.lock();
    let current = match sched.current {
        Some(pid) => pid,
        None => return, // No scheduler running (e.g., unit test); just return.
    };
    if let Some(pcb) = sched
        .processes
        .get_mut(current as usize)
        .and_then(|p| p.as_mut())
    {
        pcb.state = ProcessState::Zombie;
        pcb.exit_code = code;
        crate::serial_println!("[sched] process {} exited with code {}", current, code);
    }
    crate::net::cleanup(current);
    wake_waiters_locked(&mut sched, current);
    // Remove from ready queue (shouldn't be there, but be safe).
    sched.ready_queue.retain(|&pid| pid != current);
}

/// Terminates a process by PID.
///
/// If the target is the current process, marks it Zombie (caller must schedule away).
/// Returns the exit code on success, or None if the PID is invalid.
pub fn kill(pid: Pid) -> Option<u64> {
    let mut sched = SCHEDULER.lock();
    let current = sched.current;
    
    let pcb = sched
        .processes
        .get_mut(pid as usize)
        .and_then(|p| p.as_mut())?;
    if pcb.state == ProcessState::Zombie {
        return None;
    }
    
    pcb.state = ProcessState::Zombie;
    pcb.exit_code = 9; // SIGKILL
    crate::net::cleanup(pid);
    wake_waiters_locked(&mut sched, pid);
    sched.ready_queue.retain(|&p| p != pid);
    
    crate::serial_println!("[sched] process {} killed", pid);
    
    if Some(pid) == current {
        // Killing self: caller must schedule away (return sentinel).
        Some(PROCESS_BLOCK_SENTINEL) // Special marker: don't save, just schedule
    } else {
        Some(0)
    }
}

/// Blocks the current process waiting for a PID to exit.
///
/// If the target is already a Zombie, reaps it immediately and returns the
/// exit code. Otherwise marks the current process Blocked and returns
/// `PROCESS_BLOCK_SENTINEL` (caller must invoke the scheduler).
pub fn waitpid(pid: Pid) -> u64 {
    let mut sched = SCHEDULER.lock();
    let current = sched.current.expect("waitpid: no current process");
    
    // Validate target.
    let target_state = match sched.processes.get(pid as usize).and_then(|p| p.as_ref()) {
        Some(pcb) => pcb.state,
        None => return u64::MAX - 1, // SYSCALL_ERROR: invalid PID
    };
    
    if target_state == ProcessState::Zombie {
        // Reap immediately.
        let code = sched.processes[pid as usize].as_ref().unwrap().exit_code;
        sched.processes[pid as usize] = None;
        crate::serial_println!("[sched] reaped zombie {}", pid);
        return code;
    }
    
    // Block the current process.
    if let Some(pcb) = sched
        .processes
        .get_mut(current as usize)
        .and_then(|p| p.as_mut())
    {
        pcb.state = ProcessState::Blocked;
        pcb.block_reason = BlockReason::WaitPid(pid);
        crate::serial_println!("[sched] process {} waiting for {}", current, pid);
    }
    PROCESS_BLOCK_SENTINEL
}

/// Blocks for a duration in PIT ticks. Zero returns immediately; overflow fails.
pub fn sleep_ticks(duration: u64) -> u64 {
    if duration == 0 { return 0; }
    if duration > i64::MAX as u64 { return u64::MAX - 1; }
    let Some(deadline) = crate::interrupts::ticks().checked_add(duration) else {
        return u64::MAX - 1;
    };
    let mut sched = SCHEDULER.lock();
    let Some(current) = sched.current else { return u64::MAX - 1; };
    let Some(pcb) = sched.processes[current as usize].as_mut() else { return u64::MAX - 1; };
    pcb.state = ProcessState::Blocked;
    pcb.block_reason = BlockReason::Timer(deadline);
    PROCESS_BLOCK_SENTINEL
}

/// Wakes expired timers, including IRQs that interrupt the scheduler's idle loop.
/// Called with interrupts disabled on this single-CPU kernel. No allocation or
/// I/O occurs here; spawn reserves queue storage for every possible live entry.
pub fn wake_timers() {
    let now = crate::interrupts::ticks();
    let mut sched = SCHEDULER.lock();
    let Scheduler { processes, ready_queue, .. } = &mut *sched;
    for pcb in processes.iter_mut().flatten() {
        if pcb.state == ProcessState::Blocked {
            if let BlockReason::Timer(deadline) = pcb.block_reason {
                // Signed wrapping subtraction also handles tick-counter wrap.
                if now.wrapping_sub(deadline) as i64 >= 0 {
                    pcb.state = ProcessState::Ready;
                    pcb.block_reason = BlockReason::None;
                    ready_queue.push_back(pcb.pid);
                }
            }
        }
    }
}

/// Blocks the current process waiting for keyboard input.
///
/// Called from sys_read when no scancode is available.
/// Returns `PROCESS_BLOCK_SENTINEL`.
pub fn block_on_keyboard() -> u64 {
    let mut sched = SCHEDULER.lock();
    let current = sched.current.expect("block_on_keyboard: no current");
    if let Some(pcb) = sched
        .processes
        .get_mut(current as usize)
        .and_then(|p| p.as_mut())
    {
        pcb.state = ProcessState::Blocked;
        pcb.block_reason = BlockReason::Keyboard;
    }
    PROCESS_BLOCK_SENTINEL
}

/// Wakes processes blocked on keyboard input.
///
/// Called from the keyboard interrupt handler.
pub fn wake_keyboard_waiters() {
    let mut sched = SCHEDULER.lock();
    let mut to_wake = Vec::new();
    for (idx, pcb_opt) in sched.processes.iter_mut().enumerate() {
        if let Some(pcb) = pcb_opt {
            if pcb.state == ProcessState::Blocked && pcb.block_reason == BlockReason::Keyboard {
                pcb.state = ProcessState::Ready;
                pcb.block_reason = BlockReason::None;
                to_wake.push(idx as Pid);
            }
        }
    }
    for pid in to_wake {
        sched.ready_queue.push_back(pid);
    }
}

/// Moves Blocked processes waiting on `exited_pid` back to Ready.
fn wake_waiters_locked(sched: &mut Scheduler, exited_pid: Pid) {
    let Some(exit_code) = sched.processes.get(exited_pid as usize).and_then(Option::as_ref).map(|pcb| pcb.exit_code) else { return; };
    let mut reaped = false;
    for (idx, pcb_opt) in sched.processes.iter_mut().enumerate() {
        if let Some(pcb) = pcb_opt {
            if pcb.state == ProcessState::Blocked {
                let wake = match pcb.block_reason {
                    BlockReason::WaitPid(target) => target == exited_pid || target == u32::MAX,
                    _ => false,
                };
                if wake {
                    // SAFETY: A blocked waitpid context was saved on this live
                    // process's aligned kernel stack before another process could
                    // exit. IRQs are disabled, the scheduler lock is held and no
                    // other CPU or reference accesses the saved register frame.
                    unsafe { (*(pcb.kernel_rsp as *mut SavedContext)).rax = exit_code; }
                    reaped = true;
                    pcb.state = ProcessState::Ready;
                    pcb.block_reason = BlockReason::None;
                    sched.ready_queue.push_back(idx as Pid);
                    crate::serial_println!(
                        "[sched] woke process {} (waited for {})",
                        idx,
                        exited_pid
                    );
                }
            }
        }
    }
    if reaped { sched.processes[exited_pid as usize] = None; }
}

/// Returns the PID of the current process, if any.
pub fn current_pid() -> Option<Pid> {
    SCHEDULER.lock().current
}

/// Loads a program binary from the FAT32 filesystem.
///
/// Returns the binary bytes, or an error string.
pub fn load_program_from_disk(path: &str) -> Result<Vec<u8>, &'static str> {
    // Persistent resources such as socket buffers can fragment the heap. Stage
    // only the actual file, rather than demanding a contiguous maximum-size
    // user image on every spawn. Hold the FS lock across metadata and read.
    crate::fs::with_fs(|fs| {
        let size = fs.file_size(path).map_err(|_| "failed to query program size")?;
        if size == 0 { return Err("program file is empty"); }
        if size as u64 > userspace::USER_STACK_BOTTOM - userspace::USER_CODE_START {
            return Err("user binary overlaps stack");
        }
        let mut binary = Vec::new();
        binary.try_reserve_exact(size).map_err(|_| "not enough memory to stage program")?;
        binary.resize(size, 0);
        let read = fs.read_file(path, &mut binary).map_err(|_| "failed to read program file")?;
        if read != size { return Err("short program read"); }
        Ok(binary)
    }).ok_or("filesystem not initialized")?
}

/// PID receiving keyboard input; only the compositor can change it.
pub static INPUT_FOCUS: AtomicU64 = AtomicU64::new(0);

/// Whether this process owns keyboard focus.
pub fn has_input_focus() -> bool {
    current_pid().map(u64::from) == Some(INPUT_FOCUS.load(Ordering::Relaxed))
}

/// Publishes a completed rendering update from the current application.
pub fn present_surface() -> u64 {
    let mut sched = SCHEDULER.lock();
    let Some(pid) = sched.current else { return u64::MAX - 1; };
    let Some(proc) = sched.processes[pid as usize].as_mut() else { return u64::MAX - 1; };
    if proc.surface.is_empty() { return u64::MAX - 1; }
    proc.surface_revision = (proc.surface_revision + 1) & ((1u64 << 62) - 1);
    0
}

/// Returns revision * 2 + exit flag, or SYSCALL_ERROR for invalid targets.
/// The dispatcher restricts this metadata query to the display owner.
pub fn surface_status(pid: Pid) -> u64 {
    let sched = SCHEDULER.lock();
    let Some(proc) = sched.processes.get(pid as usize).and_then(Option::as_ref) else { return u64::MAX - 1; };
    if proc.surface.is_empty() { return u64::MAX - 1; }
    (proc.surface_revision << 1) | u64::from(proc.state == ProcessState::Zombie)
}

/// Copies a private surface to the compositor. Returns 1 for an exited process.
/// The caller validates the destination and restricts access to PID 0.
pub fn snapshot(pid: Pid, dest: &mut [u8]) -> u64 {
    let sched = SCHEDULER.lock();
    let Some(proc) = sched.processes.get(pid as usize).and_then(Option::as_ref) else { return u64::MAX - 1; };
    if proc.surface.len() * 4096 < dest.len() { return u64::MAX - 1; }
    for (chunk, frame) in dest.chunks_mut(4096).zip(&proc.surface) {
        // SAFETY: Surface frames belong to this live PCB; destination is validated user memory.
        unsafe { core::ptr::copy_nonoverlapping((sched.phys_offset + frame.start_address().as_u64()) as *const u8, chunk.as_mut_ptr(), chunk.len()); }
    }
    u64::from(proc.state == ProcessState::Zombie)
}

/// Renders console output into the calling application's private surface.
pub fn surface_write(bytes: &[u8]) {
    use font8x8::{UnicodeFonts, BASIC_FONTS};
    let mut sched = SCHEDULER.lock();
    let offset = sched.phys_offset;
    let Some(pid) = sched.current else { return; };
    let Some(proc) = sched.processes[pid as usize].as_mut() else { return; };
    if proc.surface.is_empty() || !proc.console_enabled { return; }
    if !bytes.is_empty() {
        proc.surface_revision = (proc.surface_revision + 1) & ((1u64 << 62) - 1);
    }
    let info = crate::drivers::video::framebuffer::fb_info();
    for &byte in bytes {
        if byte == b'\n' || proc.text_cursor.0 + 8 > info.width {
            proc.text_cursor.0 = 8;
            proc.text_cursor.1 += 10;
        }
        if proc.text_cursor.1 + 8 > info.height { proc.text_cursor.1 = 8; }
        if byte == b'\n' { continue; }
        if let Some(glyph) = BASIC_FONTS.get(char::from(byte)) {
            for (y, bits) in glyph.iter().enumerate() {
                for x in 0..8 {
                    let index = (proc.text_cursor.1 + y) * info.pitch + (proc.text_cursor.0 + x) * 4;
                    let frame = proc.surface[index / 4096];
                    let ptr = (offset + frame.start_address().as_u64() + (index % 4096) as u64) as *mut u32;
                    // SAFETY: Coordinates are within the private surface frames.
                    unsafe { ptr.write(if bits & (1 << x) != 0 { 0xe1eaf5 } else { 0x101827 }); }
                }
            }
        }
        proc.text_cursor.0 += 8;
    }
}

/// Transfers an application surface from console text to client-side drawing.
pub fn graphics_mode() {
    let mut sched = SCHEDULER.lock();
    if let Some(pid) = sched.current {
        if let Some(proc) = sched.processes[pid as usize].as_mut() { proc.console_enabled = false; }
    }
}
