//! Serialized activation transactions. Runtime side effects remain outside this module.
use crate::graph::Catalog;
/// Native process slots reserved for services, leaving recovery/application headroom.
pub const MAX_SERVICES: usize = 56;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start(usize),
    Stop(usize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Running,
    Succeeded,
    Degraded,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    Unknown,
    Limit,
    InvalidResult,
    Sequence,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Pending,
    Starting,
    Done,
    Failed,
}
struct Transaction {
    id: u64,
    root: usize,
    stopping: bool,
    selected: u64,
    order: alloc::vec::Vec<usize>,
    phases: [Phase; 64],
    outcome: Outcome,
    cleanup: Option<(u64, u64, Outcome)>,
    pending_stop: Option<usize>,
}
/// A manager owns one catalog and at most one activation/cleanup transaction.
pub struct Engine {
    catalog: Catalog,
    owners: [u64; 64],
    active: u64,
    resources: u64,
    sequence: u64,
    order: [usize; 64],
    order_len: usize,
    transaction: Option<Transaction>,
}
impl Engine {
    pub fn new(catalog: Catalog) -> Self {
        Self {
            catalog,
            owners: [0; 64],
            active: 0,
            resources: 0,
            sequence: 0,
            order: [0; 64],
            order_len: 0,
            transaction: None,
        }
    }
    /// Bind runtime callbacks to the currently admitted transaction.
    pub fn job_id(&self) -> Option<u64> {
        self.transaction.as_ref().map(|tx| tx.id)
    }
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    fn busy(&self) -> bool {
        self.transaction
            .as_ref()
            .is_some_and(|t| t.outcome == Outcome::Running)
    }
    pub fn begin_start(&mut self, name: &str) -> Result<u64, Error> {
        let root = self.catalog.lookup(name).ok_or(Error::Unknown)?;
        if self.busy() {
            let tx = self.transaction.as_ref().ok_or(Error::InvalidResult)?;
            return if tx.root == root && !tx.stopping {
                Ok(tx.id)
            } else {
                Err(Error::Busy)
            };
        }
        let plan = self.catalog.plan_start(name).map_err(|_| Error::Limit)?;
        let reserved = self
            .owners
            .iter()
            .fold(plan.selected, |all, mask| all | mask);
        let services = (0..self.catalog.len())
            .filter(|&id| {
                reserved & (1u64 << id) != 0
                    && self
                        .catalog
                        .unit(id)
                        .is_some_and(|u| u.kind == crate::config::Kind::Service)
            })
            .count();
        if services > MAX_SERVICES {
            return Err(Error::Limit);
        }
        let next = self.sequence.checked_add(1).ok_or(Error::Sequence)?;
        let mut phases = [Phase::Idle; 64];
        for &id in &plan.order {
            phases[id] = Phase::Pending;
        }
        self.sequence = next;
        self.transaction = Some(Transaction {
            id: next,
            root,
            stopping: false,
            selected: plan.selected,
            order: plan.order,
            phases,
            outcome: Outcome::Running,
            cleanup: None,
            pending_stop: None,
        });
        Ok(next)
    }
    pub fn begin_stop(&mut self, name: &str) -> Result<u64, Error> {
        let root = self.catalog.lookup(name).ok_or(Error::Unknown)?;
        if self.busy() {
            return Err(Error::Busy);
        }
        let next = self.sequence.checked_add(1).ok_or(Error::Sequence)?;
        let retain = self.retained_except(Some(root));
        self.sequence = next;
        self.transaction = Some(Transaction {
            id: next,
            root,
            stopping: true,
            selected: self.owners[root],
            order: alloc::vec::Vec::new(),
            phases: [Phase::Idle; 64],
            outcome: Outcome::Running,
            cleanup: Some((0, retain, Outcome::Succeeded)),
            pending_stop: None,
        });
        Ok(next)
    }
    fn retained_except(&self, excluded: Option<usize>) -> u64 {
        self.owners
            .iter()
            .enumerate()
            .filter(|(id, _)| Some(*id) != excluded)
            .fold(0, |all, (_, mask)| all | mask)
    }
    /// Issue side effects only after whole-graph admission. Independent starts
    /// may be outstanding together; cleanup stops wait for actual completion.
    pub fn next_command(&mut self) -> Option<Command> {
        let mut tx = self.transaction.take()?;
        if tx.outcome != Outcome::Running {
            self.transaction = Some(tx);
            return None;
        }
        loop {
            if let Some((desired, retain, result)) = tx.cleanup {
                if tx.pending_stop.is_some() {
                    self.transaction = Some(tx);
                    return None;
                }
                if let Some(id) = self.order[..self.order_len]
                    .iter()
                    .rev()
                    .copied()
                    .find(|&id| {
                        self.resources & (1u64 << id) != 0 && (desired | retain) & (1u64 << id) == 0
                    })
                {
                    tx.pending_stop = Some(id);
                    self.transaction = Some(tx);
                    return Some(Command::Stop(id));
                }
                self.active &= desired | retain;
                if tx.stopping {
                    self.owners[tx.root] = 0;
                } else if result != Outcome::Failed {
                    self.owners[tx.root] = desired;
                }
                tx.outcome = result;
                self.transaction = Some(tx);
                return None;
            }
            let mut progress = false;
            for &id in &tx.order {
                if tx.phases[id] == Phase::Done
                    && self
                        .catalog
                        .unit(id)
                        .is_some_and(|u| u.kind == crate::config::Kind::Target)
                {
                    let required = self.catalog.edges(id).unwrap_or_default().requires;
                    if (0..self.catalog.len()).any(|n| {
                        required & (1u64 << n) != 0
                            && matches!(tx.phases[n], Phase::Pending | Phase::Starting)
                    }) {
                        tx.phases[id] = Phase::Pending;
                        self.active &= !(1u64 << id);
                        progress = true;
                    }
                }
            }
            for &id in &tx.order {
                if !matches!(tx.phases[id], Phase::Pending | Phase::Done) {
                    continue;
                }
                let edges = self.catalog.edges(id).unwrap_or_default();
                if (0..self.catalog.len())
                    .any(|n| edges.requires & (1u64 << n) != 0 && tx.phases[n] == Phase::Failed)
                {
                    tx.phases[id] = Phase::Failed;
                    progress = true;
                    continue;
                }
                if tx.phases[id] == Phase::Done {
                    continue;
                }
                let barriers = (edges.requires | edges.after) & tx.selected;
                if (0..self.catalog.len()).any(|n| {
                    barriers & (1u64 << n) != 0
                        && matches!(tx.phases[n], Phase::Pending | Phase::Starting)
                }) {
                    continue;
                }
                let target = self
                    .catalog
                    .unit(id)
                    .is_some_and(|u| u.kind == crate::config::Kind::Target);
                if target || self.active & (1u64 << id) != 0 {
                    tx.phases[id] = Phase::Done;
                    self.active |= 1u64 << id;
                    progress = true;
                } else {
                    tx.phases[id] = Phase::Starting;
                    self.transaction = Some(tx);
                    return Some(Command::Start(id));
                }
            }
            if tx
                .phases
                .iter()
                .any(|p| matches!(p, Phase::Pending | Phase::Starting))
            {
                if progress {
                    continue;
                }
                self.transaction = Some(tx);
                return None;
            }
            let failed = tx.phases[tx.root] == Phase::Failed;
            let result = if failed {
                Outcome::Failed
            } else if tx.phases.contains(&Phase::Failed) {
                Outcome::Degraded
            } else {
                Outcome::Succeeded
            };
            let mut desired = if failed { 0 } else { 1u64 << tx.root };
            loop {
                let mut expanded = desired;
                for id in 0..self.catalog.len() {
                    if desired & (1u64 << id) == 0 {
                        continue;
                    }
                    let edges = self.catalog.edges(id).unwrap_or_default();
                    for n in 0..self.catalog.len() {
                        if (edges.requires | edges.wants) & (1u64 << n) != 0
                            && tx.phases[n] == Phase::Done
                        {
                            expanded |= 1u64 << n;
                        }
                    }
                }
                if desired == expanded {
                    break;
                }
                desired = expanded;
            }
            let retain = self.retained_except(if failed { None } else { Some(tx.root) });
            tx.cleanup = Some((desired, retain, result));
        }
    }
    /// Report successful publication or final failed activation, never an early
    /// stop acknowledgement or an unrelated process result.
    pub fn started(&mut self, job: u64, unit: usize, result: Result<(), ()>) -> Result<(), Error> {
        let tx = self.transaction.as_mut().ok_or(Error::InvalidResult)?;
        if tx.id != job || tx.cleanup.is_some() || tx.phases.get(unit) != Some(&Phase::Starting) {
            return Err(Error::InvalidResult);
        }
        if result.is_ok() {
            if self.resources & (1u64 << unit) == 0 {
                if self.order_len >= self.order.len() {
                    return Err(Error::Limit);
                }
                self.resources |= 1u64 << unit;
                self.order[self.order_len] = unit;
                self.order_len += 1;
            }
            self.active |= 1u64 << unit;
            tx.phases[unit] = Phase::Done;
        } else {
            tx.phases[unit] = Phase::Failed;
        }
        Ok(())
    }
    pub fn stopped(&mut self, job: u64, unit: usize) -> Result<(), Error> {
        let tx = self.transaction.as_mut().ok_or(Error::InvalidResult)?;
        if tx.id != job || tx.pending_stop != Some(unit) {
            return Err(Error::InvalidResult);
        }
        tx.pending_stop = None;
        self.active &= !(1u64 << unit);
        self.resources &= !(1u64 << unit);
        self.remove_order(unit);
        Ok(())
    }
    fn remove_order(&mut self, unit: usize) {
        if let Some(index) = self.order[..self.order_len]
            .iter()
            .position(|&id| id == unit)
        {
            self.order.copy_within(index + 1..self.order_len, index);
            self.order_len -= 1;
        }
    }
    /// Runtime has verified the current process identity before reporting loss.
    pub fn unavailable(&mut self, unit: usize) -> Result<(), Error> {
        if !self
            .catalog
            .unit(unit)
            .is_some_and(|u| u.kind == crate::config::Kind::Service)
        {
            return Err(Error::InvalidResult);
        }
        self.active &= !(1u64 << unit);
        self.resources &= !(1u64 << unit);
        self.remove_order(unit);
        if let Some(tx) = self.transaction.as_mut() {
            if tx.outcome == Outcome::Running {
                if tx.pending_stop == Some(unit) {
                    tx.pending_stop = None;
                }
                if tx.cleanup.is_none() && tx.phases[unit] == Phase::Done {
                    tx.phases[unit] = Phase::Failed;
                }
            }
        }
        Ok(())
    }
    /// Reserve ownership before a runtime launch can publish a process. A stop
    /// must account for this reservation until spawn failure or actual completion.
    pub fn reserve_run(&mut self, unit: usize) -> Result<(), Error> {
        let selected = self.transaction.as_ref().is_some_and(|tx| {
            tx.outcome == Outcome::Running
                && tx.cleanup.is_none()
                && tx.phases.get(unit) == Some(&Phase::Starting)
        });
        if !self
            .catalog
            .unit(unit)
            .is_some_and(|u| u.kind == crate::config::Kind::Service)
            || (!self.wanted(unit) && !selected)
            || self.resources & (1u64 << unit) != 0
        {
            return Err(Error::InvalidResult);
        }
        if self.order_len >= self.order.len() {
            return Err(Error::Limit);
        }
        self.resources |= 1u64 << unit;
        self.order[self.order_len] = unit;
        self.order_len += 1;
        Ok(())
    }
    /// Publication establishes actual process-start order without making it ready.
    pub fn process_spawned(&mut self, unit: usize) -> Result<(), Error> {
        if unit >= self.catalog.len() || self.resources & (1u64 << unit) == 0 {
            return Err(Error::InvalidResult);
        }
        self.remove_order(unit);
        self.order[self.order_len] = unit;
        self.order_len += 1;
        Ok(())
    }
    /// Withdraw readiness while retaining ownership of a process being terminated.
    pub fn hold_for_restart(&mut self, unit: usize) -> Result<Option<u64>, Error> {
        if !self
            .catalog
            .unit(unit)
            .is_some_and(|u| u.kind == crate::config::Kind::Service)
        {
            return Err(Error::InvalidResult);
        }
        let pending = self
            .transaction
            .as_ref()
            .filter(|tx| {
                tx.outcome == Outcome::Running
                    && tx.cleanup.is_none()
                    && tx.phases[unit] == Phase::Done
            })
            .map(|tx| tx.id);
        self.active &= !(1u64 << unit);
        if pending.is_some() {
            if let Some(tx) = self.transaction.as_mut() {
                tx.phases[unit] = Phase::Starting;
            }
        }
        Ok(pending)
    }
    /// Actual retryable completion releases resources but keeps activation pending.
    pub fn retrying(&mut self, unit: usize) -> Result<Option<u64>, Error> {
        let pending = self.hold_for_restart(unit)?;
        self.resources &= !(1u64 << unit);
        self.remove_order(unit);
        if let Some(tx) = self.transaction.as_mut() {
            if tx.pending_stop == Some(unit) {
                tx.pending_stop = None;
            }
        }
        Ok(pending)
    }
    /// Publish an automatic replacement for a retained service. The runtime
    /// validates its launch token and process identity before calling this.
    pub fn restored(&mut self, unit: usize) -> Result<(), Error> {
        if !self.wanted(unit)
            || self.active(unit)
            || !self
                .catalog
                .unit(unit)
                .is_some_and(|u| u.kind == crate::config::Kind::Service)
            || self.resources & (1u64 << unit) == 0 && self.order_len >= self.order.len()
        {
            return Err(Error::InvalidResult);
        }
        self.active |= 1u64 << unit;
        if self.resources & (1u64 << unit) == 0 {
            self.resources |= 1u64 << unit;
            self.order[self.order_len] = unit;
            self.order_len += 1;
        }
        Ok(())
    }
    /// Desired activation ownership, including an admitted stop's withdrawal.
    /// Active resources remain until their ordered native completion is acknowledged.
    pub fn wanted(&self, unit: usize) -> bool {
        if unit >= self.catalog.len() {
            return false;
        }
        let mask = match self.transaction.as_ref() {
            Some(tx) if tx.stopping && tx.outcome == Outcome::Running => tx
                .cleanup
                .map(|(desired, retain, _)| desired | retain)
                .unwrap_or(0),
            _ => self.retained_except(None),
        };
        mask & (1u64 << unit) != 0
    }
    pub fn outcome(&self) -> Outcome {
        self.transaction
            .as_ref()
            .map(|t| t.outcome)
            .unwrap_or(Outcome::Succeeded)
    }
    pub fn active(&self, unit: usize) -> bool {
        unit < self.catalog.len() && self.active & (1u64 << unit) != 0
    }
}
