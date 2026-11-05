//! Native thread lifecycle, guarded stack creation and process-private parking.
use super::{
    allocate_kernel_stack, make_thread, set_return, smp, stop_process_locked, userspace,
    BlockReason, ProcessState, Scheduler, ThreadState, Tid, ERROR, PROCESS_BLOCK_SENTINEL,
    SCHEDULER,
};
use crate::process::UserStack;
use core::sync::atomic::Ordering;
use rustos_thread_abi as abi;
use x86_64::{
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};
/// Map a guarded stack and create a native thread sharing the caller's PID.
pub fn create_thread(request: abi::Create) -> u64 {
    if !abi::valid(&request, smp::online_count())
        || !userspace::usercopy::validate(request.entry, 1, false)
    {
        return ERROR;
    }
    let mut s = SCHEDULER.lock();
    let Some(pid) = s.current_pid() else {
        return ERROR;
    };
    if s.threads.iter().flatten().filter(|t| t.pid == pid).count() >= 64 {
        return ERROR;
    }
    let Some(p) = s.processes.get(pid as usize).and_then(Option::as_ref) else {
        return ERROR;
    };
    if p.state == ProcessState::Zombie {
        return ERROR;
    }
    let Some(slot) = (0..abi::STACK_SLOTS).find(|i| p.stack_slots & (1u64 << i) == 0) else {
        return ERROR;
    };
    let root = p.p4_frame;
    let Ok(tid) = s.reserve_tid() else {
        return ERROR;
    };
    let Ok(stack) = allocate_kernel_stack() else {
        return ERROR;
    };
    let offset = s.phys_offset;
    let size = (request.stack_size + 4095) & !4095;
    let base = abi::STACK_ARENA + slot as u64 * abi::STACK_STRIDE + 4096;
    // SAFETY: Root belongs to this process, entry gate excludes other mappers
    // and copies. Every stack slot has preallocated private P1 tables and an
    // unmapped guard. No Rust page-table reference survives this function.
    let table = unsafe { &mut *((offset + root.start_address().as_u64()) as *mut PageTable) };
    let mut mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(offset)) };
    let pool = s.frame_allocator.as_mut().expect("frame pool");
    let mut transaction = crate::memory::owned::FrameTransaction::new(pool);
    let mut mapped = 0;
    let result = (|| {
        for i in 0..size / 4096 {
            let frame = transaction.allocate_frame().ok_or(())?;
            // SAFETY: Fresh owned frame; direct-map destination is valid RAM,
            // 4 KiB in bounds, and has never been exposed to application access.
            unsafe {
                core::ptr::write_bytes(
                    (offset + frame.start_address().as_u64()) as *mut u8,
                    0,
                    4096,
                );
            }
            core::sync::atomic::fence(Ordering::Release);
            // SAFETY: Distinct reserved user stack page, unique fresh frame,
            // private preallocated page-table parents and serialized mutation.
            unsafe {
                mapper
                    .map_to(
                        Page::<Size4KiB>::containing_address(VirtAddr::new(base + i * 4096)),
                        frame,
                        PageTableFlags::PRESENT
                            | PageTableFlags::WRITABLE
                            | PageTableFlags::USER_ACCESSIBLE,
                        &mut transaction,
                    )
                    .map_err(|_| ())?
                    .ignore();
            }
            mapped += 1;
        }
        Ok::<(), ()>(())
    })();
    if result.is_err() {
        for i in 0..mapped {
            mapper
                .unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(
                    base + i * 4096,
                )))
                .expect("partial stack leaf")
                .1
                .ignore();
        }
        drop(mapper);
        smp::shootdown();
        return ERROR; // transaction returns every unpublished allocation.
    }
    drop(mapper);
    let frames = transaction.commit();
    smp::shootdown();
    let mask = if request.affinity == 0 {
        (1u64 << smp::online_count()) - 1
    } else {
        request.affinity
    };
    let mut thread = make_thread(
        tid,
        pid,
        stack,
        request.entry,
        base + size - 8,
        request.argument,
        mask,
    );
    thread.user_stack = Some(UserStack {
        slot,
        user_base: base,
        frames,
    });
    s.processes[pid as usize]
        .as_mut()
        .expect("live process")
        .stack_slots |= 1u64 << slot;
    s.insert(thread);
    tid
}
/// Finish this thread and publish join completion. Last-thread exit ends the PID.
pub fn exit_thread() {
    let mut s = SCHEDULER.lock();
    let Some(tid) = s.current_tid() else {
        return;
    };
    let Some(t) = s.thread_mut(tid) else {
        return;
    };
    t.state = ThreadState::Zombie;
    let pid = t.pid;
    let joiner = t.joiner;
    if let Some(joiner) = joiner {
        if let Some(t) = s.thread_mut(joiner) {
            if t.state == ThreadState::Blocked && t.reason == BlockReason::Join(tid) {
                set_return(t, 0);
                t.state = ThreadState::Ready;
                t.reason = BlockReason::None;
                s.ready_queue.push_back(joiner);
                s.thread_mut(tid).expect("finished thread").detached = true;
            }
        }
    }
    if !s
        .threads
        .iter()
        .flatten()
        .any(|t| t.pid == pid && t.state != ThreadState::Zombie)
    {
        stop_process_locked(&mut s, pid, 0);
    }
}
/// Same-process, single-consumer join. No user pointers are retained while asleep.
pub fn join_thread(target: Tid) -> u64 {
    let mut s = SCHEDULER.lock();
    let Some(caller) = s.current_tid() else {
        return ERROR;
    };
    if caller == target {
        return ERROR;
    }
    let Some(pid) = s.current_pid() else {
        return ERROR;
    };
    let Some(t) = s.thread_mut(target) else {
        return ERROR;
    };
    if t.pid != pid || t.detached || t.joiner.is_some() {
        return ERROR;
    }
    if t.state == ThreadState::Zombie {
        t.detached = true;
        return 0;
    }
    t.joiner = Some(caller);
    let caller = s.thread_mut(caller).expect("join caller");
    caller.state = ThreadState::Blocked;
    caller.reason = BlockReason::Join(target);
    PROCESS_BLOCK_SENTINEL
}
/// Give up join ownership, allowing completion metadata and stacks to be reclaimed.
pub fn detach_thread(target: Tid) -> u64 {
    let mut s = SCHEDULER.lock();
    let Some(pid) = s.current_pid() else {
        return ERROR;
    };
    let Some(t) = s.thread_mut(target) else {
        return ERROR;
    };
    if t.pid != pid || t.detached || t.joiner.is_some() {
        return ERROR;
    }
    t.detached = true;
    0
}
/// Make this running thread ready and switch through the normal saved-frame path.
pub fn yield_current() -> u64 {
    let mut s = SCHEDULER.lock();
    let Some(tid) = s.current_tid() else {
        return ERROR;
    };
    let t = s.thread_mut(tid).expect("yield caller");
    t.state = ThreadState::Ready;
    t.reason = BlockReason::None;
    s.ready_queue.push_back(tid);
    PROCESS_BLOCK_SENTINEL
}
/// Compare an aligned user atomic word and park atomically with respect to wake.
pub fn wait_word(address: u64, expected: u64, timeout: u64) -> u64 {
    if address & 3 != 0
        || expected > u32::MAX as u64
        || timeout > i64::MAX as u64
        || !userspace::usercopy::validate(address, 4, false)
    {
        return ERROR;
    }
    let deadline = if timeout == 0 {
        None
    } else {
        match crate::interrupts::ticks().checked_add(timeout) {
            Some(d) => Some(d),
            None => return ERROR,
        }
    };
    let mut s = SCHEDULER.lock();
    let Some(tid) = s.current_tid() else {
        return ERROR;
    };
    let value: u32;
    // SAFETY: Complete aligned user mapping is stable under the entry gate.
    // Assembly performs a single x86 acquire load with no Rust reference into
    // memory concurrently modified by sibling user threads. No typed validity
    // restriction exists for u32, and the compiler sees a memory barrier.
    unsafe {
        core::arch::asm!("mov {value:e},dword ptr [{address}]",value=out(reg)value,address=in(reg)address,options(nostack,readonly));
    }
    if value != expected as u32 {
        return 1;
    }
    let t = s.thread_mut(tid).expect("wait caller");
    t.state = ThreadState::Blocked;
    t.reason = BlockReason::Word(address, deadline);
    PROCESS_BLOCK_SENTINEL
}
/// Wake process-private waiters. Equal numeric addresses in other PIDs are separate.
pub fn wake_word(address: u64, count: u64) -> u64 {
    if address & 3 != 0 || count > 1024 || !userspace::usercopy::validate(address, 4, false) {
        return ERROR;
    }
    let mut s = SCHEDULER.lock();
    let Some(pid) = s.current_pid() else {
        return ERROR;
    };
    let Scheduler {
        threads,
        ready_queue,
        ..
    } = &mut *s;
    let mut woken = 0;
    for t in threads.iter_mut().flatten() {
        if woken < count
            && t.pid == pid
            && t.state == ThreadState::Blocked
            && matches!(t.reason,BlockReason::Word(a,_) if a==address)
        {
            set_return(t, 0);
            t.state = ThreadState::Ready;
            t.reason = BlockReason::None;
            ready_queue.push_back(t.tid);
            woken += 1;
        }
    }
    woken
}
/// Allocated worker-stack pages in this process, for lifecycle diagnostics.
pub fn thread_pages() -> u64 {
    let s = SCHEDULER.lock();
    let Some(pid) = s.current_pid() else {
        return ERROR;
    };
    s.threads
        .iter()
        .flatten()
        .filter(|t| t.pid == pid)
        .filter_map(|t| t.user_stack.as_ref())
        .map(|stack| stack.frames.len() as u64)
        .sum()
}
/// Physical frames currently owned by the scheduler's allocator.
pub fn frame_count() -> u64 {
    SCHEDULER
        .lock()
        .frame_allocator
        .as_ref()
        .map(|f| f.allocated() as u64)
        .unwrap_or(ERROR)
}
