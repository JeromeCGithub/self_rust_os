//! Bounded owned process channels, separate from compositor mailboxes.
//! The kernel entry gate and scheduler lock protect all links and queue state;
//! no reference escapes an entry or survives scheduling. App policy stays in Ring 3.
use super::{
    events, retire_process, stop_threads_locked, wake_waiters_locked, Pid, Process, ProcessState,
    Scheduler, ERROR, SCHEDULER,
};
use alloc::collections::VecDeque;

/// At most 16 KiB per endpoint, allocated fallibly only for connected processes.
pub(super) const STDIO_QUEUE_CAPACITY: usize = 4096;

/// Whether the current process can own a new I/O connection.
pub fn can_connect_stdio() -> bool {
    let sched = SCHEDULER.lock();
    sched
        .current_pid()
        .and_then(|pid| sched.processes[pid as usize].as_ref())
        .map(|p| p.io_nested_peer.is_none() && !p.io_peer.is_some_and(|(_, child)| !child))
        .unwrap_or(false)
}

/// Connect a newly spawned child before returning from its spawning syscall.
/// The kernel entry gate spans creation and this linkage, so no child executes
/// before its connection is published. This function is not exposed as an attach syscall.
pub fn connect_stdio(child: Pid) -> bool {
    let mut sched = SCHEDULER.lock();
    let Some(parent) = sched.current_pid() else {
        return false;
    };
    if parent == child {
        return false;
    }
    let Some(host) = sched
        .processes
        .get(parent as usize)
        .and_then(Option::as_ref)
    else {
        return false;
    };
    if host.state == ProcessState::Zombie
        || host.io_nested_peer.is_some()
        || host.io_peer.is_some_and(|(_, child)| !child)
    {
        return false;
    }
    let nested = host.io_peer.is_some();
    let Some(target) = sched.processes.get(child as usize).and_then(Option::as_ref) else {
        return false;
    };
    if target.state == ProcessState::Zombie
        || target.io_peer.is_some()
        || target.io_nested_peer.is_some()
    {
        return false;
    }
    // Reserve both queues before publishing either endpoint. Allocation failure
    // drops local storage and leaves both process records unchanged. Enqueue
    // never allocates; the explicit bound stays below reserved capacity.
    let mut parent_queue = VecDeque::new();
    let mut child_queue = VecDeque::new();
    if parent_queue
        .try_reserve_exact(STDIO_QUEUE_CAPACITY)
        .is_err()
        || child_queue.try_reserve_exact(STDIO_QUEUE_CAPACITY).is_err()
    {
        return false;
    }
    let host = sched.processes[parent as usize]
        .as_mut()
        .expect("validated host");
    if nested {
        host.io_nested_peer = Some(child);
        host.io_nested_inbox = parent_queue;
    } else {
        host.io_peer = Some((child, false));
        host.io_inbox = parent_queue;
    }
    let target = sched.processes[child as usize]
        .as_mut()
        .expect("validated child");
    target.io_inbox = child_queue;
    target.io_peer = Some((parent, true));
    true
}

