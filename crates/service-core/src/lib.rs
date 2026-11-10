//! Native supervision policy and bounded configuration, independent of OS syscalls.
#![no_std]
extern crate alloc;
/// Native unit definitions and bounded validation.
pub mod config;
/// Immutable unit catalogs and dependency graph validation.
pub mod graph;
/// Bounded structured memory journal; it performs no output I/O.
pub mod journal;
/// Shared native/host adapter for process actions and dependency jobs.
pub mod manager;
/// Target activation jobs and ownership-aware failure cleanup.
pub mod transaction;

/// Kernel-issued process identity; a reused number must have a new generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessKey {
    pub pid: u32,
    pub generation: u32,
}
/// Successful exit is distinct from termination and faults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    Exit(u64),
    Killed,
    Fault(u16),
}
/// Automatic restart policy; explicit stop always overrides this policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    Never,
    OnFailure,
    Always,
}
/// Tick-based limits. All durations must fit the native signed deadline range.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub restart: Restart,
    pub delay: u64,
    pub maximum_delay: u64,
    pub burst: u16,
    pub window: u64,
    pub healthy_reset: u64,
}
/// A service cannot have two published runs at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Spawning(u64),
    /// Published process still awaiting an explicit readiness message.
    Starting(ProcessKey),
    Running(ProcessKey),
    Stopping(ProcessKey),
    Backoff(u64),
    Inactive,
    Failed,
}
/// One reserved start. Native launch results must carry this token back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Start {
    pub unit: usize,
    pub run: u64,
}
/// Invalid configuration, stale launch result, or invalid unit index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;
/// Maximum retained attempts per service; catalog construction rejects larger bursts.
pub const MAX_BURST: usize = 32;
/// One service's bounded bookkeeping, including a cancelled in-flight launch.
#[derive(Clone, Copy)]
struct Instance {
    state: State,
    wanted: bool,
    started: u64,
    healthy_ready: bool,
    attempts: [u64; MAX_BURST],
    head: usize,
    count: usize,
    failures: u32,
}
impl Instance {
    const NEW: Self = Self {
        state: State::Ready,
        wanted: true,
        started: 0,
        healthy_ready: false,
        attempts: [0; MAX_BURST],
        head: 0,
        count: 0,
        failures: 0,
    };
}
/// A fixed-size catalog and its lifecycle state. No heap allocation occurs.
pub struct Supervisor<const N: usize> {
    policies: [Policy; N],
    instances: [Instance; N],
    next_run: u64,
}
impl<const N: usize> Supervisor<N> {
    /// Validate every limit before admitting any launch.
    pub fn new(policies: [Policy; N]) -> Result<Self, Invalid> {
        for p in &policies {
            if p.delay == 0
                || p.maximum_delay < p.delay
                || p.burst == 0
                || usize::from(p.burst) > MAX_BURST
                || p.window == 0
                || p.healthy_reset == 0
                || [p.delay, p.maximum_delay, p.window, p.healthy_reset]
                    .iter()
                    .any(|&n| n > i64::MAX as u64)
            {
                return Err(Invalid);
            }
        }
        Ok(Self {
            policies,
            instances: [Instance::NEW; N],
            next_run: 1,
        })
    }
    /// Construct an inactive catalog for dependency-controlled admission.
    pub fn new_stopped(policies: [Policy; N]) -> Result<Self, Invalid> {
        let mut manager = Self::new(policies)?;
        for instance in &mut manager.instances {
            instance.wanted = false;
            instance.state = State::Inactive;
        }
        Ok(manager)
    }
    /// Request activation without bypassing restart limits or pending cleanup.
    pub fn request_start(&mut self, unit: usize) -> Result<(), Invalid> {
        let instance = self.instances.get_mut(unit).ok_or(Invalid)?;
        if matches!(instance.state, State::Stopping(_))
            || matches!(instance.state, State::Spawning(_)) && !instance.wanted
        {
            return Err(Invalid);
        }
        instance.wanted = true;
        if matches!(instance.state, State::Inactive | State::Failed) {
            instance.state = State::Ready;
        }
        Ok(())
    }
    /// Whether an exact unpublished reservation has lost its activation owner.
    pub(crate) fn launch_unwanted(&self, start: Start) -> bool {
        self.instances.get(start.unit).is_some_and(|instance| {
            instance.state == State::Spawning(start.run) && !instance.wanted
        })
    }
    /// Reserve one eligible launch; each successful reservation counts as an attempt.
    /// Call only after events or the earliest deadline, not on periodic ticks.
    pub fn next_start(&mut self, now: u64) -> Option<Start> {
        for (unit, i) in self.instances.iter_mut().enumerate() {
            if !i.wanted
                || !matches!(i.state, State::Ready)
                    && !matches!(i.state, State::Backoff(d) if now >= d)
            {
                continue;
            }
            let p = self.policies[unit];
            while i.count != 0
                && now
                    .checked_sub(i.attempts[i.head])
                    .is_some_and(|elapsed| elapsed >= p.window)
            {
                i.head = (i.head + 1) % MAX_BURST;
                i.count -= 1;
            }
            if i.count >= usize::from(p.burst) || self.next_run == u64::MAX {
                i.state = State::Failed;
                continue;
            }
            let run = self.next_run;
            self.next_run += 1;
            i.attempts[(i.head + i.count) % MAX_BURST] = now;
            i.count += 1;
            i.state = State::Spawning(run);
            return Some(Start { unit, run });
        }
        None
    }
    /// Associate a reserved launch with its process. True means a concurrent stop
    /// cancelled it: the runtime must terminate this process and drain completion.
    pub fn spawned(&mut self, start: Start, key: ProcessKey, now: u64) -> Result<bool, Invalid> {
        self.publish(start, key, now, true)
    }
    fn publish(
        &mut self,
        start: Start,
        key: ProcessKey,
        now: u64,
        ready: bool,
    ) -> Result<bool, Invalid> {
        // Never let two records acquire the same process, even for a faulty adapter.
        if self.instances.iter().any(|i| {
            matches!(i.state,
            State::Starting(k) | State::Running(k) | State::Stopping(k) if k == key)
        }) {
            return Err(Invalid);
        }
        let i = self.instances.get_mut(start.unit).ok_or(Invalid)?;
        if i.state != State::Spawning(start.run) {
            return Err(Invalid);
        }
        i.started = now;
        i.healthy_ready = ready;
        i.state = if i.wanted {
            if ready {
                State::Running(key)
            } else {
                State::Starting(key)
            }
        } else {
            State::Stopping(key)
        };
        Ok(!i.wanted)
    }
    /// Publish a process that must explicitly announce readiness.
    pub fn spawned_waiting(
        &mut self,
        start: Start,
        key: ProcessKey,
        now: u64,
    ) -> Result<bool, Invalid> {
        self.publish(start, key, now, false)
    }
    /// Accept readiness only for the exact current waiting process.
    pub fn mark_ready(&mut self, key: ProcessKey, now: u64) -> Result<(), Invalid> {
        let instance = self
            .instances
            .iter_mut()
            .find(|i| i.state == State::Starting(key))
            .ok_or(Invalid)?;
        instance.started = now;
        instance.healthy_ready = true;
        instance.state = State::Running(key);
        Ok(())
    }
    /// Stop a failed run without cancelling its configured restart policy.
    pub fn fail_run(&mut self, unit: usize) -> Result<Option<ProcessKey>, Invalid> {
        let instance = self.instances.get_mut(unit).ok_or(Invalid)?;
        match instance.state {
            State::Starting(key) | State::Running(key) | State::Stopping(key) => {
                instance.state = State::Stopping(key);
                Ok(Some(key))
            }
            _ => Ok(None),
        }
    }
    /// Handle a failed launch without inventing a process to reap.
    pub fn spawn_failed(&mut self, start: Start, now: u64) -> Result<(), Invalid> {
        let i = self.instances.get_mut(start.unit).ok_or(Invalid)?;
        if i.state != State::Spawning(start.run) {
            return Err(Invalid);
        }
        Self::finish(i, self.policies[start.unit], false, now);
        Ok(())
    }
    /// Consume a completion only for the currently owned identity. Duplicate or
    /// stale completions leave all records unchanged.
    pub fn completed(&mut self, key: ProcessKey, cause: Completion, now: u64) -> bool {
        let Some(unit) = self.instances.iter().position(|i| {
            matches!(i.state,
            State::Starting(k) | State::Running(k) | State::Stopping(k) if k == key)
        }) else {
            return false;
        };
        let p = self.policies[unit];
        let i = &mut self.instances[unit];
        if i.healthy_ready
            && now
                .checked_sub(i.started)
                .is_some_and(|d| d >= p.healthy_reset)
        {
            i.count = 0;
            i.head = 0;
            i.failures = 0;
        }
        Self::finish(i, p, cause == Completion::Exit(0) && i.healthy_ready, now);
        true
    }
    fn finish(i: &mut Instance, p: Policy, success: bool, now: u64) {
        if !i.wanted {
            i.state = State::Inactive;
            return;
        }
        let retry = p.restart == Restart::Always || p.restart == Restart::OnFailure && !success;
        if !retry {
            i.state = if success {
                State::Inactive
            } else {
                State::Failed
            };
            return;
        }
        let factor = 1u64.checked_shl(i.failures.min(63)).unwrap_or(u64::MAX);
        let delay = p.delay.saturating_mul(factor).min(p.maximum_delay);
        i.failures = i.failures.saturating_add(1);
        i.state = now
            .checked_add(delay)
            .map(State::Backoff)
            .unwrap_or(State::Failed);
    }
    /// A failed activation has no live resource to stop. Cancel retry desire
    /// while preserving the observable failure until an explicit start/stop.
    pub(crate) fn settle_failed(&mut self, unit: usize) -> Result<(), Invalid> {
        let instance = self.instances.get_mut(unit).ok_or(Invalid)?;
        if instance.state != State::Failed {
            return Err(Invalid);
        }
        instance.wanted = false;
        Ok(())
    }
    pub(crate) fn is_settled_failure(&self, unit: usize) -> bool {
        self.instances
            .get(unit)
            .is_some_and(|i| i.state == State::Failed && !i.wanted)
    }
    /// Withdraw restart desire while preserving live state for ordered shutdown.
    pub(crate) fn suppress_restart(&mut self, unit: usize) -> Result<(), Invalid> {
        let instance = self.instances.get_mut(unit).ok_or(Invalid)?;
        instance.wanted = false;
        if matches!(instance.state, State::Ready | State::Backoff(_)) {
            instance.state = State::Inactive;
        }
        Ok(())
    }
    /// Suppress automatic restart before requesting native termination. An in-flight
    /// spawn retains its token so a late successful result can still be cleaned up.
    pub fn stop(&mut self, unit: usize) -> Result<Option<ProcessKey>, Invalid> {
        let i = self.instances.get_mut(unit).ok_or(Invalid)?;
        i.wanted = false;
        match i.state {
            State::Starting(k) | State::Running(k) | State::Stopping(k) => {
                i.state = State::Stopping(k);
                Ok(Some(k))
            }
            State::Spawning(_) => Ok(None),
            _ => {
                i.state = State::Inactive;
                Ok(None)
            }
        }
    }
    /// Observable lifecycle state; invalid indices have no state.
    pub fn state(&self, unit: usize) -> Option<State> {
        self.instances.get(unit).map(|i| i.state)
    }
    /// Earliest restart deadline. None means the runtime may wait indefinitely.
    pub fn next_deadline(&self) -> Option<u64> {
        self.instances
            .iter()
            .filter_map(|i| match i.state {
                State::Backoff(d) => Some(d),
                _ => None,
            })
            .min()
    }
}

pub mod control;
pub mod publication;
