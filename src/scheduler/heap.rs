//! Linux-style program break over owned, zero-filled private pages.
use super::{smp, Scheduler, ERROR, SCHEDULER};
use alloc::vec::Vec;
use x86_64::{
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};
/// Query/grow/shrink the calling process heap. Failure returns the old break.
/// Page-table mutation is serialized by entry and scheduler locks; published
/// mappings are invalidated across CPUs before any frame can be recycled.
pub fn program_break(request: u64) -> u64 {
    let mut scheduler = SCHEDULER.lock();
    let Some(pid) = scheduler.current_pid() else {
        return ERROR;
    };
    let process = scheduler.processes[pid as usize]
        .as_ref()
        .expect("live owner");
    let old = process.heap_break;
    let root = process.p4_frame;
    if request == 0 {
        return old;
    }
    let limit = process
        .worker_heap_limit
        .unwrap_or(crate::userspace::USER_STACK_BOTTOM - 4096);
    if request < rustos_thread_abi::HEAP_BASE || request > limit {
        return old;
    }
    let previous = (old + 4095) & !4095;
    let end = (request + 4095) & !4095;
    if previous == end {
        scheduler.processes[pid as usize]
            .as_mut()
            .expect("owner")
            .heap_break = request;
        return request;
    }
    let offset = VirtAddr::new(scheduler.phys_offset);
    let table = (offset + root.start_address().as_u64()).as_mut_ptr::<PageTable>();
    if end < previous {
        let count = ((previous - end) / 4096) as usize;
        let mut removed = Vec::new();
        if removed.try_reserve_exact(count).is_err() {
            return old;
        }
        // SAFETY: This live process owns root. Entry excludes other kernel
        // mutation and copies; no mapper reference survives the shootdown.
        let mut mapper = unsafe { OffsetPageTable::new(&mut *table, offset) };
        for address in (end..previous).step_by(4096) {
            let (frame, flush) = mapper
                .unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(address)))
                .expect("owned heap page");
            flush.ignore();
            removed.push(frame);
        }
        drop(mapper);
        smp::shootdown();
        removed.sort_unstable();
        let process = scheduler.processes[pid as usize].as_mut().expect("owner");
        process
            .owned_frames
            .retain(|frame| removed.binary_search(frame).is_err());
        process.heap_break = request;
        for frame in removed {
            // SAFETY: Owned leaf is unmapped, remote TLBs acknowledged, and its
            // sole ledger entry was removed. Parent tables remain process-owned.
            unsafe {
                scheduler
                    .frame_allocator
                    .as_mut()
                    .expect("pool")
                    .recycle(frame);
            }
        }
        let Scheduler {
            scopes, processes, ..
        } = &mut *scheduler;
        if let Some(charge) = processes[pid as usize]
            .as_mut()
            .expect("owner")
            .scope_charge
            .as_mut()
        {
            scopes
                .release_part(
                    charge,
                    rustos_scope_core::Usage {
                        pages: count as u64,
                        ..rustos_scope_core::Usage::ZERO
                    },
                )
                .expect("unmapped heap page charge");
        }
        return request;
    }
    let count = ((end - previous) / 4096) as usize;
    let old_metadata = process.owned_frames.capacity()
        * core::mem::size_of::<x86_64::structures::paging::PhysFrame>();
    let Some(capacity) = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(process.owned_frames.len()))
    else {
        return old;
    };
    let Some(expected_metadata) =
        capacity.checked_mul(core::mem::size_of::<x86_64::structures::paging::PhysFrame>())
    else {
        return old;
    };
    let Scheduler {
        scopes,
        processes,
        frame_allocator,
        ..
    } = &mut *scheduler;
    let process = processes[pid as usize].as_mut().expect("owner");
    let mut budget = match process.scope_charge.as_mut() {
        Some(charge) => match scopes.growth(
            charge,
            rustos_scope_core::Usage {
                kernel_bytes: expected_metadata.saturating_sub(old_metadata) as u64,
                ..rustos_scope_core::Usage::ZERO
            },
        ) {
            Ok(budget) => Some(budget),
            Err(_) => return old,
        },
        None => None,
    };
    // The new ledger and mapping rollback are dropped before the quota guard.
    let mut ownership = Vec::new();
    if ownership.try_reserve_exact(capacity).is_err() {
        return old;
    }
    let actual_metadata =
        ownership.capacity() * core::mem::size_of::<x86_64::structures::paging::PhysFrame>();
    if let Some(budget) = budget.as_mut() {
        if budget
            .grow(rustos_scope_core::Usage {
                kernel_bytes: actual_metadata.saturating_sub(expected_metadata) as u64,
                ..rustos_scope_core::Usage::ZERO
            })
            .is_err()
        {
            return old;
        }
    }
    let maximum = match budget.as_ref() {
        Some(budget) => match budget
            .available()
            .ok()
            .and_then(|room| usize::try_from(room.pages).ok())
        {
            Some(n) => n,
            None => return old,
        },
        None => usize::MAX,
    };
    let mut parents = Vec::new();
    let mut mapped = 0;
    let pool = frame_allocator.as_mut().expect("pool");
    let mut transaction = crate::memory::owned::FrameTransaction::bounded(pool, maximum);
    // SAFETY: Exclusive live root under entry/scheduler serialization. Temporary
    // raw parent links are used only while this mapper operation owns the tree.
    let mut mapper = unsafe { OffsetPageTable::new(&mut *table, offset) };
    let result = (|| {
        for address in (previous..end).step_by(4096) {
            // SAFETY: User heap arena is private to this process; every new
            // parent link is recorded for reverse-order rollback before free.
            unsafe {
                crate::memory::user::ensure_tables(
                    mapper.level_4_table(),
                    address,
                    &mut transaction,
                    offset,
                    &mut parents,
                )
            }?;
            let frame = transaction.allocate_frame().ok_or(())?;
            // SAFETY: Fresh owned frame, 4 KiB valid direct-mapped RAM, zeroed
            // before any user-visible PTE; no references alias the destination.
            unsafe {
                core::ptr::write_bytes(
                    (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
                    0,
                    4096,
                );
            }
            // SAFETY: Previously unmapped reserved heap page; distinct owned
            // zeroed frame, complete parent ownership and entry serialization.
            unsafe {
                mapper.map_to(
                    Page::<Size4KiB>::containing_address(VirtAddr::new(address)),
                    frame,
                    PageTableFlags::PRESENT
                        | PageTableFlags::WRITABLE
                        | PageTableFlags::USER_ACCESSIBLE,
                    &mut transaction,
                )
            }
            .map_err(|_| ())?
            .ignore();
            mapped += 1;
        }
        if let Some(budget) = budget.as_mut() {
            budget
                .grow(rustos_scope_core::Usage {
                    pages: transaction.allocated_frames() as u64,
                    ..rustos_scope_core::Usage::ZERO
                })
                .map_err(|_| ())?;
        }
        Ok::<_, ()>(())
    })();
    if result.is_err() {
        for i in 0..mapped {
            mapper
                .unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(
                    previous + i * 4096,
                )))
                .expect("partial heap leaf")
                .1
                .ignore();
        }
        drop(mapper);
        // SAFETY: Leaves are detached, and these links were created only by
        // this failed operation. Parent frames remain transaction-owned.
        unsafe {
            crate::memory::user::rollback_parents(&parents);
        }
        smp::shootdown();
        return old;
    }
    drop(mapper);
    let frames = transaction.commit();
    ownership.extend_from_slice(&process.owned_frames);
    ownership.extend(frames);
    process.owned_frames = ownership;
    process.heap_break = request;
    budget.map(rustos_scope_core::Growth::commit);
    if actual_metadata < old_metadata {
        if let Some(charge) = process.scope_charge.as_mut() {
            scopes
                .release_part(
                    charge,
                    rustos_scope_core::Usage {
                        kernel_bytes: (old_metadata - actual_metadata) as u64,
                        ..rustos_scope_core::Usage::ZERO
                    },
                )
                .expect("replaced heap ownership metadata");
        }
    }
    smp::shootdown();
    request
}
