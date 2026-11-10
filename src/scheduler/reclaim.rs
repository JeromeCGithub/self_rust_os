//! Deferred stack/address-space reclamation after CPU and TLB quiescence.
use super::{set_return, smp, BlockReason, Scheduler, ThreadState, ERROR, SCHEDULER};
use crate::process::UserStack;
use x86_64::{
    structures::paging::{Mapper, OffsetPageTable, Page, PageTable, Size4KiB},
    VirtAddr,
};
/// Reclaim only off-CPU stacks and quiescent retired address spaces.
/// Called on syscall entry or a CPU-owned idle stack, never in a hard IRQ.
pub fn reap_retired() {
    let mut s = SCHEDULER.lock();
    let Scheduler {
        resources, scopes, ..
    } = &mut *s;
    resources.collect(scopes);
    loop {
        let dead = s
            .threads
            .iter()
            .flatten()
            .find(|t| {
                t.state == ThreadState::Zombie && t.detached && !s.current.contains(&Some(t.tid))
            })
            .map(|t| t.tid);
        let Some(tid) = dead else {
            break;
        };
        let mut t = s.threads[(tid & 65535) as usize]
            .take()
            .expect("retired thread");
        let mut thread_charge = None;
        if let Some(UserStack {
            slot,
            user_base: base,
            frames,
            tls_base,
            tls_frames,
            scope_charge,
        }) = t.user_stack.take()
        {
            let root = s
                .processes
                .get(t.pid as usize)
                .and_then(Option::as_ref)
                .or_else(|| s.retired.iter().find(|p| p.pid == t.pid))
                .expect("thread address-space owner")
                .p4_frame;
            let offset = s.phys_offset;
            {
                // SAFETY: Entry gate excludes all other page-table mutation or
                // user copies; this temporary mapper is the only mutable Rust
                // reference to this owned root and is dropped before shootdown.
                let table =
                    unsafe { &mut *((offset + root.start_address().as_u64()) as *mut PageTable) };
                let mut mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(offset)) };
                for (i, frame) in tls_frames.iter().enumerate() {
                    let (unmapped, flush) = mapper
                        .unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(
                            tls_base + i as u64 * 4096,
                        )))
                        .expect("owned TLS mapping");
                    assert_eq!(*frame, unmapped);
                    flush.ignore();
                }
                for (i, frame) in frames.iter().enumerate() {
                    let (unmapped, flush) = mapper
                        .unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(
                            base + i as u64 * 4096,
                        )))
                        .expect("owned thread mapping");
                    assert_eq!(*frame, unmapped);
                    flush.ignore();
                }
            }
            // Cancel wait keys before the same virtual address can be reused.
            let end = base + frames.len() as u64 * 4096;
            let Scheduler {
                threads,
                ready_queue,
                ..
            } = &mut *s;
            for waiter in threads.iter_mut().flatten() {
                if waiter.pid == t.pid
                    && waiter.state == ThreadState::Blocked
                    && matches!(waiter.reason,BlockReason::Word(address,_) if (address>=base && address<end)||(address>=tls_base&&address<tls_base+tls_frames.len() as u64*4096))
                {
                    set_return(waiter, ERROR);
                    waiter.state = ThreadState::Ready;
                    waiter.reason = BlockReason::None;
                    ready_queue.push_back(waiter.tid);
                }
            }
            smp::shootdown();
            for frame in frames.into_iter().chain(tls_frames) {
                // SAFETY: Thread is off every CPU, its leaves were removed,
                // all remote TLBs acknowledged, and this ledger is the sole owner.
                unsafe {
                    s.frame_allocator
                        .as_mut()
                        .expect("frame pool")
                        .recycle(frame);
                }
            }
            thread_charge = scope_charge;
            if let Some(p) = s.processes.get_mut(t.pid as usize).and_then(Option::as_mut) {
                p.stack_slots &= !(1u64 << slot);
            }
        }
        // Kernel stack/metadata cease to exist before a scope can become closed.
        drop(t);
        if let Some(charge) = thread_charge {
            s.scopes.release(charge).expect("owned thread scope charge");
        }
    }
    let mut i = 0;
    while i < s.retired.len() {
        let pid = s.retired[i].pid;
        if s.threads.iter().flatten().any(|t| t.pid == pid) {
            i += 1;
            continue;
        }
        let mut p = s.retired.swap_remove(i);
        let scope_charge = p.scope_charge.take();
        let frames = core::mem::take(&mut p.owned_frames);
        smp::shootdown();
        for frame in frames {
            // SAFETY: Every thread is off-CPU and gone; no user-copy reference
            // survives entry, and no TLB or DMA references these owned frames.
            unsafe {
                s.frame_allocator
                    .as_mut()
                    .expect("frame pool")
                    .recycle(frame);
            }
        }
        drop(p);
        if let Some(charge) = scope_charge {
            s.scopes
                .release(charge)
                .expect("owned process scope charge");
        }
    }
}
