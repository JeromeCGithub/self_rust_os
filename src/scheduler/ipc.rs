//! Current-process native channels. SCHEDULER serializes objects and waiters;
//! the entry gate stabilizes mappings through each complete validated usercopy.
//! No user reference or lock survives blocking. IRQ paths do not allocate.
use super::{events, ProcessState, Scheduler, ERROR, SCHEDULER};
use crate::userspace::usercopy;
use rustos_ipc_abi::{self as abi, Handle, ProcessKey};
use rustos_scope_core::Usage;
/// The native non-system pool leaves this many physical channel slots to init.
pub(super) const RECOVERY_CHANNELS: usize = 8;
pub(super) fn usage(storage: rustos_ipc_core::Storage) -> Usage {
    Usage {
        ipc_handles: storage.handles as u64,
        kernel_bytes: storage.inbox_bytes as u64,
        ..Usage::ZERO
    }
}
fn released(s: &mut Scheduler, owner: ProcessKey, before: rustos_ipc_core::Storage) {
    let after = s.ipc.owner_usage(owner).expect("authenticated IPC owner");
    let delta = Usage {
        ipc_handles: (before.handles - after.handles) as u64,
        kernel_bytes: (before.inbox_bytes - after.inbox_bytes) as u64,
        ..Usage::ZERO
    };
    let Scheduler {
        scopes, processes, ..
    } = s;
    if let Some(charge) = processes[owner.pid as usize]
        .as_mut()
        .and_then(|p| p.scope_charge.as_mut())
    {
        scopes
            .release_part(charge, delta)
            .expect("closed endpoints owned their scope reservations");
    }
}
/// Revoke real endpoint storage before releasing its reservation; no allocation.
pub(super) fn close_owner_locked(s: &mut Scheduler, pid: super::Pid) {
    let owner = ProcessKey { pid, generation: 1 };
    let before = s.ipc.owner_usage(owner).expect("valid stopped process");
    s.ipc.close_owner(owner);
    released(s, owner, before);
}
fn owner(s: &Scheduler) -> Option<ProcessKey> {
    let pid = s.current_pid()?;
    s.processes
        .get(pid as usize)?
        .as_ref()
        .filter(|p| p.state == ProcessState::Live)?;
    Some(ProcessKey { pid, generation: 1 })
}
/// Dispatch pair/send/next/close/interest/bootstrap after the syscall allowlist.
/// Objects belong to the authenticated current process; numeric handles confer
/// no authority in another process. Output publication precedes event consumption.
pub fn ipc_syscall(number: u64, first: u64, second: u64, third: u64) -> u64 {
    match number {
        abi::SYS_PAIR => pair(first, second, third),
        abi::SYS_SEND => send(first, second, third),
        abi::SYS_NEXT => next(first, second, third),
        abi::SYS_CLOSE | abi::SYS_INTEREST | abi::SYS_BOOTSTRAP => {
            if third != 0
                || number != abi::SYS_INTEREST && second != 0
                || number == abi::SYS_INTEREST && second & !abi::INTEREST_ALL != 0
            {
                return ERROR;
            }
            let mut s = SCHEDULER.lock();
            let Some(owner) = owner(&s) else {
                return ERROR;
            };
            match number {
                abi::SYS_BOOTSTRAP => {
                    let Ok(tag) = u32::try_from(first) else {
                        return ERROR;
                    };
                    s.ipc
                        .take_bootstrap(owner, tag)
                        .map(|handle| handle.0)
                        .unwrap_or(ERROR)
                }
                abi::SYS_CLOSE => {
                    let before = s.ipc.owner_usage(owner).expect("authenticated IPC owner");
                    if s.ipc.close(owner, Handle(first)).is_err() {
                        return ERROR;
                    }
                    released(&mut s, owner, before);
                    events::wake_locked(&mut s);
                    0
                }
                _ => {
                    if s.ipc
                        .set_interest(
                            owner,
                            Handle(first),
                            second & abi::INTEREST_READ != 0,
                            second & abi::INTEREST_WRITE != 0,
                        )
                        .is_err()
                    {
                        return ERROR;
                    }
                    events::wake_locked(&mut s);
                    0
                }
            }
        }
        _ => ERROR,
    }
}
fn pair(pointer: u64, size: u64, reserved: u64) -> u64 {
    if size != 16 || reserved != 0 || !usercopy::validate(pointer, size, true) {
        return ERROR;
    }
    let mut s = SCHEDULER.lock();
    let Some(owner) = owner(&s) else {
        return ERROR;
    };
    let scope = s.processes[owner.pid as usize]
        .as_ref()
        .expect("live IPC owner")
        .principal
        .scope;
    if s.init_pid.is_some()
        && scope != rustos_scope_core::SYSTEM
        && s.ipc.channels() >= abi::MAX_CHANNELS - RECOVERY_CHANNELS
    {
        return ERROR;
    }
    let Scheduler {
        scopes,
        processes,
        ipc,
        ..
    } = &mut *s;
    let before = ipc.owner_usage(owner).expect("authenticated IPC owner");
    let mut budget = match processes[owner.pid as usize]
        .as_mut()
        .expect("live owner")
        .scope_charge
        .as_mut()
    {
        Some(charge) => match scopes.growth(
            charge,
            Usage {
                ipc_handles: 2,
                kernel_bytes: rustos_ipc_core::PAIR_INBOX_BYTES as u64,
                ..Usage::ZERO
            },
        ) {
            Ok(budget) => Some(budget),
            Err(_) => return ERROR,
        },
        None => None,
    };
    let Ok(handles) = ipc.pair(owner) else {
        return ERROR;
    };
    let after = ipc.owner_usage(owner).expect("published pair owner");
    let additional = (after.inbox_bytes - before.inbox_bytes)
        .checked_sub(rustos_ipc_core::PAIR_INBOX_BYTES)
        .expect("reserved minimum inbox storage");
    if budget.as_mut().is_some_and(|budget| {
        budget
            .grow(Usage {
                kernel_bytes: additional as u64,
                ..Usage::ZERO
            })
            .is_err()
    }) {
        ipc.close(owner, handles[0]).expect("unpublished endpoint");
        ipc.close(owner, handles[1]).expect("unpublished endpoint");
        return ERROR;
    }
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&handles[0].0.to_le_bytes());
    bytes[8..].copy_from_slice(&handles[1].0.to_le_bytes());
    if !usercopy::to_user(pointer, &bytes) {
        // The same scheduler lock spans allocation/copy; destroy objects before
        // the growth guard returns quota, preserving every older owned endpoint.
        ipc.close(owner, handles[0])
            .expect("unpublished first endpoint");
        ipc.close(owner, handles[1])
            .expect("unpublished second endpoint");
        return ERROR;
    }
    budget.map(rustos_scope_core::Growth::commit);
    0
}

