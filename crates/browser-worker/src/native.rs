//! Ring 3 endpoint adapter. The SDK Child owns kill/reap on every drop path.
use crate::{Endpoint, Error, Session};
use userlib::{process::Child, sys};
/// Owned SDK process connection, no global descriptor or cross-process pointer.
pub struct Native {
    child: Child,
}
impl Native {
    /// Take ownership of an already connected trusted launch. The protocol handshake
    /// validates framing; restriction policy belongs to the trusted executable.
    pub fn owned(child: Child) -> Self {
        Self { child }
    }
    /// Start the service with an empty environment; page content is sent only after
    /// the restricted startup handshake, through the private authenticated endpoint.
    pub fn start(cancel: impl Fn() -> bool) -> Result<Session<Self>, Error> {
        let child =
            Child::spawn_with_env("/BIN/JWORKER.SVC", &[], &[]).map_err(|_| Error::Unavailable)?;
        Session::connect(Self { child }, cancel)
    }
}
impl Endpoint for Native {
    fn send(&mut self, byte: u8) -> Result<bool, Error> {
        match self.child.write_byte(byte) {
            Ok(()) => Ok(true),
            Err(sys::MessageError::Full) => Ok(false),
            Err(_) => Err(Error::Unavailable),
        }
    }
    fn send_slice(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        match self.child.write_packet(bytes) {
            Ok(count) => Ok(count),
            Err(sys::MessageError::Full) => Ok(0),
            Err(_) => Err(Error::Unavailable),
        }
    }
    fn receive(&mut self) -> Result<Option<([u8; 3], usize)>, Error> {
        self.child.read().map_err(|_| Error::Unavailable)
    }
    fn ticks(&self) -> u64 {
        sys::ticks()
    }
    fn idle(&mut self) {
        let _ = sys::sleep_ticks(1);
    }
}
