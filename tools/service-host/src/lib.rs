#![feature(linux_pidfd)]
//! Event-driven Linux development transport. Native authority remains in the OS.
#[cfg(not(target_os = "linux"))]
compile_error!("initator-host requires Linux pidfds");
pub mod config;
mod poll;
mod spawn;
mod wire;
use rustos_service_abi::Message;
pub use rustos_service_core::ProcessKey;
use rustos_service_core::{
    graph::Catalog,
    journal::{Budget, Intake, Journal},
    manager::{Action, Manager, Notification},
    Completion, Start,
};
use std::{
    collections::VecDeque,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        linux::process::ChildExt,
        unix::{net::UnixStream, process::ExitStatusExt},
    },
    sync::Arc,
    time::{Duration, Instant},
};
/// Development clock: a core tick is 10 ms. Native PIT timing is independent.
pub const TICK: Duration = Duration::from_millis(10);
const UNITS: usize = 64;
const TRACE: usize = 2048;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Started {
        unit: usize,
        run: u64,
        process: ProcessKey,
        tick: u64,
    },
    Ready {
        unit: usize,
    },
    Completed {
        unit: usize,
        process: ProcessKey,
        cause: Completion,
        tick: u64,
    },
    SpawnFailed {
        unit: usize,
    },
    ProtocolError {
        unit: usize,
    },
}
struct Child {
    process: spawn::Spawned,
    start: Start,
    key: ProcessKey,
    input: Option<UnixStream>,
    output: Option<wire::Frame>,
    stop: Option<wire::Frame>,
    notify: wire::Reader,
    log: wire::Reader,
    budgets: [Budget; 2],
    paused: [bool; 2],
    resume: [Option<u64>; 2],
}
#[derive(Clone, Copy)]
enum Role {
    Wake,
    Exit(usize),
    Input(usize),
    Notify(usize),
    Log(usize),
}
pub struct Runtime {
    pub manager: Manager,
    pub journal: Journal<256>,
    worker: spawn::Worker,
    children: [Option<Child>; UNITS],
    pending: [bool; UNITS],
    queued: [Option<spawn::Request>; UNITS],
    events: VecDeque<Event>,
    pub overwritten_events: u64,
    epoch: Instant,
    next_pid: u32,
    fds: Vec<poll::Fd>,
    roles: Vec<Role>,
    pub waits: u64,
}
impl Runtime {
    pub fn new(catalog: Catalog) -> io::Result<Self> {
        Self::with_launch_hook(catalog, |_| {})
    }
    /// Fault-injection hook executes only on the bounded launch worker. It permits
    /// deterministic tests of blocked loaders without holding the reactor thread.
    pub fn with_launch_hook(
        catalog: Catalog,
        hook: impl Fn(Start) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let manager = Manager::new(catalog).map_err(|_| io::Error::other("invalid policies"))?;
        Ok(Self {
            manager,
            journal: Journal::new(),
            worker: spawn::Worker::new(Arc::new(hook))?,
            children: std::array::from_fn(|_| None),
            pending: [false; UNITS],
            queued: std::array::from_fn(|_| None),
            events: VecDeque::with_capacity(TRACE),
            overwritten_events: 0,
            epoch: Instant::now(),
            next_pid: 2,
            fds: Vec::with_capacity(257),
            roles: Vec::with_capacity(257),
            waits: 0,
        })
    }
    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.events.iter()
    }
    pub fn live_count(&self) -> usize {
        self.children.iter().flatten().count()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.iter().filter(|&&p| p).count()
    }
    pub fn tick(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis() / TICK.as_millis()).unwrap_or(u64::MAX)
    }
    fn record(&mut self, event: Event) {
        if self.events.len() == TRACE {
            self.events.pop_front();
            self.overwritten_events = self.overwritten_events.saturating_add(1);
        }
        self.events.push_back(event);
    }
    /// Close the development command stream, delivering EOF to a cooperative host
    /// fixture. Service stop uses a separate framed Stop request.
    pub fn close_input(&mut self, unit: usize) -> io::Result<()> {
        let child = self
            .children
            .get_mut(unit)
            .and_then(Option::as_mut)
            .ok_or(io::ErrorKind::NotFound)?;
        child.input = None;
        child.output = None;
        child.stop = None;
        Ok(())
    }
    /// Runtime authority uses complete synthetic identity and stable kernel pidfd,
    /// never a recycled Linux numeric PID. Callers of this Rust API own the backend.
    pub fn terminate(&mut self, key: ProcessKey) -> io::Result<()> {
        let child = self
            .children
            .iter_mut()
            .flatten()
            .find(|c| c.key == key)
            .ok_or(io::ErrorKind::NotFound)?;
        child.process.child.pidfd()?.kill()
    }
    fn results(&mut self) -> io::Result<()> {
        let mut bytes = [0; 256];
        loop {
            match self.worker.wake.read(&mut bytes) {
                Ok(0) => return Err(io::Error::other("launch worker disconnected")),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        for _ in 0..UNITS {
            let Ok(event) = self.worker.results.try_recv() else {
                break;
            };
            self.pending[event.start.unit] = false;
            match event.child {
                Err(_) => {
                    self.manager
                        .spawn_failed(event.start, self.tick())
                        .map_err(|_| io::Error::other("failed launch token"))?;
                    self.record(Event::SpawnFailed {
                        unit: event.start.unit,
                    });
                }
                Ok(mut process) => {
                    if let Err(_) = self.manager.spawned(event.start, event.key, self.tick()) {
                        spawn::cleanup(&mut process.child);
                        return Err(io::Error::other("stale launch publication"));
                    }
                    let definition = self
                        .manager
                        .catalog()
                        .unit(event.start.unit)
                        .ok_or_else(|| io::Error::other("unit"))?;
                    let frame = wire::Frame::message(Message::Init {
                        run: event.start.run,
                        notify: definition.readiness
                            == rustos_service_core::config::Readiness::Notify,
                        watchdog: definition.watchdog,
                    })?;
                    let input = process
                        .input
                        .take()
                        .ok_or_else(|| io::Error::other("missing child input"))?;
                    self.children[event.start.unit] = Some(Child {
                        process,
                        start: event.start,
                        key: event.key,
                        input: Some(input),
                        output: Some(frame),
                        stop: None,
                        notify: wire::Reader::new(),
                        log: wire::Reader::new(),
                        budgets: [
                            Budget::new(8, 1).expect("constant notify budget"),
                            Budget::new(4, 1).expect("constant log budget"),
                        ],
                        paused: [false; 2],
                        resume: [None; 2],
                    });
                    self.record(Event::Started {
                        unit: event.start.unit,
                        run: event.start.run,
                        process: event.key,
                        tick: self.tick(),
                    });
                }
            }
        }
        Ok(())
    }
    fn action(&mut self) -> io::Result<bool> {
        let Some(action) = self
            .manager
            .next_action(self.tick())
            .map_err(|_| io::Error::other("manager action invariant"))?
        else {
            return Ok(false);
        };
        match action {
            Action::Terminate { process, .. } => {
                let _ = self.terminate(process);
            }
            Action::NotifyStop { unit, process, run } => {
                if let Some(child) =
                    self.children
                        .get_mut(unit)
                        .and_then(Option::as_mut)
                        .filter(|child| {
                            child.key == process && child.start.run == run && child.input.is_some()
                        })
                {
                    child.stop = Some(wire::Frame::message(Message::Stop { run })?);
                } else {
                    self.manager
                        .stop_delivery_failed(unit, run, process)
                        .map_err(|_| io::Error::other("stop delivery identity"))?;
                }
            }
            Action::Start(start) => {
                let definition = self
                    .manager
                    .catalog()
                    .unit(start.unit)
                    .ok_or_else(|| io::Error::other("launch unit"))?;
                let key = ProcessKey {
                    pid: self.next_pid,
                    generation: 1,
                };
                self.next_pid = self
                    .next_pid
                    .checked_add(1)
                    .filter(|&p| p < u32::MAX)
                    .ok_or_else(|| io::Error::other("process IDs exhausted"))?;
                let request = spawn::Request {
                    start,
                    key,
                    path: definition
                        .executable
                        .clone()
                        .ok_or_else(|| io::Error::other("launch path"))?,
                    arguments: definition.arguments.clone(),
                    environment: definition.environment.clone(),
                };
                if self.pending[start.unit] {
                    return Err(io::Error::other("duplicate launch reservation"));
                }
                self.pending[start.unit] = true;
                self.queued[start.unit] = Some(request);
            }
        }
        Ok(true)
    }
    fn queue(&mut self) -> io::Result<bool> {
        use std::sync::mpsc::TrySendError;
        let mut progress = false;
        for unit in 0..UNITS {
            let Some(request) = self.queued[unit].take() else {
                continue;
            };
            if self.manager.launch_cancelled(request.start) {
                self.manager
                    .spawn_failed(request.start, self.tick())
                    .map_err(|_| io::Error::other("cancel queued launch"))?;
                self.pending[unit] = false;
                self.record(Event::SpawnFailed { unit });
                progress = true;
                continue;
            }
            match self
                .worker
                .sender
                .as_ref()
                .ok_or_else(|| io::Error::other("worker stopped"))?
                .try_send(request)
            {
                Ok(()) => progress = true,
                Err(TrySendError::Full(request)) => {
                    self.queued[unit] = Some(request);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    return Err(io::Error::other("worker disconnected"))
                }
            }
        }
        Ok(progress)
    }
    fn descriptor(&mut self, fd: i32, events: i16, role: Role) {
        self.fds.push(poll::Fd {
            fd,
            events,
            returned: 0,
        });
        self.roles.push(role);
    }
    fn descriptors(&mut self) -> io::Result<()> {
        self.fds.clear();
        self.roles.clear();
        self.descriptor(self.worker.wake.as_raw_fd(), poll::READ, Role::Wake);
        let now = self.tick();
        for unit in 0..UNITS {
            let Some(child) = self.children[unit].as_mut() else {
                continue;
            };
            for role in 0..2 {
                if child.resume[role].is_some_and(|d| now >= d) {
                    child.paused[role] = false;
                    child.resume[role] = None;
                }
            }
            let pidfd = child.process.child.pidfd()?.as_raw_fd();
            let notify = (!child.notify.closed && !child.paused[0])
                .then(|| child.process.notify.as_raw_fd());
            let log =
                (!child.log.closed && !child.paused[1]).then(|| child.process.log.as_raw_fd());
            let input = if child.output.is_some() || child.stop.is_some() {
                child.input.as_ref().map(AsRawFd::as_raw_fd)
            } else {
                None
            };
            self.descriptor(pidfd, poll::READ, Role::Exit(unit));
            if let Some(fd) = notify {
                self.descriptor(fd, poll::READ, Role::Notify(unit));
            }
            if let Some(fd) = log {
                self.descriptor(fd, poll::READ, Role::Log(unit));
            }
            if let Some(fd) = input {
                self.descriptor(fd, poll::WRITE, Role::Input(unit));
            }
        }
        Ok(())
    }
    fn complete(&mut self, unit: usize) -> io::Result<()> {
        let Some(child) = self.children[unit].as_mut() else {
            return Ok(());
        };
        let Some(status) = child.process.child.try_wait()? else {
            return Ok(());
        };
        let key = child.key;
        let cause = match (status.code(), status.signal()) {
            (Some(code), _) => Completion::Exit(code as u64),
            (_, Some(9)) => Completion::Killed,
            (_, Some(signal)) => Completion::Fault(signal as u16),
            _ => return Err(io::Error::other("unknown child status")),
        };
        self.children[unit] = None;
        self.manager.completed(key, cause, self.tick());
        self.record(Event::Completed {
            unit,
            process: key,
            cause,
            tick: self.tick(),
        });
        Ok(())
    }
    fn input(&mut self, unit: usize) {
        let Some(child) = self.children[unit].as_mut() else {
            return;
        };
        // An Init frame already partially written must finish before Stop. The
        // independent second slot retains bounded progress without overwriting it.
        let initialization = child.output.is_some();
        let output = if initialization {
            &mut child.output
        } else {
            &mut child.stop
        };
        let (Some(frame), Some(input)) = (output.as_mut(), &mut child.input) else {
            return;
        };
        match frame.write(input) {
            Ok(true) => *output = None,
            Ok(false) => {}
            Err(_) => {
                let stopping = child.stop.is_some();
                let (run, key) = (child.start.run, child.key);
                child.output = None;
                child.stop = None;
                child.input = None;
                if stopping {
                    let _ = self.manager.stop_delivery_failed(unit, run, key);
                }
            }
        }
    }
    fn read(&mut self, unit: usize, log: bool) -> io::Result<()> {
        for _ in 0..8 {
            let now = self.tick();
            let Some(child) = self.children[unit].as_mut() else {
                break;
            };
            let role = usize::from(log);
            if let Intake::Pause { until } = child.budgets[role].admit(now) {
                child.paused[role] = true;
                child.resume[role] = until;
                break;
            }
            let frame = if log {
                child.log.next(&mut child.process.log)
            } else {
                child.notify.next(&mut child.process.notify)
            };
            let frame = match frame {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(_) => {
                    if log {
                        child.log.closed = true;
                    } else {
                        child.notify.closed = true;
                    }
                    let _ = child.process.child.pidfd()?.kill();
                    self.record(Event::ProtocolError { unit });
                    break;
                }
            };
            let key = child.key;
            let token = child.start.run;
            match Message::decode(&frame.bytes[..frame.length]) {
                Ok(Message::Ready { run }) if !log && run == token => {
                    if self
                        .manager
                        .notify(unit, run, key, Notification::Ready, now)
                        .is_ok()
                    {
                        self.record(Event::Ready { unit });
                    }
                }
                Ok(Message::Heartbeat { run }) if !log && run == token => {
                    let _ = self
                        .manager
                        .notify(unit, run, key, Notification::Heartbeat, now);
                }
                Ok(Message::Log { run, level, text }) if log && run == token => {
                    self.journal
                        .push(unit, run, key, now, level, text)
                        .map_err(|_| io::Error::other("journal attribution"))?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// Sleep on real exit/stream/worker readiness until a predicate or deadline.
    /// Process status is collected only for a ready pidfd, never on a timer scan.
    pub fn drive_until(
        &mut self,
        limit: Duration,
        predicate: impl Fn(&Self) -> bool,
    ) -> io::Result<()> {
        let until = Instant::now()
            .checked_add(limit)
            .ok_or_else(|| io::Error::other("duration overflow"))?;
        loop {
            self.results()?;
            let issued = self.action()?;
            let queued = self.queue()?;
            if predicate(self) {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err(io::ErrorKind::TimedOut.into());
            }
            if issued || queued {
                continue;
            }
            self.descriptors()?;
            let next = self
                .manager
                .next_deadline()
                .into_iter()
                .chain(
                    self.children
                        .iter()
                        .flatten()
                        .flat_map(|c| c.resume)
                        .flatten(),
                )
                .min();
            let stop = if let Some(tick) = next {
                let millis = tick
                    .checked_mul(10)
                    .ok_or_else(|| io::Error::other("tick overflow"))?;
                self.epoch
                    .checked_add(Duration::from_millis(millis))
                    .ok_or_else(|| io::Error::other("deadline overflow"))?
                    .min(until)
            } else {
                until
            };
            let remaining = stop.saturating_duration_since(Instant::now());
            let timeout = if remaining.is_zero() {
                0
            } else {
                remaining
                    .as_millis()
                    .saturating_add(1)
                    .min(i32::MAX as u128) as i32
            };
            self.waits += 1;
            poll::wait(&mut self.fds, timeout)?;
            // Durable exits win over any queued Ready or log tail from the dead process.
            for index in 0..self.fds.len() {
                if self.fds[index].returned != 0 {
                    if let Role::Exit(unit) = self.roles[index] {
                        self.complete(unit)?;
                    }
                }
            }
            for index in 0..self.fds.len() {
                if self.fds[index].returned == 0 {
                    continue;
                }
                match self.roles[index] {
                    Role::Notify(unit) => self.read(unit, false)?,
                    Role::Log(unit) => self.read(unit, true)?,
                    Role::Input(unit) => self.input(unit),
                    _ => {}
                }
            }
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        for child in self.children.iter_mut().flatten() {
            spawn::cleanup(&mut child.process.child);
        }
        self.children = std::array::from_fn(|_| None);
        self.worker.shutdown();
    }
}