fn send(handle: u64, pointer: u64, length: u64) -> u64 {
    if length > abi::MAX_PAYLOAD as u64 {
        return ERROR;
    }
    let mut bytes = [0; abi::MAX_PAYLOAD];
    if !usercopy::from_user(pointer, &mut bytes[..length as usize]) {
        return ERROR;
    }
    let mut s = SCHEDULER.lock();
    let Some(owner) = owner(&s) else {
        return ERROR;
    };
    match s.ipc.send(owner, Handle(handle), &bytes[..length as usize]) {
        Ok(()) => {
            events::wake_locked(&mut s);
            0
        }
        Err(rustos_ipc_core::Error::Full) => abi::SEND_FULL,
        Err(rustos_ipc_core::Error::Closed) => abi::SEND_CLOSED,
        Err(_) => ERROR,
    }
}
fn next(pointer: u64, size: u64, reserved: u64) -> u64 {
    if size != abi::EVENT_SIZE as u64 || reserved != 0 || !usercopy::validate(pointer, size, true) {
        return ERROR;
    }
    let mut s = SCHEDULER.lock();
    let Some(owner) = owner(&s) else {
        return ERROR;
    };
    let pending = match s.ipc.peek(owner) {
        Ok(Some(pending)) => pending,
        Ok(None) => return 0,
        Err(_) => return ERROR,
    };
    let Ok(bytes) = pending.event.encode() else {
        return ERROR;
    };
    if !usercopy::to_user(pointer, &bytes) {
        return ERROR;
    }
    // The same lock spans peek, copy and commit; no competing reader can consume
    // this identity, and every mutation used kernel-owned bounded data.
    s.ipc
        .consume(owner, pending)
        .expect("peeked native event under scheduler lock");
    events::wake_locked(&mut s);
    1
}
