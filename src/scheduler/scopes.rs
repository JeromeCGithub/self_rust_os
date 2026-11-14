//! Kernel-only native scope adapters. No Ring 3 grant ABI is exposed yet.
use super::{ProcessState, Scheduler, SCHEDULER};
use rustos_process_abi::principal::{Capabilities, Principal};
use rustos_scope_core::{Error, Kind, ProcessKey, Snapshot, State, Usage};

pub(super) fn caller(s: &Scheduler) -> Result<ProcessKey, Error> {
    if s.init_pid.is_none() {
        return Err(Error::Denied);
    }
    let pid = s.current_pid().ok_or(Error::Denied)?;
    s.processes
        .get(pid as usize)
        .and_then(Option::as_ref)
        .filter(|p| p.state == ProcessState::Live)
        .ok_or(Error::Denied)?;
    Ok(ProcessKey { pid, generation: 1 })
}
fn factory(s: &Scheduler) -> Result<ProcessKey, Error> {
    let key = caller(s)?;
    if !s.processes[key.pid as usize]
        .as_ref()
        .ok_or(Error::Denied)?
        .principal
        .capabilities
        .contains(Capabilities::SCOPE_CREATE)
    {
        return Err(Error::Denied);
    }
    Ok(key)
}
/// Create a session using current kernel-derived authority.
pub fn scope_create_session(user: u32, limit: Usage) -> Result<u64, Error> {
    let mut s = SCHEDULER.lock();
    let owner = factory(&s)?;
    s.scopes.create_session(owner, user, limit)
}
/// Create an owned service leaf under an authorized factory.
pub fn scope_create_service(parent: u64, limit: Usage) -> Result<u64, Error> {
    let mut s = SCHEDULER.lock();
    let owner = factory(&s)?;
    s.scopes.create_service(owner, parent, limit)
}
/// Snapshot an owned scope. Currently charged: processes/base threads, private
/// frames, constructor/heap/thread ownership allocations and IPC handles/inboxes.
/// Filesystem/socket/audio accounting remain pending; this is not a Ring 3 ABI.
pub fn scope_status(scope: u64) -> Result<Snapshot, Error> {
    let s = SCHEDULER.lock();
    s.scopes.status(caller(&s)?, scope)
}
/// Begin closure and terminate scope members; completion requires reclamation.
pub fn scope_close(scope: u64) -> Result<(), Error> {
    let mut s = SCHEDULER.lock();
    let owner = caller(&s)?;
    s.scopes.close(owner, scope)?;
    drain_locked(&mut s);
    Ok(())
}
/// Peek durable closed-scope state without consuming process status.
pub fn scope_next_closed() -> Result<Option<Snapshot>, Error> {
    let s = SCHEDULER.lock();
    Ok(s.scopes.next_closed(caller(&s)?))
}
/// Release a closed, drained scope record.
pub fn scope_ack_closed(scope: u64) -> Result<(), Error> {
    let mut s = SCHEDULER.lock();
    let owner = caller(&s)?;
    s.scopes.ack_closed(owner, scope)
}
/// Construct an explicitly scoped leader with attenuated rights, atomically.
/// Trusted kernel callers supply an owned image; the userspace grant API is shut.
pub fn spawn_process_in_scope(
    binary: &[u8],
    readonly: usize,
    argv: &[&str],
    envp: &[&str],
    scope: u64,
    caps: Capabilities,
) -> Result<super::Pid, &'static str> {
    spawn_process_in_scope_with_bindings(binary, readonly, argv, envp, &[], scope, caps)
}
/// Publish an explicitly scoped leader and its attenuated endpoint moves under
/// the same scheduler lock. Kernel-only until the Ring 3 grant ABI is complete.
pub fn spawn_process_in_scope_with_bindings(
    binary: &[u8],
    readonly: usize,
    argv: &[&str],
    envp: &[&str],
    bindings: &[rustos_ipc_abi::Binding],
    scope: u64,
    caps: Capabilities,
) -> Result<super::Pid, &'static str> {
    rustos_process_abi::startup_size(argv, envp).map_err(|_| "invalid startup vectors")?;
    x86_64::instructions::interrupts::without_interrupts(|| {
        super::spawn_process_inner(binary, readonly, argv, envp, bindings, Some((scope, caps)))
    })
}
pub(super) fn launch_principal(
    s: &Scheduler,
    scope: u64,
    caps: Capabilities,
) -> Result<Principal, &'static str> {
    let owner = caller(s).map_err(|_| "scope caller unavailable")?;
    let source = s.processes[owner.pid as usize]
        .as_ref()
        .ok_or("scope caller exited")?
        .principal;
    if !source.capabilities.contains(Capabilities::SPAWN)
        || source.capabilities.attenuate(caps).is_err()
    {
        return Err("scope rights expansion");
    }
    let target = s
        .scopes
        .status(owner, scope)
        .map_err(|_| "scope grant denied")?;
    if target.owner != owner
        || target.kind == Kind::System
        || target.leader.is_some()
        || target.state != State::Open
    {
        return Err("scope leader grant unavailable");
    }
    if (target.session != 0 && caps.contains(Capabilities::SYSTEM_WRITE))
        || (target.kind != Kind::Session && caps.contains(Capabilities::SCOPE_CREATE))
    {
        return Err("scope role rights denied");
    }
    Ok(Principal {
        user: target.user,
        session: target.session,
        scope,
        flags: 0,
        capabilities: caps,
    })
}
/// Bounded, allocation-free drain. Nested stop/connected-child paths can notify
/// more closures, but never recursively start another scope-drain traversal.
pub(super) fn drain_locked(s: &mut Scheduler) {
    if s.draining_scopes {
        return;
    }
    s.draining_scopes = true;
    for _ in 0..128 {
        let victim = s
            .processes
            .iter()
            .flatten()
            .find(|p| p.state == ProcessState::Live && s.scopes.is_closing(p.principal.scope))
            .map(|p| p.pid);
        let Some(pid) = victim else {
            break;
        };
        super::stop_process_with_cause_locked(
            s,
            pid,
            9,
            rustos_process_abi::supervision::Cause::Killed,
        );
    }
    s.draining_scopes = false;
}

/// Existing kernel fatal-init handling runs before this non-init notification.
pub(super) fn death_locked(s: &mut Scheduler, pid: super::Pid) {
    if s.init_pid.is_some() {
        s.scopes
            .owner_died(ProcessKey { pid, generation: 1 })
            .expect("live non-init scope death notification");
    }
}