/// Register-only endpoint operations: 0 metadata, 1 send, 2 receive.
/// Queues are bounded and independent of compositor inboxes. The scheduler lock
/// serializes endpoint metadata and FIFO changes; no reference spans scheduling.
pub fn stdio(operation: u64, payload: u32) -> u64 {
    stdio_selected(operation, payload, 0)
}
/// Select only an existing owned child using child PID + 1; zero preserves the
/// calling process's original redirected standard stream. Not a global handle.
pub fn stdio_selected(operation: u64, payload: u32, selector: u64) -> u64 {
    if operation > 2 || selector > u64::from(u32::MAX) + 1 || (operation == 0 && selector != 0) {
        return ERROR;
    }
    let mut sched = SCHEDULER.lock();
    let Some(pid) = sched.current_pid() else {
        return ERROR;
    };
    let Some(proc) = sched.processes.get(pid as usize).and_then(Option::as_ref) else {
        return ERROR;
    };
    if operation == 0 {
        return proc
            .io_peer
            .map(|(peer, child)| ((u64::from(peer) + 1) << 1) | u64::from(child))
            .unwrap_or(0);
    }
    let (peer, nested) = if selector == 0 {
        let Some((peer, _)) = proc.io_peer else {
            return ERROR;
        };
        (peer, false)
    } else {
        let peer = (selector - 1) as u32;
        if proc.io_nested_peer == Some(peer) {
            (peer, true)
        } else if proc.io_peer == Some((peer, false)) {
            (peer, false)
        } else {
            return ERROR;
        }
    };
    let inbox = if nested {
        &proc.io_nested_inbox
    } else {
        &proc.io_inbox
    };
    if operation == 2 && !inbox.is_empty() {
        let proc = sched.processes[pid as usize]
            .as_mut()
            .expect("validated endpoint");
        let inbox = if nested {
            &mut proc.io_nested_inbox
        } else {
            &mut proc.io_inbox
        };
        let word = inbox.pop_front().expect("nonempty endpoint queue");
        events::wake_locked(&mut sched);
        return u64::from(word) + 1;
    }
    let Some(target) = sched
        .processes
        .get_mut(peer as usize)
        .and_then(Option::as_mut)
    else {
        return ERROR;
    };
    if target.state == ProcessState::Zombie {
        return ERROR;
    }
    match operation {
        1 => {
            let inbox = if target.io_peer.is_some_and(|(source, _)| source == pid) {
                &mut target.io_inbox
            } else if target.io_nested_peer == Some(pid) {
                &mut target.io_nested_inbox
            } else {
                return ERROR;
            };
            if inbox.len() == STDIO_QUEUE_CAPACITY {
                return 1;
            }
            inbox.push_back(payload);
            events::wake_locked(&mut sched);
            0
        }
        2 => 0,
        _ => ERROR,
    }
}
fn owned_stdio_child(process: &Process) -> Option<Pid> {
    process.io_nested_peer.or_else(|| {
        process
            .io_peer
            .and_then(|(pid, child)| if child { None } else { Some(pid) })
    })
}

/// An owned endpoint cannot outlive its host. Stack and address-space owners
/// remain in the deferred reaper until every CPU has stopped executing them.
pub(super) fn stop_owned_child_locked(sched: &mut Scheduler, parent: Pid) {
    // Every connection is to a previously unconnected process, so ownership is
    // acyclic. Both traversals are bounded by the 128 live PCB slots, allocate
    // nothing and use constant stack space even for a deep descendant chain.
    let mut current = parent;
    for _ in 0..128 {
        let Some(child) = sched
            .processes
            .get(current as usize)
            .and_then(Option::as_ref)
            .and_then(owned_stdio_child)
        else {
            break;
        };
        let Some(p) = sched
            .processes
            .get_mut(child as usize)
            .and_then(Option::as_mut)
        else {
            break;
        };
        if p.state != ProcessState::Zombie {
            p.state = ProcessState::Zombie;
            p.files.clear();
            p.exit_code = 9;
            p.exit_cause = rustos_process_abi::supervision::Cause::Killed;
        }
        crate::net::cleanup(child);
        crate::audio::cleanup(child);
        stop_threads_locked(sched, child);
        current = child;
    }
    // Wake/reap from leaf to root. A waiter can reap a whole stopped subtree,
    // so its descendants must have had their waiters woken first.
    for _ in 0..128 {
        if current == parent {
            break;
        }
        let upstream = sched
            .processes
            .get(current as usize)
            .and_then(Option::as_ref)
            .and_then(|p| p.io_peer)
            .filter(|(_, child)| *child)
            .map(|(pid, _)| pid);
        wake_waiters_locked(sched, current);
        let Some(upstream) = upstream else {
            break;
        };
        current = upstream;
    }
}

pub(super) fn reap_connection_locked(sched: &mut Scheduler, pid: Pid) {
    let mut current = pid;
    for _ in 0..128 {
        let Some(p) = sched
            .processes
            .get(current as usize)
            .and_then(Option::as_ref)
        else {
            break;
        };
        let endpoint = p.io_peer;
        let downstream = owned_stdio_child(p);
        if let Some((peer, true)) = endpoint {
            if let Some(parent) = sched
                .processes
                .get_mut(peer as usize)
                .and_then(Option::as_mut)
            {
                if parent.io_peer == Some((current, false)) {
                    parent.io_peer = None;
                    parent.io_inbox = VecDeque::new();
                }
                if parent.io_nested_peer == Some(current) {
                    parent.io_nested_peer = None;
                    parent.io_nested_inbox = VecDeque::new();
                }
            }
        }
        if current != pid {
            retire_process(sched, current);
        }
        if sched.init_pid.is_some() { break; }
        let Some(child) = downstream else {
            break;
        };
        current = child;
    }
}
