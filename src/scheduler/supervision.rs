//! Owned child notifications; user output is copied before acknowledgement.
use super::{reap_connection_locked, retire_process, ProcessState, ERROR, SCHEDULER};
use rustos_process_abi::supervision::{ChildEvent, ProcessKey, EVENT_SIZE};
/// Consume at most one event owned by the calling process. Entry serialization
/// stabilizes user mappings; the scheduler lock serializes selection and reaping.
pub fn child_next_user(pointer: u64, size: u64, reserved: u64) -> u64 {
    if size != EVENT_SIZE as u64
        || reserved != 0
        || !crate::userspace::usercopy::validate(pointer, size, true)
    {
        return ERROR;
    }
    let mut s = SCHEDULER.lock();
    let Some(owner) = s.current_pid() else {
        return ERROR;
    };
    let Some(child) = s.processes.iter().flatten().find(|p| {
        p.parent == Some(owner) && (p.state == ProcessState::Zombie || p.adoption_pending)
    }) else {
        return 0;
    };
    let pid = child.pid;
    let terminal = child.state == ProcessState::Zombie;
    let key = ProcessKey { pid, generation: 1 };
    let event = if terminal {
        ChildEvent::Completed {
            process: key,
            cause: child.exit_cause,
            status: child.exit_code,
        }
    } else {
        ChildEvent::Adopted(key)
    };
    if !crate::userspace::usercopy::to_user(pointer, &event.encode()) {
        return ERROR;
    }
    if terminal {
        reap_connection_locked(&mut s, pid);
        retire_process(&mut s, pid);
    } else if let Some(p) = s.processes.get_mut(pid as usize).and_then(Option::as_mut) {
        p.adoption_pending = false;
    }
    1
}
/// Terminate only a live direct child named by its complete native identity.
pub fn terminate_child(identity: u64, second: u64, third: u64) -> u64 {
    use rustos_process_abi::supervision::Cause;
    if second != 0 || third != 0 {
        return ERROR;
    }
    let Some(key) = ProcessKey::decode(identity) else {
        return ERROR;
    };
    let mut s = SCHEDULER.lock();
    let Some(owner) = s.current_pid() else {
        return ERROR;
    };
    let Some(child) = s.processes.get(key.pid as usize).and_then(Option::as_ref) else {
        return ERROR;
    };
    if child.parent != Some(owner) || child.state != ProcessState::Live {
        return ERROR;
    }
    super::stop_process_with_cause_locked(&mut s, key.pid, 9, Cause::Killed);
    0
}
/// Select native-init ownership before publishing the first userspace context.
pub fn configure_native_init() -> Result<(), &'static str> {
    let mut s = SCHEDULER.lock();
    if s.next_pid != 0 || s.init_pid.is_some() || s.frame_allocator.is_none() {
        return Err("native init must be selected once before process publication");
    }
    s.next_pid = 1;
    s.init_pid = Some(1);
    Ok(())
}

pub(super) fn adopt_orphans_locked(s: &mut super::Scheduler) {
    let Some(init) = s.init_pid else {
        return;
    };
    // A bounded scalar list avoids allocation and mutable/immutable table aliasing.
    let mut dead = [0; 128];
    let mut count = 0;
    for p in s
        .processes
        .iter()
        .flatten()
        .filter(|p| p.state == ProcessState::Zombie)
    {
        dead[count] = p.pid;
        count += 1;
    }
    for p in s.processes.iter_mut().flatten() {
        if p.parent.is_some_and(|pid| dead[..count].contains(&pid)) {
            p.parent = Some(init);
            p.adoption_pending = p.state == ProcessState::Live;
        }
    }
}

/// Stop the current process after a user exception; vector is public status.
pub fn fault_current(vector: u64) {
    let mut s = SCHEDULER.lock();
    if let Some(pid) = s.current_pid() {
        super::stop_process_with_cause_locked(
            &mut s,
            pid,
            vector,
            rustos_process_abi::supervision::Cause::Fault,
        );
    }
}

/// Terminal PID 1 failure. Do not allocate, unwind, wait on a service lock or
/// reclaim memory: remote CPUs may have been interrupted while owning resources.
pub(super) fn fatal_init(cause: rustos_process_abi::supervision::Cause, status: u64) -> ! {
    use core::fmt::Write;
    let parked = crate::arch::x86_64::smp::park_others_terminal();
    if let Some(mut serial) = crate::drivers::tty::serial::SERIAL1.try_lock() {
        let _ = writeln!(
            serial,
            "[initator-fatal] cause={:?} status={} parked={}",
            cause, status, parked
        );
    }
    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}

/// Native-init identity is selected by kernel boot, never inferred from PID alone.
/// The entry gate excludes competing publication; the scheduler lock stabilizes
/// current-process metadata. Terminal halt must recheck drained ownership last.
pub(crate) fn system_action_gate(require_drained: bool) -> Result<(), rustos_platform_abi::Error> {
    use rustos_platform_abi::Error;
    let scheduler = SCHEDULER.lock();
    let owner = scheduler.current_pid().ok_or(Error::Denied)?;
    if scheduler.init_pid != Some(owner)
        || !scheduler
            .processes
            .get(owner as usize)
            .and_then(Option::as_ref)
            .is_some_and(|process| process.state == ProcessState::Live)
    {
        return Err(Error::Denied);
    }
    // Zombies still own collection obligations. Retired frames are never reused
    // by terminal halt; they need not be freed merely to enter the final state.
    if require_drained
        && scheduler
            .processes
            .iter()
            .flatten()
            .any(|process| process.pid != owner)
    {
        return Err(Error::Busy);
    }
    Ok(())
}
