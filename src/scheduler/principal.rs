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

/// Copy authority before acquiring FS; callers retain no scheduler references.
pub(crate) fn filesystem_authority() -> Option<crate::fs::access::Authority> {
    filesystem_snapshot(false)
}
pub(crate) fn filesystem_read_authority() -> Option<crate::fs::access::Authority> {
    filesystem_snapshot(true)
}
fn filesystem_snapshot(read: bool) -> Option<crate::fs::access::Authority> {
    let s = SCHEDULER.lock();
    let process = s.processes.get(s.current_pid()? as usize)?.as_ref()?;
    if process.state != ProcessState::Live
        || (read
            && !process
                .principal
                .capabilities
                .contains(abi::Capabilities::FILE_READ))
    {
        return None;
    }
    Some(process.filesystem_authority)
}
/// Irreversibly narrow this native process's directory grants. Kernel-only until
/// the scope factory ABI can install initial grants atomically before publication.
/// Single-thread admission excludes concurrent filesystem I/O by a sibling.
pub fn restrict_filesystem(requests: &[(&str, crate::fs::FileRights)]) -> Result<(), &'static str> {
    let (pid, source) = {
        let s = SCHEDULER.lock();
        let pid = s.current_pid().ok_or("no filesystem authority owner")?;
        if s.init_pid.is_none()
            || s.init_pid == Some(pid)
            || s.threads.iter().flatten().filter(|t| t.pid == pid).count() != 1
        {
            return Err("filesystem attenuation requires one native non-init thread");
        }
        let p = s
            .processes
            .get(pid as usize)
            .and_then(Option::as_ref)
            .filter(|p| p.state == ProcessState::Live)
            .ok_or("filesystem owner exited")?;
        for &(_, rights) in requests {
            if (rights.contains(crate::fs::FileRights::READ)
                && !p
                    .principal
                    .capabilities
                    .contains(abi::Capabilities::FILE_READ))
                || (rights.contains(crate::fs::FileRights::WRITE)
                    && !p
                        .principal
                        .capabilities
                        .contains(abi::Capabilities::FILE_WRITE))
            {
                return Err("filesystem mechanism rights denied");
            }
        }
        (pid, p.filesystem_authority)
    };
    let narrowed = crate::fs::with_fs(|fs| source.narrow(fs, requests))
        .ok_or("filesystem unavailable")?
        .map_err(|_| "filesystem grant expansion or invalid root")?;
    let mut s = SCHEDULER.lock();
    if s.current_pid() != Some(pid)
        || s.threads.iter().flatten().filter(|t| t.pid == pid).count() != 1
    {
        return Err("filesystem owner changed");
    }
    let p = s
        .processes
        .get_mut(pid as usize)
        .and_then(Option::as_mut)
        .filter(|p| p.state == ProcessState::Live && p.filesystem_authority == source)
        .ok_or("filesystem owner changed")?;
    p.filesystem_authority = narrowed;
    Ok(())
}
