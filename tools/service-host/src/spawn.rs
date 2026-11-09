//! One bounded worker constructs real processes; publication wakes the reactor.
use rustos_service_core::{ProcessKey, Start};
use std::{
    io::{self, Write},
    os::{
        fd::OwnedFd,
        linux::process::{ChildExt, CommandExt},
        unix::net::UnixStream,
    },
    process::{Child, Command, Stdio},
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
};
pub struct Request {
    pub start: Start,
    pub key: ProcessKey,
    pub path: String,
    pub arguments: Vec<String>,
    pub environment: Vec<String>,
}
pub struct Spawned {
    pub child: Child,
    pub input: Option<UnixStream>,
    pub notify: UnixStream,
    pub log: UnixStream,
}
pub struct ResultEvent {
    pub start: Start,
    pub key: ProcessKey,
    pub child: io::Result<Spawned>,
}
pub struct Worker {
    pub sender: Option<SyncSender<Request>>,
    pub results: Receiver<ResultEvent>,
    pub wake: UnixStream,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn new(hook: Arc<dyn Fn(Start) + Send + Sync>) -> io::Result<Self> {
        let (requests, receive) = mpsc::sync_channel::<Request>(4);
        let (results, output) = mpsc::sync_channel(64);
        let (wake, mut notify) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        notify.set_nonblocking(true)?;
        let worker = thread::Builder::new()
            .name("initator-launch".into())
            .spawn(move || {
                while let Ok(request) = receive.recv() {
                    hook(request.start);
                    let event = ResultEvent {
                        start: request.start,
                        key: request.key,
                        child: launch(request),
                    };
                    if let Err(error) = results.send(event) {
                        if let Ok(mut spawned) = error.0.child {
                            cleanup(&mut spawned.child);
                        }
                        break;
                    }
                    // Publish before wake. A full wake stream already contains a readable byte;
                    // the reactor drains wake bytes before checking all published results.
                    match notify.write(&[1]) {
                        Ok(_) => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            sender: Some(requests),
            results: output,
            wake,
            thread: Some(worker),
        })
    }
    pub fn shutdown(&mut self) {
        self.sender = None;
        while let Ok(event) = self.results.recv() {
            if let Ok(mut spawned) = event.child {
                cleanup(&mut spawned.child);
            }
        }
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}
pub fn cleanup(child: &mut Child) {
    // The Child has not been reaped elsewhere. Prefer its stable pidfd; fallback
    // Child::kill is used only before publication when pidfd setup itself failed.
    if let Ok(fd) = child.pidfd() {
        let _ = fd.kill();
    } else {
        let _ = child.kill();
    }
    let _ = child.wait();
}
fn launch(request: Request) -> io::Result<Spawned> {
    let (input, child_input) = UnixStream::pair()?;
    let (notify, child_notify) = UnixStream::pair()?;
    let (log, child_log) = UnixStream::pair()?;
    input.set_nonblocking(true)?;
    notify.set_nonblocking(true)?;
    log.set_nonblocking(true)?;
    let mut command = Command::new(&request.path);
    command
        .create_pidfd(true)
        .env_clear()
        .current_dir("/")
        .args(&request.arguments)
        .stdin(Stdio::from(OwnedFd::from(child_input)))
        .stdout(Stdio::from(OwnedFd::from(child_notify)))
        .stderr(Stdio::from(OwnedFd::from(child_log)));
    for value in &request.environment {
        let (key, value) = value
            .split_once('=')
            .ok_or_else(|| io::Error::other("invalid environment"))?;
        command.env(key, value);
    }
    let mut child = command.spawn()?;
    if let Err(error) = child.pidfd() {
        cleanup(&mut child);
        return Err(error);
    }
    Ok(Spawned {
        child,
        input: Some(input),
        notify,
        log,
    })
}

impl Drop for Spawned {
    fn drop(&mut self) {
        cleanup(&mut self.child);
    }
}
