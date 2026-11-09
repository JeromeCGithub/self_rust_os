//! Per-run native grants. Kernel endpoint identity supplies provenance; unit/run
//! claims in payloads never select a different service. Log intake performs no I/O.
use rustos_service_core::{
    config::{Readiness, Unit},
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
    key: ipc::ProcessKey,
    run: u64,
    init: abi::Message<'static>,
    init_sent: bool,
    closing: bool,
    notify_budget: Budget,
    log_budget: Budget,
    notify_resume: Option<u64>,
    log_resume: Option<u64>,
}
impl Run {
    pub fn launch(unit: &Unit, start: Start) -> Result<Self, ()> {
        // Complete fallible local setup before publishing a child. After a
        // successful spawn the returned Run must account for that live process.
        let notify_budget = Budget::new(8, 1).map_err(|_| ())?;
        let log_budget = Budget::new(4, 1).map_err(|_| ())?;
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
        let path = unit.executable.as_deref().ok_or(())?;
        let pid = ipc::spawn(
            path,
            &args[..unit.arguments.len()],
            &env[..unit.environment.len()],
            &mut [
                Binding {
                    endpoint: &mut child_notify,
                    tag: abi::NOTIFY_TAG,
                    rights: Rights::from_bits(3).map_err(|_| ())?,
                },
                Binding {
                    endpoint: &mut child_log,
                    tag: abi::LOG_TAG,
                    rights: Rights::SEND,
                },
            ],
        )?;
        Ok(Self {
            notify,
            log,
            key: ipc::ProcessKey { pid, generation: 1 },
            run: start.run,
            init: abi::Message::Init {
                run: start.run,
                notify: unit.readiness == Readiness::Notify,
                watchdog: unit.watchdog,
            },
            init_sent: false,
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
    pub fn next_deadline(&self) -> Option<u64> {
        self.runs
            .iter()
            .flatten()
            .flat_map(|r| [r.notify_resume, r.log_resume])
            .flatten()
            .min()
    }
    pub fn drain(&mut self, manager: &mut Manager) -> Result<(), ()> {
        for _ in 0..64 {
            let Some(event) = ipc::next()? else {
                break;
            };
            let Some((unit, notify)) = self.runs.iter().enumerate().find_map(|(unit, slot)| {
                let run = slot.as_ref()?;
                if run.notify.raw() == Some(event.endpoint) {
                    Some((unit, true))
                } else if run.log.raw() == Some(event.endpoint) {
                    Some((unit, false))
                } else {
                    None
                }
            }) else {
                self.rejected = self.rejected.saturating_add(1);
                continue;
            };
            let run = self.runs[unit].as_mut().ok_or(())?;
            if event.kind == Kind::Writable {
                if notify {
                    run.initialize()?;
                }
                continue;
            }
            if event.kind == Kind::PeerClosed {
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
