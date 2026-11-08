//! Atomic process-local readiness check/park and event-driven wakeups.
//! The entry gate protects frame handoff; SCHEDULER protects queues and waiters.
//! No user pointers or service locks survive parking. IRQs only publish input;
//! ordinary kernel entry performs bounded scans of the reserved thread table.
use super::{
    set_return, BlockReason, Pid, ProcessState, Scheduler, ThreadState, ERROR, INPUT_FOCUS,
    PROCESS_BLOCK_SENTINEL, SCHEDULER,
};
use core::sync::atomic::Ordering;
use rustos_thread_abi as abi;

fn ready(s: &Scheduler, pid: Pid, mask: u64) -> u64 {
    let Some(process) = s.processes.get(pid as usize).and_then(Option::as_ref) else {
        return ERROR;
    };
    let mut result = 0;
    if mask & abi::EVENT_CHILD != 0
        && s.processes.iter().flatten().any(|p| p.parent == Some(pid) && (p.state == ProcessState::Zombie || p.adoption_pending))
    {
        result |= abi::EVENT_CHILD;
    }
    if mask & abi::EVENT_POINTER != 0 {
        if pid != 0 {
            return ERROR;
        }
        if crate::drivers::input::mouse::revision() != process.pointer_seen {
            result |= abi::EVENT_POINTER;
        }
    }
    if mask & abi::EVENT_MAILBOX != 0 && process.inbox_len != 0 {
        result |= abi::EVENT_MAILBOX;
    }
    if mask & (abi::EVENT_STDIO_READ | abi::EVENT_STDIO_WRITE) != 0 {
        let Some((peer, _)) = process.io_peer else {
            return ERROR;
        };
        let target = s.processes.get(peer as usize).and_then(Option::as_ref);
        let closed = target
            .map(|p| p.state == ProcessState::Zombie)
            .unwrap_or(true);
        // EOF/error is readiness: the next I/O operation observes disconnection.
        if closed || !process.io_inbox.is_empty() {
            result |= abi::EVENT_STDIO_READ;
        }
        if closed
            || target
                .map(|p| {
                    if p.io_nested_peer == Some(pid) {
                        p.io_nested_inbox.len() < super::STDIO_QUEUE_CAPACITY
                    } else {
                        p.io_inbox.len() < super::STDIO_QUEUE_CAPACITY
                    }
                })
                .unwrap_or(false)
        {
            result |= abi::EVENT_STDIO_WRITE;
        }
    }
    if mask & abi::EVENT_KEYBOARD != 0
        && INPUT_FOCUS.load(Ordering::Relaxed) == u64::from(pid)
        && crate::drivers::input::keyboard::has_scancodes()
    {
        result |= abi::EVENT_KEYBOARD;
    }
    result & mask
}

/// Park only the calling thread until any requested condition is ready.
/// Zero timeout waits indefinitely; success returns a readiness mask (0 timeout).
/// Readiness is a hint, so consumers must recheck I/O after a wake.
pub fn wait_events(mask: u64, timeout: u64) -> u64 {
    if mask == 0 || mask & !abi::EVENT_ALL != 0 || timeout > i64::MAX as u64 {
        return ERROR;
    }
    let deadline = if timeout == 0 {
        None
    } else {
        let Some(end) = crate::interrupts::ticks().checked_add(timeout) else {
            return ERROR;
        };
        Some(end)
    };
    let mut s = SCHEDULER.lock();
    let Some(pid) = s.current_pid() else {
        return ERROR;
    };
    let result = ready(&s, pid, mask);
    if result != 0 {
        return result;
    }
    let Some(tid) = s.current_tid() else {
        return ERROR;
    };
    let thread = s.thread_mut(tid).expect("readiness caller");
    thread.state = ThreadState::Blocked;
    thread.reason = BlockReason::Events(mask, deadline);
    PROCESS_BLOCK_SENTINEL
}

pub(super) fn wake_locked(s: &mut Scheduler) {
    let now = crate::interrupts::ticks();
    for index in 0..s.threads.len() {
        let Some(thread) = s.threads[index].as_ref() else {
            continue;
        };
        if thread.state != ThreadState::Blocked {
            continue;
        }
        let BlockReason::Events(mask, deadline) = thread.reason else {
            continue;
        };
        let result = ready(s, thread.pid, mask);
        let expired = deadline
            .map(|end| now.wrapping_sub(end) as i64 >= 0)
            .unwrap_or(false);
        if result != 0 || expired {
            let thread = s.threads[index].as_mut().expect("locked readiness waiter");
            set_return(thread, result);
            thread.state = ThreadState::Ready;
            thread.reason = BlockReason::None;
            s.ready_queue.push_back(thread.tid);
        }
    }
}
