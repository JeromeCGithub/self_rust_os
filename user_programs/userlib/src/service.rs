//! Explicit per-run service protocol; application work chooses when to become ready.
use crate::ipc::{self, Endpoint};
pub use abi::Level;
pub use rustos_service_abi as abi;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
    Full,
    Closed,
}
impl From<abi::Error> for Error {
    fn from(_: abi::Error) -> Self {
        Self::Invalid
    }
}
impl From<ipc::SendError> for Error {
    fn from(error: ipc::SendError) -> Self {
        match error {
            ipc::SendError::Full => Self::Full,
            ipc::SendError::Closed => Self::Closed,
            ipc::SendError::Invalid => Self::Invalid,
        }
    }
}
/// Owned notify/log grants and a nonblocking initialization state. The application
/// dispatches every process event; this client never steals another endpoint's data.
pub struct Client {
    notify: Endpoint,
    log: Option<Endpoint>,
    state: abi::ClientState,
}
impl Client {
    pub fn bootstrap() -> Result<Self, Error> {
        Ok(Self {
            notify: Endpoint::bootstrap(abi::NOTIFY_TAG).map_err(|_| Error::Unavailable)?,
            log: Endpoint::bootstrap(abi::LOG_TAG).ok(),
            state: abi::ClientState::new(),
        })
    }
    /// Return false for unrelated endpoints so the application's dispatcher retains
    /// responsibility. One authenticated Init establishes the immutable run context.
    pub fn on_event(&mut self, event: &ipc::Event) -> Result<bool, Error> {
        if Some(event.endpoint) != self.notify.raw() {
            return Ok(false);
        }
        match event.kind {
            ipc::Kind::Message => {
                let sender = event.sender.ok_or(Error::Invalid)?;
                let message =
                    abi::Message::decode(event.payload.get(..event.length).ok_or(Error::Invalid)?)?;
                self.state.accept_init(sender, message)?;
                Ok(true)
            }
            ipc::Kind::PeerClosed => Err(Error::Closed),
            ipc::Kind::Writable => Ok(true),
        }
    }
    /// Authenticated Init sender, for binding other grants to the same manager.
    pub fn manager(&self) -> Option<ipc::ProcessKey> {
        self.state.manager()
    }
    pub fn run(&self) -> Option<u64> {
        self.state.run()
    }
    pub fn watchdog(&self) -> Option<u64> {
        self.state.watchdog()
    }
    /// Call after application initialization completes. Send failure leaves Ready
    /// retryable; the application can request writable interest and retry later.
    pub fn ready(&mut self) -> Result<(), Error> {
        if let Some(frame) = self.state.ready_frame()? {
            send(&self.notify, frame)?;
        }
        self.state.ready_sent()?;
        Ok(())
    }
    /// Call from completed work, not an unrelated background keepalive thread.
    pub fn heartbeat(&self) -> Result<(), Error> {
        send(&self.notify, self.state.heartbeat_frame()?)
    }
    pub fn request_writable(&self) -> Result<(), Error> {
        self.notify
            .request_writable(true)
            .map_err(|_| Error::Invalid)
    }
    /// Emit one bounded structured log record. Full log queues are independent
    /// from notifications; callers may drop a record rather than blocking work.
    pub fn log(&self, level: Level, text: &str) -> Result<(), Error> {
        send(
            self.log.as_ref().ok_or(Error::Unavailable)?,
            self.state.log_frame(level, text)?,
        )
    }
}
fn send(endpoint: &Endpoint, message: abi::Message<'_>) -> Result<(), Error> {
    let mut bytes = [0; abi::MAX_FRAME];
    let length = message.encode(&mut bytes)?;
    endpoint.send(&bytes[..length]).map_err(Error::from)
}
