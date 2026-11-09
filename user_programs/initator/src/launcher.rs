//! One bounded native launch worker. Only the actor owns manager state and drains
//! process-wide events. No lock spans filesystem I/O, spawn, or event delivery.
use crate::ports::Run;
use alloc::{string::String, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};
use rustos_service_core::{
    config::{Kind, Readiness, Unit},
    Start,
};
use userlib::{
    fs,
    ipc::{self, Endpoint},
    service::abi::control::Access,
    supervision,
    sync::{Condvar, Mutex},
    sys, thread,
};
pub struct Spec {
    pub(crate) executable: String,
    pub(crate) arguments: Vec<String>,
    pub(crate) environment: Vec<String>,
    pub(crate) readiness: Readiness,
    pub(crate) watchdog: Option<u64>,
    pub(crate) control: Option<Access>,
    cancel: Option<Arc<AtomicBool>>,
}
fn text(value: &str) -> Result<String, ()> {
    let mut copy = String::new();
    copy.try_reserve_exact(value.len()).map_err(|_| ())?;
    copy.push_str(value);
    Ok(copy)
}
fn strings(values: &[String]) -> Result<Vec<String>, ()> {
    let mut copies = Vec::new();
    copies.try_reserve_exact(values.len()).map_err(|_| ())?;
    for value in values {
        copies.push(text(value)?);
    }
    Ok(copies)
}
impl Spec {
    pub fn new(unit: &Unit) -> Result<Self, ()> {
        if unit.kind != Kind::Service
            || unit.arguments.len() > 32
            || unit.environment.len() > 32
            || unit.watchdog.is_some_and(|n| n == 0 || n > i64::MAX as u64)
            || unit.watchdog.is_some() && unit.readiness != Readiness::Notify
        {
            return Err(());
        }
        Ok(Self {
            executable: text(unit.executable.as_deref().ok_or(())?)?,
            arguments: strings(&unit.arguments)?,
            environment: strings(&unit.environment)?,
            readiness: unit.readiness,
            watchdog: unit.watchdog,
            control: unit.control,
            cancel: None,
        })
    }
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Acquire))
    }
    pub(crate) fn image(&self) -> Result<Vec<u8>, ()> {
        if self.cancelled() {
            return Err(());
        }
        let mut file = fs::File::open(&self.executable, fs::O_RDONLY)?;
        let length = usize::try_from(file.seek(0, fs::SEEK_END)?).map_err(|_| ())?;
        if length == 0 {
            return Err(());
        }
        file.seek(0, fs::SEEK_SET)?;
        if self.cancelled() {
            return Err(());
        }
        let mut image = Vec::new();
        image.try_reserve_exact(length).map_err(|_| ())?;
        image.resize(length, 0);
        let mut offset = 0;
        while offset < length {
            if self.cancelled() {
                return Err(());
            }
            let end = offset.saturating_add(16 * 1024).min(length);
            let count = file.read(&mut image[offset..end])?;
            if count == 0 || count > end - offset {
                return Err(());
            }
            offset += count;
            // Kernel-mode PIO is not preemptible on one CPU. Returning to Ring 3
            // between bounded reads gives the actor a scheduling opportunity there too.
            thread::yield_now();
        }
        let mut trailing = [0; 1];
        if file.read(&mut trailing)? != 0 {
            return Err(());
        }
        Ok(image)
    }
}
struct State {
    busy: bool,
    current: Option<Start>,
    stopping: bool,
    request: Option<(Start, Spec)>,
    result: Option<(Start, Result<Run, ()>)>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
pub struct Launcher {
    shared: Arc<Shared>,
    receive: Endpoint,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}
impl Launcher {
    pub fn new() -> Result<Self, ()> {
        let [receive, signal] = Endpoint::pair()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                busy: false,
                current: None,
                stopping: false,
                request: None,
                result: None,
            }),
            changed: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .spawn(move || loop {
                let request = {
                    let mut state = worker_shared.state.lock();
                    while state.request.is_none() && !state.stopping {
                        state = worker_shared.changed.wait(state);
                    }
                    if state.stopping {
                        break;
                    }
                    state.request.take().expect("launch request predicate")
                };
                let result = Run::launch(&request.1, request.0);
                let stopping = {
                    let mut state = worker_shared.state.lock();
                    assert!(state.result.is_none(), "one outstanding launch result");
                    state.result = Some((request.0, result));
                    state.stopping
                };
                if stopping {
                    break;
                }
                match signal.send(&[1]) {
                    Ok(()) | Err(ipc::SendError::Full) => {}
                    Err(_) => panic!("private launch wake endpoint lost"),
                }
            })
            .map_err(|_| ())?;
        Ok(Self {
            shared,
            receive,
            worker: Some(worker),
            cancel: Arc::new(AtomicBool::new(false)),
        })
    }
    /// Recheck this after draining wakes: notification consumption is separate
    /// from result consumption. Publication always precedes the kernel wake.
    pub fn has_result(&self) -> bool {
        self.shared.state.lock().result.is_some()
    }
    /// Identity remains outstanding until its result is taken, even after posting.
    pub fn current(&self) -> Option<Start> {
        self.shared.state.lock().current
    }
    /// Stop chunked loading without confusing a later reservation with this run.
    /// A child already committed by spawn is still returned for normal supervision.
    pub fn cancel(&self, start: Start) -> bool {
        let state = self.shared.state.lock();
        if state.current != Some(start) {
            return false;
        }
        self.cancel.store(true, Ordering::Release);
        true
    }
    pub fn busy(&self) -> bool {
        self.shared.state.lock().busy
    }
    pub fn submit(&mut self, start: Start, mut spec: Spec) -> Result<(), ()> {
        let mut state = self.shared.state.lock();
        if state.busy {
            return Err(());
        }
        self.cancel.store(false, Ordering::Release);
        state.current = Some(start);
        spec.cancel = Some(self.cancel.clone());
        state.busy = true;
        state.request = Some((start, spec));
        drop(state);
        self.shared.changed.notify_one();
        Ok(())
    }
    pub fn take(&mut self) -> Option<(Start, Result<Run, ()>)> {
        let mut state = self.shared.state.lock();
        let result = state.result.take()?;
        state.busy = false;
        state.current = None;
        Some(result)
    }
    pub fn wake_handle(&self) -> ipc::Handle {
        self.receive.raw().expect("owned private wake endpoint")
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        // One independent cancellation flag is shared with chunked reads. All
        // request/result metadata remains protected by the associated mutex.
        self.cancel.store(true, Ordering::Release);
        self.shared.state.lock().stopping = true;
        self.shared.changed.notify_one();
        if let Some(worker) = self.worker.take() {
            worker.join().expect("owned launch worker join");
        }
        let result = self.shared.state.lock().result.take();
        if let Some((_, Ok(run))) = result {
            let key = run.key();
            let _ = supervision::terminate(ipc::ProcessKey {
                pid: key.pid,
                generation: key.generation,
            });
            // Native PIDs are monotonic and unreused; this explicit owned-child
            // wait consumes only our unpublished child, including an old zombie.
            let _ = sys::waitpid(key.pid);
        }
        // Receiver Drop follows worker join (and sender Drop), freeing the
        // complete private channel instead of leaving a detached waiter/EOF.
    }
}
