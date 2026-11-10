//! Per-run native grants. Kernel endpoint identity supplies provenance; unit/run
//! claims in payloads never select a different service. Log intake performs no I/O.
use rustos_service_core::{
    config::Readiness,
    journal::{Budget, Intake, Journal},
    manager::{Manager, Notification},
    ProcessKey, Start,
};
use userlib::{
    ipc::{self, Binding, Endpoint, Kind, Rights},
    recordln,
    service::abi,
    sys,
};
const UNITS: usize = 64;
const LOG_RECORDS: usize = 256;
pub struct Run {
    notify: Endpoint,
    log: Endpoint,
    control: Option<crate::control_port::Connection>,
    key: ipc::ProcessKey,
    run: u64,
    init: abi::Message<'static>,
    init_sent: bool,
    stop_pending: bool,
    closing: bool,
    notify_budget: Budget,
    log_budget: Budget,
    notify_resume: Option<u64>,
    log_resume: Option<u64>,
}
impl Run {
    pub fn launch(unit: &crate::launcher::Spec, start: Start) -> Result<Self, ()> {
        if start.run == 0 {
            return Err(());
        }
        let image = unit.image()?;
        if unit.cancelled() {
            return Err(());
        }
        // Complete fallible local setup before publishing a child. After a
        // successful spawn the returned Run must account for that live process.
        let notify_budget = Budget::new(8, 1).map_err(|_| ())?;
        let log_budget = Budget::new(4, 1).map_err(|_| ())?;
        if start.run == 0 {
            return Err(());
        }
        let control_budget = Budget::new(4, 1).map_err(|_| ())?;
        let (control, mut child_control) = if unit.control.is_some() {
            let [parent, child] = Endpoint::pair()?;
            (Some(parent), Some(child))
        } else {
            (None, None)
        };
        let [notify, mut child_notify] = Endpoint::pair()?;
        let [log, mut child_log] = Endpoint::pair()?;
        let mut args = [""; 32];
        let mut env = [""; 32];
        for (slot, value) in args.iter_mut().zip(&unit.arguments) {
            *slot = value;
        }
        for (slot, value) in env.iter_mut().zip(&unit.environment) {
            *slot = value;
        }
        let path = unit.executable.as_str();
        let mut bindings = alloc::vec::Vec::new();
        bindings.try_reserve_exact(3).map_err(|_| ())?;
        let duplex = Rights::from_bits(3).map_err(|_| ())?;
        bindings.push(Binding {
            endpoint: &mut child_notify,
            tag: abi::NOTIFY_TAG,
            rights: duplex,
        });
        bindings.push(Binding {
            endpoint: &mut child_log,
            tag: abi::LOG_TAG,
            rights: Rights::SEND,
        });
        if let Some(child) = child_control.as_mut() {
            bindings.push(Binding {
                endpoint: child,
                tag: abi::CONTROL_TAG,
                rights: duplex,
            });
        }
        if unit.cancelled() {
            return Err(());
        }
        let pid = ipc::spawn_image(
            path,
            &image,
            &args[..unit.arguments.len()],
            &env[..unit.environment.len()],
            &mut bindings,
        )?;
        let key = ipc::ProcessKey { pid, generation: 1 };
        let control = match (control, unit.control) {
            (Some(endpoint), Some(access)) => Some(crate::control_port::Connection::new(
                endpoint,
                key,
                start.run,
                access,
                control_budget,
            )),
            _ => None,
        };
        Ok(Self {
            notify,
            log,
            control,
            key: ipc::ProcessKey { pid, generation: 1 },
            run: start.run,
            init: abi::Message::Init {
                run: start.run,
                notify: unit.readiness == Readiness::Notify,
                watchdog: unit.watchdog,
            },
            init_sent: false,
            stop_pending: false,
            closing: false,
            notify_budget,
            log_budget,
            notify_resume: None,
            log_resume: None,
        })
    }
    pub fn key(&self) -> ProcessKey {
        ProcessKey {
            pid: self.key.pid,
            generation: self.key.generation,
        }
    }
    fn send_stop(&mut self) -> Result<(), ()> {
        if !self.stop_pending || !self.init_sent || self.closing {
            return Ok(());
        }
        let mut bytes = [0; abi::MAX_FRAME];
        let length = abi::Message::Stop { run: self.run }
            .encode(&mut bytes)
            .map_err(|_| ())?;
        match self.notify.send(&bytes[..length]) {
            Ok(()) => self.stop_pending = false,
            Err(ipc::SendError::Full) => self.notify.request_writable(true)?,
            Err(_) => {
                self.stop_pending = false;
                return Err(());
            }
        }
        Ok(())
    }
    fn initialize(&mut self) -> Result<(), ()> {
        if self.init_sent || self.closing {
            return Ok(());
        }
        let mut bytes = [0; abi::MAX_FRAME];
        let n = self.init.encode(&mut bytes).map_err(|_| ())?;
        match self.notify.send(&bytes[..n]) {
            Ok(()) => self.init_sent = true,
            Err(ipc::SendError::Full) => self.notify.request_writable(true)?,
            // Early service exit is collected through its durable child event.
            Err(ipc::SendError::Closed) => self.init_sent = true,
            Err(ipc::SendError::Invalid) => return Err(()),
        }
        Ok(())
    }
}
pub struct Ports {
    runs: [Option<Run>; UNITS],
    pub journal: Journal<LOG_RECORDS>,
    pub rejected: u64,
    pub throttled: u64,
}
impl Ports {
    pub fn new() -> Self {
        Self {
            runs: core::array::from_fn(|_| None),
            journal: Journal::new(),
            rejected: 0,
            throttled: 0,
        }
    }
    pub fn install(&mut self, unit: usize, mut run: Run) -> Result<(), ()> {
        let slot = self.runs.get_mut(unit).ok_or(())?;
        if slot.is_some() {
            return Err(());
        }
        run.initialize()?;
        *slot = Some(run);
        Ok(())
    }
    /// Queue one run-scoped cooperative stop after Init. A full channel arms
    /// writable interest; the manager's grace deadline bounds delivery and exit.
    pub fn request_stop(&mut self, unit: usize, run: u64, key: ProcessKey) -> Result<(), ()> {
        let current = self
            .runs
            .get_mut(unit)
            .and_then(Option::as_mut)
            .filter(|current| current.run == run && current.key() == key && !current.closing)
            .ok_or(())?;
        current.stop_pending = true;
        current.initialize()?;
        current.send_stop()
    }
    pub fn mark_exit(&mut self, key: ipc::ProcessKey) {
        for run in self.runs.iter_mut().flatten() {
            if run.key == key {
                run.closing = true;
            }
        }
    }
    /// Keep attribution through the bounded final event batch, then close both
    /// grants before admitting a replacement. Exit storms may discard log tails;
    /// the journal is explicitly volatile and child status never waits on logs.
    pub fn finish_exits(&mut self) {
        for slot in &mut self.runs {
            if slot.as_ref().is_some_and(|r| r.closing) {
                *slot = None;
            }
        }
    }
    pub fn resume(&mut self, now: u64) -> Result<(), ()> {
        for run in self.runs.iter_mut().flatten() {
            if run.notify_resume.is_some_and(|d| now >= d) {
                run.notify.set_readable(true)?;
                run.notify_resume = None;
                run.initialize()?;
            }
            if run.log_resume.is_some_and(|d| now >= d) {
                run.log.set_readable(true)?;
                run.log_resume = None;
            }
        }
        Ok(())
    }
    pub fn refresh_control(&mut self, manager: &Manager) {
        for run in self.runs.iter_mut().flatten().filter(|run| !run.closing) {
            if let Some(control) = run.control.as_mut() {
                control.refresh(manager);
            }
        }
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.runs
            .iter()
            .flatten()
            .flat_map(|r| {
                [
                    r.notify_resume,
                    r.log_resume,
                    r.control.as_ref().and_then(|c| c.deadline()),
                ]
            })
            .flatten()
            .min()
    }
    pub fn drain(&mut self, manager: &mut Manager, wake: ipc::Handle) -> Result<(), ()> {
        for _ in 0..64 {
            let Some(event) = ipc::next()? else {
                break;
            };
            if event.endpoint == wake {
                continue;
            }
            let Some((unit, role)) = self.runs.iter().enumerate().find_map(|(unit, slot)| {
                let run = slot.as_ref()?;
                if run.notify.raw() == Some(event.endpoint) {
                    Some((unit, 0))
                } else if run.log.raw() == Some(event.endpoint) {
                    Some((unit, 1))
                } else if run.control.as_ref().and_then(|c| c.raw()) == Some(event.endpoint) {
                    Some((unit, 2))
                } else {
                    None
                }
            }) else {
                self.rejected = self.rejected.saturating_add(1);
                continue;
            };
            let run = self.runs[unit].as_mut().ok_or(())?;
            if role == 2 {
                if !run.closing {
                    if let Some(control) = run.control.as_mut() {
                        control.event(&event, manager, &self.journal);
                    }
                }
                continue;
            }
            let notify = role == 0;
            if event.kind == Kind::Writable {
                if notify {
                    run.initialize()?;
                    if run.send_stop().is_err() {
                        let _ = manager.stop_delivery_failed(unit, run.run, run.key());
                    }
                }
                continue;
            }
            if event.kind == Kind::PeerClosed {
                if notify {
                    run.stop_pending = false;
                    let _ = manager.stop_delivery_failed(unit, run.run, run.key());
                }
                continue;
            }
            if event.sender != Some(run.key) {
                self.rejected = self.rejected.saturating_add(1);
                continue;
            }
            let now = sys::ticks();
            let intake = if notify {
                run.notify_budget.admit(now)
            } else {
                run.log_budget.admit(now)
            };
            if let Intake::Pause { until } = intake {
                if notify {
                    run.notify.set_readable(false)?;
                    run.notify_resume = until;
                } else {
                    run.log.set_readable(false)?;
                    run.log_resume = until;
                }
                self.throttled = self.throttled.saturating_add(1);
                continue;
            }
            let message = event
                .payload
                .get(..event.length)
                .and_then(|bytes| abi::Message::decode(bytes).ok());
            let key = run.key();
            match (notify, message) {
                (true, Some(abi::Message::Ready { run: token }))
                    if token == run.run && !run.closing =>
                {
                    if manager
                        .notify(unit, token, key, Notification::Ready, now)
                        .is_ok()
                    {
                        recordln!(
                            "[initator] ready name={} unit={} run={} pid={} tick={}",
                            manager.catalog().unit(unit).ok_or(())?.name,
                            unit,
                            token,
                            key.pid,
                            now
                        );
                    } else {
                        self.rejected = self.rejected.saturating_add(1);
                    }
                }
                (true, Some(abi::Message::Heartbeat { run: token }))
                    if token == run.run && !run.closing =>
                {
                    if manager
                        .notify(unit, token, key, Notification::Heartbeat, now)
                        .is_err()
                    {
                        self.rejected = self.rejected.saturating_add(1);
                    }
                }
                (
                    false,
                    Some(abi::Message::Log {
                        run: token,
                        level,
                        text,
                    }),
                ) if token == run.run => {
                    if self
                        .journal
                        .push(unit, token, key, now, level, text)
                        .is_err()
                    {
                        self.rejected = self.rejected.saturating_add(1);
                    }
                }
                _ => self.rejected = self.rejected.saturating_add(1),
            }
        }
        Ok(())
    }
}
