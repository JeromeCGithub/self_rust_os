//! Current-process native channels. SCHEDULER serializes objects and waiters;
//! the entry gate stabilizes mappings through each complete validated usercopy.
//! No user reference or lock survives blocking. IRQ paths do not allocate.
use super::{events, ProcessState, Scheduler, ERROR, SCHEDULER};
use crate::userspace::usercopy;
use rustos_ipc_abi::{self as abi, Handle, ProcessKey};
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
                    if s.ipc.close(owner, Handle(first)).is_err() {
                        return ERROR;
                    }
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
    let Ok(handles) = s.ipc.pair(owner) else {
        return ERROR;
    };
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&handles[0].0.to_le_bytes());
    bytes[8..].copy_from_slice(&handles[1].0.to_le_bytes());
    if !usercopy::to_user(pointer, &bytes) {
        // Both unpublished handles remain owned by this caller under this lock.
        // Rollback cannot race transfer/close or consume any preexisting object.
        s.ipc
            .close(owner, handles[0])
            .expect("unpublished first endpoint");
        s.ipc
            .close(owner, handles[1])
            .expect("unpublished second endpoint");
        return ERROR;
    }
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
