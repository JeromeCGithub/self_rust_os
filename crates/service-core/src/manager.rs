//! Side-effect-free bridge between activation jobs and process supervision.
use crate::{
    config::{Kind, Readiness},
    graph::Catalog,
    transaction::{Command, Engine, Error, Outcome},
    Completion, Invalid, ProcessKey, Start, State, Supervisor,
};
/// Runtime effects carry launch tokens or exact process identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start(Start),
    Terminate { unit: usize, process: ProcessKey },
}
/// Run-scoped messages, authorized by endpoint ownership and complete identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notification {
    Ready,
    Heartbeat,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyError {
    Stale,
    Unexpected,
    Late,
    Invalid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    StartTimeout,
    Watchdog,
    ExitedBeforeReady,
    Spawn,
    Clock,
}
/// Last/current run status, independent of the historical target job outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunStatus {
    pub run: u64,
    pub process: Option<ProcessKey>,
    pub ready: bool,
    pub failure: Option<Failure>,
    pub start_deadline: Option<u64>,
    pub watchdog_deadline: Option<u64>,
    pub completion: Option<Completion>,
}
/// Bounded native/host adapter. All syscalls, clocks and waits belong to callers.
pub struct Manager {
    engine: Engine,
    supervisor: Supervisor<64>,
    jobs: [Option<u64>; 64],
    termination_sent: u64,
    runs: [Option<RunStatus>; 64],
}
impl Manager {
    pub fn new(catalog: Catalog) -> Result<Self, Invalid> {
        let mut policies = [crate::Policy {
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
            runs: [None; 64],
        })
    }
    pub fn catalog(&self) -> &Catalog {
        self.engine.catalog()
    }
    /// Cooperative cancellation for an exact, still-unpublished launch token.
    /// A runtime must still publish and reap a child whose creation raced this check.
    pub fn launch_cancelled(&self, start: Start) -> bool {
        self.supervisor.launch_unwanted(start)
            || self.supervisor.state(start.unit) == Some(State::Spawning(start.run))
                && (self.engine.launch_blocked(start.unit)
                    || self
                        .run_status(start.unit)
                        .is_some_and(|run| run.run == start.run && run.failure.is_some()))
    }
    pub fn run_status(&self, unit: usize) -> Option<RunStatus> {
        self.runs.get(unit).copied().flatten()
    }
    pub fn start(&mut self, name: &str) -> Result<u64, Error> {
        self.engine.begin_start(name)
    }
    /// Preserve owners/dependents while one job covers stop, reap and readiness.
    pub fn restart(&mut self, name: &str) -> Result<u64, Error> {
        let job = self.engine.begin_restart(name)?;
        let unit = self.catalog().lookup(name).ok_or(Error::Unknown)?;
        self.disarm(unit);
        Ok(job)
    }
    pub fn stop(&mut self, name: &str) -> Result<u64, Error> {
        let job = self.engine.begin_stop(name)?;
        let root = self.catalog().lookup(name).ok_or(Error::Unknown)?;
        if !self.engine.wanted(root) && self.supervisor.state(root) == Some(State::Failed) {
            self.supervisor
                .stop(root)
                .map_err(|_| Error::InvalidResult)?;
        }
        for unit in 0..self.catalog().len() {
            if !self.engine.wanted(unit) && self.jobs[unit].is_none() {
                self.disarm(unit);
            }
        }
        Ok(job)
    }
    fn disarm(&mut self, unit: usize) {
        if let Some(run) = self.runs[unit].as_mut() {
            run.start_deadline = None;
            run.watchdog_deadline = None;
        }
    }
    fn publish_ready(&mut self, unit: usize) -> Result<(), Invalid> {
        if let Some(job) = self.jobs[unit].take() {
            self.engine
                .started(job, unit, Ok(()))
                .map_err(|_| Invalid)?;
        } else {
            self.engine.restored(unit).map_err(|_| Invalid)?;
        }
        Ok(())
    }
    /// Caller maps an authenticated endpoint to unit/run and passes its kernel
    /// sender identity. Claimed unit names or IDs inside payload are never authority.
    pub fn notify(
        &mut self,
        unit: usize,
        run: u64,
        key: ProcessKey,
        message: Notification,
        now: u64,
    ) -> Result<(), NotifyError> {
        let status = self
            .run_status(unit)
            .filter(|s| s.run == run && s.process == Some(key))
            .ok_or(NotifyError::Stale)?;
        if status.failure.is_some() {
            return Err(NotifyError::Unexpected);
        }
        match message {
            Notification::Ready => {
                if self.supervisor.state(unit) != Some(State::Starting(key)) || status.ready {
                    return Err(NotifyError::Unexpected);
                }
                if !status.start_deadline.is_some_and(|d| now < d) {
                    return Err(NotifyError::Late);
                }
                let watchdog = self
                    .catalog()
                    .unit(unit)
                    .ok_or(NotifyError::Stale)?
                    .watchdog;
                let deadline = match watchdog {
                    Some(ticks) => Some(now.checked_add(ticks).ok_or(NotifyError::Invalid)?),
                    None => None,
                };
                self.supervisor
                    .mark_ready(key, now)
                    .map_err(|_| NotifyError::Invalid)?;
                let status = self.runs[unit].as_mut().ok_or(NotifyError::Invalid)?;
                status.ready = true;
                status.start_deadline = None;
                status.watchdog_deadline = deadline;
                self.publish_ready(unit).map_err(|_| NotifyError::Invalid)
            }
            Notification::Heartbeat => {
                if !status.ready || self.supervisor.state(unit) != Some(State::Running(key)) {
                    return Err(NotifyError::Unexpected);
                }
                let deadline = status.watchdog_deadline.ok_or(NotifyError::Unexpected)?;
                if now >= deadline {
                    return Err(NotifyError::Late);
                }
                let ticks = self
                    .catalog()
                    .unit(unit)
                    .and_then(|u| u.watchdog)
                    .ok_or(NotifyError::Unexpected)?;
                let deadline = now.checked_add(ticks).ok_or(NotifyError::Invalid)?;
                self.runs[unit]
                    .as_mut()
                    .ok_or(NotifyError::Invalid)?
                    .watchdog_deadline = Some(deadline);
                Ok(())
            }
        }
    }
    fn expire(&mut self, now: u64) -> Result<(), Invalid> {
        for unit in 0..self.catalog().len() {
            let failure = match self.runs[unit] {
                Some(run) if run.failure.is_none() => {
                    if run.start_deadline.is_some_and(|d| now >= d) {
                        Some(Failure::StartTimeout)
                    } else if run.watchdog_deadline.is_some_and(|d| now >= d) {
                        Some(Failure::Watchdog)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(failure) = failure {
                let status = self.runs[unit].as_mut().ok_or(Invalid)?;
                status.failure = Some(failure);
                status.ready = false;
                status.start_deadline = None;
                status.watchdog_deadline = None;
                if self.supervisor.fail_run(unit)?.is_some() {
                    let job = self.engine.hold_for_restart(unit).map_err(|_| Invalid)?;
                    if self.jobs[unit].is_none() {
                        self.jobs[unit] = job;
                    }
                }
            }
        }
        Ok(())
    }
    /// Drain after events/results or a deadline; no periodic service polling.
    pub fn next_action(&mut self, now: u64) -> Result<Option<Action>, Invalid> {
        self.expire(now)?;
        loop {
            while let Some(command) = self.engine.next_command() {
                let job = self.engine.job_id().ok_or(Invalid)?;
                match command {
                    Command::Start(unit) => {
                        self.supervisor.request_start(unit)?;
                        self.jobs[unit] = Some(job);
                    }
                    Command::Stop(unit) => {
                        self.disarm(unit);
                        let key = self.supervisor.stop(unit)?;
                        if key.is_none()
                            && !matches!(self.supervisor.state(unit), Some(State::Spawning(_)))
                        {
                            self.engine.stopped(job, unit).map_err(|_| Invalid)?;
                        }
                    }
                }
            }
            for unit in 0..self.catalog().len() {
                let orphaned = !self.engine.wanted(unit)
                    && self.jobs[unit].is_none()
                    && (self.outcome() != Outcome::Running || !self.engine.active(unit));
                if orphaned {
                    self.disarm(unit);
                    if !self.supervisor.is_settled_failure(unit) {
                        self.supervisor.stop(unit)?;
                    }
                }
                if let Some(State::Stopping(process)) = self.supervisor.state(unit) {
                    if self.termination_sent & (1u64 << unit) == 0 {
                        self.termination_sent |= 1u64 << unit;
                        return Ok(Some(Action::Terminate { unit, process }));
                    }
                }
            }
            if let Some(start) = self.supervisor.next_start(now) {
                self.engine.reserve_run(start.unit).map_err(|_| Invalid)?;
                let ticks = self
                    .catalog()
                    .unit(start.unit)
                    .ok_or(Invalid)?
                    .start_timeout;
                let deadline = now.checked_add(ticks);
                self.runs[start.unit] = Some(RunStatus {
                    run: start.run,
                    process: None,
                    ready: false,
                    failure: if deadline.is_none() {
                        Some(Failure::Clock)
                    } else {
                        None
                    },
                    start_deadline: deadline,
                    watchdog_deadline: None,
                    completion: None,
                });
                if deadline.is_some() {
                    return Ok(Some(Action::Start(start)));
                }
                self.spawn_failed(start, now)?;
                continue;
            }
            let mut failed = false;
            for unit in 0..self.catalog().len() {
                if self.supervisor.state(unit) == Some(State::Failed) {
                    if let Some(job) = self.jobs[unit].take() {
                        self.disarm(unit);
                        self.engine
                            .started(job, unit, Err(()))
                            .map_err(|_| Invalid)?;
                        self.supervisor.settle_failed(unit)?;
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
        let run = self
            .run_status(start.unit)
            .filter(|r| r.run == start.run)
            .ok_or(Invalid)?;
        let late = run.failure.is_some() || run.start_deadline.is_some_and(|d| now >= d);
        let waiting =
            self.catalog().unit(start.unit).ok_or(Invalid)?.readiness == Readiness::Notify;
        let cancelled = if waiting || late {
            self.supervisor.spawned_waiting(start, key, now)?
        } else {
            self.supervisor.spawned(start, key, now)?
        };
        self.engine
            .process_spawned(start.unit)
            .map_err(|_| Invalid)?;
        let status = self.runs[start.unit].as_mut().ok_or(Invalid)?;
        status.process = Some(key);
        if cancelled {
            self.disarm(start.unit);
            return Ok(());
        }
        if late {
            status.failure = status.failure.or(Some(Failure::StartTimeout));
            status.ready = false;
            self.disarm(start.unit);
            self.supervisor.fail_run(start.unit)?;
            return Ok(());
        }
        if !waiting {
            status.ready = true;
            status.start_deadline = None;
            self.publish_ready(start.unit)?;
        }
        Ok(())
    }
    pub fn spawn_failed(&mut self, start: Start, now: u64) -> Result<(), Invalid> {
        if !self
            .run_status(start.unit)
            .is_some_and(|s| s.run == start.run)
        {
            return Err(Invalid);
        }
        self.supervisor.spawn_failed(start, now)?;
        self.disarm(start.unit);
        if let Some(status) = self.runs[start.unit].as_mut() {
            status.failure = status.failure.or(Some(Failure::Spawn));
        }
        if let Some(job) = self.jobs[start.unit].take() {
            self.engine
                .started(job, start.unit, Err(()))
                .map_err(|_| Invalid)?;
            self.supervisor.stop(start.unit)?;
        }
        self.engine.unavailable(start.unit).map_err(|_| Invalid)?;
        Ok(())
    }
    /// Stale events leave both process and transaction state unchanged.
    pub fn completed(&mut self, key: ProcessKey, cause: Completion, now: u64) -> Option<usize> {
        let unit = (0..self.catalog().len()).find(|&id| {
            matches!(self.supervisor.state(id),
            Some(State::Starting(k)|State::Running(k)|State::Stopping(k)) if k==key)
        })?;
        let previous = self.runs[unit];
        let classified = if previous
            .is_some_and(|s| matches!(s.failure, Some(Failure::StartTimeout | Failure::Watchdog)))
            && cause == Completion::Exit(0)
        {
            Completion::Killed
        } else {
            cause
        };
        if !self.supervisor.completed(key, classified, now) {
            return None;
        }
        self.termination_sent &= !(1u64 << unit);
        if let Some(status) = self.runs[unit].as_mut() {
            if !status.ready && status.failure.is_none() {
                status.failure = Some(Failure::ExitedBeforeReady);
            }
            status.ready = false;
            status.process = None;
            status.start_deadline = None;
            status.watchdog_deadline = None;
            status.completion = Some(cause);
        }
        if matches!(self.supervisor.state(unit), Some(State::Backoff(_))) {
            let job = self.engine.retrying(unit).ok()?;
            if self.jobs[unit].is_none() {
                self.jobs[unit] = job;
            }
        } else {
            if let Some(job) = self.jobs[unit].take() {
                self.engine.started(job, unit, Err(())).ok()?;
            }
            self.engine.unavailable(unit).ok()?;
        }
        Some(unit)
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.runs
            .iter()
            .flatten()
            .flat_map(|r| [r.start_deadline, r.watchdog_deadline])
            .flatten()
            .chain(self.supervisor.next_deadline())
            .min()
    }
    pub fn job_id(&self) -> Option<u64> {
        self.engine.job_id()
    }
    pub fn active(&self, unit: usize) -> bool {
        self.engine.active(unit)
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
