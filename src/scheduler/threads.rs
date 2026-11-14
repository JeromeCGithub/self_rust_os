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
    let offset = s.phys_offset;
    let size = (request.stack_size + 4095) & !4095;
    let base = abi::STACK_ARENA + slot as u64 * abi::STACK_STRIDE + 4096;
    let scope = p.principal.scope;
    let native = p.scope_charge.is_some();
    let Scheduler {
        scopes,
        processes,
        frame_allocator,
        ..
    } = &mut *s;
    let template = processes[pid as usize]
        .as_ref()
        .expect("TLS owner")
        .tls
        .as_ref();
    let tls_pages = template.map_or(0, |template| {
        crate::userspace::tls::region(template, slot + 1).2
    });
    let mut budget = if native {
        match scopes.transaction(
            scope,
            rustos_scope_core::Usage {
                threads: 1,
                kernel_bytes: 16384
                    + (tls_pages * core::mem::size_of::<x86_64::structures::paging::PhysFrame>())
                        as u64,
                ..rustos_scope_core::Usage::ZERO
            },
        ) {
            Ok(budget) => Some(budget),
            Err(_) => return ERROR,
        }
    } else {
        None
    };
    let maximum = match budget.as_ref() {
        Some(budget) => match budget
            .available()
            .ok()
            .and_then(|room| usize::try_from(room.pages).ok())
        {
            Some(n) => n,
            None => return ERROR,
        },
        None => usize::MAX,
    };
    if maximum < (size / 4096) as usize + tls_pages {
        return ERROR;
    }
    let Ok(stack) = allocate_kernel_stack() else {
        return ERROR;
    };
    let mut tls_frames = alloc::vec::Vec::new();
    if tls_frames.try_reserve_exact(tls_pages).is_err() {
        return ERROR;
    }
    if let Some(budget) = budget.as_mut() {
        if budget
            .grow(rustos_scope_core::Usage {
                kernel_bytes: ((tls_frames.capacity() - tls_pages)
                    * core::mem::size_of::<x86_64::structures::paging::PhysFrame>())
                    as u64,
                ..rustos_scope_core::Usage::ZERO
            })
            .is_err()
        {
            return ERROR;
        }
    }
    // SAFETY: Root belongs to this process, entry gate excludes other mappers
    // and copies. Every stack slot has preallocated private P1 tables and an
    // unmapped guard. No Rust page-table reference survives this function.
    let table = unsafe { &mut *((offset + root.start_address().as_u64()) as *mut PageTable) };
    let mut mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(offset)) };
    let pool = frame_allocator.as_mut().expect("frame pool");
    // Mappings are detached and shot down on failure before this transaction
    // returns frames; its destructor runs before the resource budget guard.
    let mut transaction = crate::memory::owned::FrameTransaction::bounded(pool, maximum);
    let mut mapped = 0;
    let mut mapped_tls = 0;
    let mut tls_base = 0;
    let mut fs_base = 0;
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
        if let Some(template) = template {
            let (pointer, base) = crate::userspace::tls::map(
                template,
                slot + 1,
                &mut mapper,
                &mut transaction,
                VirtAddr::new(offset),
                &mut mapped_tls,
            )?;
            fs_base = pointer;
            tls_base = base;
        }
        if let Some(budget) = budget.as_mut() {
            budget
                .grow(rustos_scope_core::Usage {
                    pages: transaction.allocated_frames() as u64,
                    kernel_bytes: transaction.metadata_bytes() as u64,
                    ..rustos_scope_core::Usage::ZERO
                })
                .map_err(|_| ())?;
        }
        Ok::<(), ()>(())
    })();
    if result.is_err() {
        if mapped_tls > 0 {
            let base = crate::userspace::tls::region(template.expect("TLS template"), slot + 1).0;
            for index in 0..mapped_tls {
                mapper
                    .unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(
                        base + index as u64 * 4096,
                    )))
                    .expect("partial TLS leaf")
                    .1
                    .ignore();
            }
        }
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
    let mut frames = transaction.commit();
    // Parent tables were preallocated by process construction. Only stack/TLS
    // leaves may be owned here; the TLS ledger was fallibly allocated above.
    assert_eq!(
        frames.len(),
        (size / 4096) as usize + tls_pages,
        "preallocated thread parents"
    );
    tls_frames.extend_from_slice(&frames[(size / 4096) as usize..]);
    frames.truncate((size / 4096) as usize);
    let scope_charge = budget.map(rustos_scope_core::Reservation::commit);
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
        (request.argument, 0, 0),
        mask,
    );
    thread.fs_base = fs_base;
    thread.user_stack = Some(UserStack {
        slot,
        user_base: base,
        frames,
        tls_base,
        tls_frames,
        scope_charge,
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

/// Native arch_prctl subset with Linux ARCH_SET_FS/ARCH_GET_FS operation values.
pub fn thread_pointer(operation: u64, value: u64, reserved: u64) -> u64 {
    if reserved != 0 {
        return ERROR;
    }
    if operation == 0x1002 {
        if value >= abi::USER_END || (value != 0 && !userspace::usercopy::validate(value, 8, false))
        {
            return ERROR;
        }
        let mut scheduler = SCHEDULER.lock();
        let Some(tid) = scheduler.current_tid() else {
            return ERROR;
        };
        let Some(thread) = scheduler.thread_mut(tid) else {
            return ERROR;
        };
        thread.fs_base = value;
        // SAFETY: Canonical validated user mapping or zero; this caller owns the
        // current CPU/thread. Kernel code does not use FS-relative accesses.
        unsafe {
            crate::arch::x86_64::extended::set_fs(value);
        }
        0
    } else if operation == 0x1003 {
        let scheduler = SCHEDULER.lock();
        let Some(tid) = scheduler.current_tid() else {
            return ERROR;
        };
        let Some(thread) = scheduler.thread(tid) else {
            return ERROR;
        };
        let pointer = thread.fs_base;
        drop(scheduler);
        if userspace::usercopy::to_user(value, &pointer.to_le_bytes()) {
            0
        } else {
            ERROR
        }
    } else {
        ERROR
    }
}
