//! One explicitly granted, bounded management connection. Slow clients retain
//! at most one pending reply; read interest stays paused until it can be queued.
use rustos_service_core::{
    control::Session,
    journal::{Budget, Intake, Journal},
    manager::Manager,
    transaction::Outcome,
    ProcessKey,
};
use userlib::{
    ipc::{self, Endpoint, Kind},
    service::abi::{
        self,
        control::{Access, Request},
        replies::{Body, Code, Reply},
    },
    sys,
};
struct Frame {
    bytes: [u8; abi::MAX_FRAME],
    length: usize,
    hello: bool,
}
pub struct Connection {
    endpoint: Option<Endpoint>,
    peer: ipc::ProcessKey,
    run: u64,
    access: Access,
    session: Session,
    pending: Option<Frame>,
    welcomed: bool,
    budget: Budget,
    paused: bool,
    resume: Option<u64>,
}
impl Connection {
    pub fn new(
        endpoint: Endpoint,
        peer: ipc::ProcessKey,
        run: u64,
        access: Access,
        budget: Budget,
    ) -> Self {
        Self {
            endpoint: Some(endpoint),
            peer,
            run,
            access,
            session: Session::new(
                ProcessKey {
                    pid: peer.pid,
                    generation: peer.generation,
                },
                run,
                access,
            )
            .expect("validated native run identity"),
            pending: None,
            welcomed: false,
            budget,
            paused: false,
            resume: None,
        }
    }
    pub fn raw(&self) -> Option<ipc::Handle> {
        self.endpoint.as_ref().and_then(Endpoint::raw)
    }
    pub fn deadline(&self) -> Option<u64> {
        self.resume
    }
    fn close(&mut self) {
        self.endpoint = None;
        self.pending = None;
        self.resume = None;
    }
    fn readable(&mut self, enabled: bool) {
        if self
            .endpoint
            .as_mut()
            .is_some_and(|e| e.set_readable(enabled).is_err())
        {
            self.close();
        }
    }
    fn queue(&mut self, reply: Reply<'_>) {
        let mut bytes = [0; abi::MAX_FRAME];
        match reply.encode(&mut bytes) {
            Ok(length) => {
                self.pending = Some(Frame {
                    bytes,
                    length,
                    hello: matches!(reply.body, Body::Hello { .. }),
                })
            }
            Err(_) => self.close(),
        }
        self.flush();
    }
    fn flush(&mut self) {
        let (Some(endpoint), Some(frame)) = (&self.endpoint, &self.pending) else {
            return;
        };
        match endpoint.send(&frame.bytes[..frame.length]) {
            Ok(()) => {
                self.welcomed |= frame.hello;
                self.pending = None;
                self.readable(!self.paused);
            }
            Err(ipc::SendError::Full) => {
                self.readable(false);
                if self
                    .endpoint
                    .as_ref()
                    .is_some_and(|e| e.request_writable(true).is_err())
                {
                    self.close();
                }
            }
            Err(_) => self.close(),
        }
    }
    /// Hello waits until the initial activation settles. A notify-ready client
    /// must initialize its service protocol first; it must not wait on Hello to
    /// become ready. Explicit queries report Busy if another job runs later.
    pub fn refresh(&mut self, manager: &Manager) {
        if self.endpoint.is_none() {
            return;
        }
        if self.resume.is_some_and(|d| sys::ticks() >= d) {
            self.resume = None;
            self.paused = false;
            if self.pending.is_none() {
                self.readable(true);
            }
        }
        if !self.welcomed && self.pending.is_none() && manager.outcome() != Outcome::Running {
            self.queue(Reply {
                id: 0,
                body: Body::Hello {
                    access: self.access,
                    run: self.run,
                },
            });
        }
    }
    pub fn event<const N: usize>(
        &mut self,
        event: &ipc::Event,
        manager: &mut Manager,
        journal: &Journal<N>,
    ) {
        match event.kind {
            Kind::PeerClosed => {
                self.close();
                return;
            }
            Kind::Writable => {
                self.flush();
                return;
            }
            Kind::Message => {}
        }
        if event.sender != Some(self.peer) || !self.welcomed || self.pending.is_some() {
            self.close();
            return;
        }
        let Some(request) = event
            .payload
            .get(..event.length)
            .and_then(|b| Request::decode(b).ok())
        else {
            self.close();
            return;
        };
        let reply = match self.budget.admit(sys::ticks()) {
            Intake::Accept => self.session.dispatch(
                ProcessKey {
                    pid: self.peer.pid,
                    generation: self.peer.generation,
                },
                self.run,
                request,
                manager,
                journal,
            ),
            Intake::Pause { until } => {
                self.paused = true;
                self.resume = until;
                Reply {
                    id: request.id,
                    body: Body::Error {
                        code: Code::Busy,
                        detail: 0,
                    },
                }
            }
        };
        self.queue(reply);
    }
}
