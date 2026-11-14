//! Read-only caller principal snapshots. Scope factories are separate authority.
use super::{ProcessState, ERROR, SCHEDULER};
use rustos_process_abi::principal as abi;
/// Copy a complete canonical record only after validating the full writable range.
/// Kernel entry stabilizes mappings. No process reference survives the lock/copy.
pub fn principal_user(pointer: u64, size: u64, reserved: u64) -> u64 {
    if size != abi::SIZE as u64
        || reserved != 0
        || !crate::userspace::usercopy::validate(pointer, size, true)
    {
        return ERROR;
    }
    let bytes = {
        let scheduler = SCHEDULER.lock();
        let Some(process) = scheduler
            .current_pid()
            .and_then(|pid| scheduler.processes.get(pid as usize))
            .and_then(Option::as_ref)
            .filter(|process| process.state == ProcessState::Live)
        else {
            return ERROR;
        };
        let Ok(bytes) = process.principal.encode() else {
            return ERROR;
        };
        bytes
    };
    if !crate::userspace::usercopy::to_user(pointer, &bytes) {
        return ERROR;
    }
    0
}

/// Reduce ambient rights atomically before threads/untrusted work are started.
/// Entry gate and scheduler lock serialize both checks and publication. Requiring
/// exactly one owned thread prevents a sibling's filesystem syscall (which may
/// release kernel entry during I/O) from outliving the rights reduction. Native
/// PID 1 retains its recovery authority. This operation revokes no existing grant
/// or child's authority; containment/resource reclamation belongs to scopes.
pub fn attenuate_principal(bits: u64, flags: u64, reserved: u64) -> u64 {
    let Ok(requested) = abi::Capabilities::from_bits(bits) else {
        return ERROR;
    };
    if flags != 0 || reserved != 0 {
        return ERROR;
    }
    let mut scheduler = SCHEDULER.lock();
    let Some(pid) = scheduler.current_pid() else {
        return ERROR;
    };
    if scheduler.init_pid == Some(pid)
        || scheduler
            .threads
            .iter()
            .flatten()
            .filter(|thread| thread.pid == pid)
            .count()
            != 1
    {
        return ERROR;
    }
    let Some(process) = scheduler
        .processes
        .get_mut(pid as usize)
        .and_then(Option::as_mut)
        .filter(|process| process.state == ProcessState::Live)
    else {
        return ERROR;
    };
    let Ok(reduced) = process.principal.capabilities.attenuate(requested) else {
        return ERROR;
    };
    process.principal.capabilities = reduced;
    0
}

/// Snapshot write authority before acquiring the filesystem service lock. The
/// legacy desktop remains explicitly trusted until its M6 lease migration.
/// Native ordinary children cannot inherit protected-system-write authority.
pub(crate) fn system_write_authority() -> bool {
    let scheduler = SCHEDULER.lock();
    scheduler
        .current_pid()
        .and_then(|pid| scheduler.processes.get(pid as usize))
        .and_then(Option::as_ref)
        .filter(|process| process.state == ProcessState::Live)
        .is_some_and(|process| {
            process.principal.flags & abi::LEGACY != 0
                || process
                    .principal
                    .capabilities
                    .contains(abi::Capabilities::SYSTEM_WRITE)
        })
}
