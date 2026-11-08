//! Side-effect-free bridge between activation jobs and process supervision.
use crate::{
    config::Kind,
    graph::Catalog,
    transaction::{Command, Engine, Error, Outcome},
    Completion, Invalid, Policy, ProcessKey, Start, State, Supervisor,
};
/// Runtime effects carry launch tokens or exact process identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start(Start),
    Terminate { unit: usize, process: ProcessKey },
}
/// Bounded native/host adapter. All syscalls, clocks and waits belong to callers.
pub struct Manager {
    engine: Engine,
    supervisor: Supervisor<64>,
    jobs: [Option<u64>; 64],
    termination_sent: u64,
}
impl Manager {
    pub fn new(catalog: Catalog) -> Result<Self, Invalid> {
        let mut policies = [Policy {
            restart: crate::Restart::Never,
            delay: 1,
            maximum_delay: 1,
            burst: 1,
            window: 1,
            healthy_reset: 1,
        }; 64];
        for (id, policy) in policies.iter_mut().enumerate().take(catalog.len()) {
            *policy = catalog.unit(id).ok_or(Invalid)?.policy;
        }
        Ok(Self {
            engine: Engine::new(catalog),
            supervisor: Supervisor::new_stopped(policies)?,
            jobs: [None; 64],
            termination_sent: 0,
        })
    }
    pub fn catalog(&self) -> &Catalog {
        self.engine.catalog()
    }
    pub fn start(&mut self, name: &str) -> Result<u64, Error> {
        self.engine.begin_start(name)
    }
    pub fn stop(&mut self, name: &str) -> Result<u64, Error> {
        self.engine.begin_stop(name)
    }
    /// Drain after events/results or a deadline. A bounded catalog scan reconciles
    /// desired state; callers never wake periodically to poll services.
    pub fn next_action(&mut self, now: u64) -> Result<Option<Action>, Invalid> {
        loop {
            while let Some(command) = self.engine.next_command() {
                let job = self.engine.job_id().ok_or(Invalid)?;
                match command {
                    Command::Start(unit) => {
                        self.supervisor.request_start(unit)?;
                        self.jobs[unit] = Some(job);
                    }
                    Command::Stop(unit) => {
                        let key = self.supervisor.stop(unit)?;
                        if key.is_none()
                            && !matches!(self.supervisor.state(unit), Some(State::Spawning(_)))
                        {
                            self.engine.stopped(job, unit).map_err(|_| Invalid)?;
                        }
                    }
                }
            }
            // Failed/removed branches lose restart desire even if they have a pending
            // backoff and no process for transaction cleanup to terminate.
            for unit in 0..self.catalog().len() {
                let orphaned = !self.engine.wanted(unit)
                    && self.jobs[unit].is_none()
                    && (self.outcome() != Outcome::Running || !self.engine.active(unit));
                if orphaned {
                    self.supervisor.stop(unit)?;
                }
                if let Some(State::Stopping(process)) = self.supervisor.state(unit) {
                    if self.termination_sent & (1u64 << unit) == 0 {
                        self.termination_sent |= 1u64 << unit;
                        return Ok(Some(Action::Terminate { unit, process }));
                    }
                }
            }
            if let Some(start) = self.supervisor.next_start(now) {
                return Ok(Some(Action::Start(start)));
            }
            let mut failed = false;
            for unit in 0..self.catalog().len() {
                if self.supervisor.state(unit) == Some(State::Failed) {
                    if let Some(job) = self.jobs[unit].take() {
                        self.engine
                            .started(job, unit, Err(()))
                            .map_err(|_| Invalid)?;
                        self.supervisor.stop(unit)?;
                        failed = true;
                    }
                }
            }
            if !failed {
                return Ok(None);
            }
        }
    }
    pub fn spawned(&mut self, start: Start, key: ProcessKey, now: u64) -> Result<(), Invalid> {
        let cancelled = self.supervisor.spawned(start, key, now)?;
        if cancelled {
            return Ok(());
        } // next_action emits exact-identity termination
        if let Some(job) = self.jobs[start.unit].take() {
            self.engine
                .started(job, start.unit, Ok(()))
                .map_err(|_| Invalid)?;
        } else {
            self.engine.restored(start.unit).map_err(|_| Invalid)?;
        }
        Ok(())
    }
    pub fn spawn_failed(&mut self, start: Start, now: u64) -> Result<(), Invalid> {
        self.supervisor.spawn_failed(start, now)?;
        if let Some(job) = self.jobs[start.unit].take() {
            self.engine
                .started(job, start.unit, Err(()))
                .map_err(|_| Invalid)?;
            self.supervisor.stop(start.unit)?;
        }
        Ok(())
    }
    /// A stale event has no side effects, including on transaction state.
    pub fn completed(&mut self, key: ProcessKey, cause: Completion, now: u64) -> Option<usize> {
        let unit = (0..self.catalog().len()).find(|&id|
            matches!(self.supervisor.state(id),Some(State::Running(k)|State::Stopping(k)) if k==key))?;
        if !self.supervisor.completed(key, cause, now) {
            return None;
        }
        self.termination_sent &= !(1u64 << unit);
        // Catalog indices here always denote services with validated identities.
        if matches!(self.supervisor.state(unit), Some(State::Backoff(_))) {
            self.jobs[unit] = self.engine.retrying(unit).ok()?;
        } else {
            self.engine.unavailable(unit).ok()?;
        }
        Some(unit)
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.supervisor.next_deadline()
    }
    pub fn outcome(&self) -> Outcome {
        self.engine.outcome()
    }
    pub fn state(&self, unit: usize) -> Option<State> {
        self.catalog()
            .unit(unit)
            .filter(|u| u.kind == Kind::Service)
            .and_then(|_| self.supervisor.state(unit))
    }
}
