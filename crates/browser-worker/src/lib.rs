//! Bounded process protocol. Platform transports own child cancellation/reaping.
#![no_std]
extern crate alloc;
use alloc::{string::String, vec::Vec};
/// Fixed wire header, no native pointers or platform layout assumptions.
pub const HEADER: usize = 24;
/// Maximum classic script bytes per request.
pub const MAX_SOURCE: usize = 128 * 1024;
/// Maximum copied result bytes.
pub const MAX_OUTPUT: usize = 8192;
/// Total monotonic ticks allowed per bootstrap/request, including backpressure.
pub const DEADLINE_TICKS: u64 = 180;
/// Protocol/resource failures omit script contents and foreign exception text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Protocol,
    Unavailable,
    Deadline,
    Cancelled,
    Memory,
    Engine(u8),
    Input,
}
/// The worker only speaks language operations, never native handles or addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Ready = 0,
    Eval = 1,
    Result = 2,
    Failure = 3,
    Document = 4,
    /// JPQ1 page context; reply is JPR1, both with the existing byte budgets.
    Page = 5,
}
/// Explicit-width, little-endian wire fields, validated before allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub generation: u64,
    pub length: usize,
    pub status: u8,
}
impl Header {
    fn valid(&self) -> bool {
        match self.kind {
            Kind::Ready => self.generation == 0 && self.length == 0 && self.status == 0,
            Kind::Eval | Kind::Document | Kind::Page => {
                self.generation != 0 && self.length <= MAX_SOURCE && self.status == 0
            }
            Kind::Result => self.generation != 0 && self.length <= MAX_OUTPUT && self.status == 0,
            Kind::Failure => {
                self.generation != 0 && self.length == 0 && (1..=6).contains(&self.status)
            }
        }
    }
    /// Encode after checking all operation-specific bounds.
    pub fn bytes(self) -> Result<[u8; HEADER], Error> {
        if !self.valid() {
            return Err(Error::Protocol);
        }
        let mut bytes = [0; HEADER];
        bytes[..4].copy_from_slice(b"JBW1");
        bytes[4] = self.kind as u8;
        bytes[5] = self.status;
        bytes[8..16].copy_from_slice(&self.generation.to_le_bytes());
        bytes[16..20].copy_from_slice(&(self.length as u32).to_le_bytes());
        Ok(bytes)
    }
    /// Decode owned bytes. Reserved fields, unknown versions/operations fail closed.
    pub fn parse(bytes: [u8; HEADER]) -> Result<Self, Error> {
        if &bytes[..4] != b"JBW1" || bytes[6..8] != [0; 2] || bytes[20..24] != [0; 4] {
            return Err(Error::Protocol);
        }
        let kind = match bytes[4] {
            0 => Kind::Ready,
            1 => Kind::Eval,
            2 => Kind::Result,
            3 => Kind::Failure,
            4 => Kind::Document,
            5 => Kind::Page,
            _ => return Err(Error::Protocol),
        };
        let header = Self {
            kind,
            status: bytes[5],
            generation: u64::from_le_bytes(bytes[8..16].try_into().map_err(|_| Error::Protocol)?),
            length: u32::from_le_bytes(bytes[16..20].try_into().map_err(|_| Error::Protocol)?)
                as usize,
        };
        if header.valid() {
            Ok(header)
        } else {
            Err(Error::Protocol)
        }
    }
}
/// Endpoint methods are nonblocking. Dropping it kills/reaps its owned worker.
pub trait Endpoint {
    fn send(&mut self, byte: u8) -> Result<bool, Error>;
    /// Nonblocking prefix transfer. Zero is backpressure, not EOF. The default
    /// preserves byte transports; native endpoints use copied three-byte packets.
    fn send_slice(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        match bytes.first() {
            Some(byte) => self.send(*byte).map(usize::from),
            None => Ok(0),
        }
    }
    fn receive(&mut self) -> Result<Option<([u8; 3], usize)>, Error>;
    fn ticks(&self) -> u64;
    fn idle(&mut self);
}
/// Single outstanding request. Irrecoverable protocol/transport failure closes it.
pub struct Session<E: Endpoint> {
    endpoint: Option<E>,
    generation: u64,
}
impl<E: Endpoint> Session<E> {
    /// Validate a restricted worker's startup record under cancellation/deadline.
    pub fn connect(endpoint: E, cancel: impl Fn() -> bool) -> Result<Self, Error> {
        let mut session = Self {
            endpoint: Some(endpoint),
            generation: 0,
        };
        let start = session.endpoint.as_ref().ok_or(Error::Unavailable)?.ticks();
        let ready = session.header(start, &cancel)?;
        if ready.kind != Kind::Ready {
            return Err(Error::Protocol);
        }
        Ok(session)
    }
    fn check(&self, start: u64, cancel: &impl Fn() -> bool) -> Result<(), Error> {
        if cancel() {
            return Err(Error::Cancelled);
        }
        if self
            .endpoint
            .as_ref()
            .ok_or(Error::Unavailable)?
            .ticks()
            .wrapping_sub(start)
            >= DEADLINE_TICKS
        {
            return Err(Error::Deadline);
        }
        Ok(())
    }
    fn read(
        &mut self,
        bytes: &mut [u8],
        start: u64,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), Error> {
        let mut used = 0;
        while used < bytes.len() {
            self.check(start, cancel)?;
            let endpoint = self.endpoint.as_mut().ok_or(Error::Unavailable)?;
            match endpoint.receive()? {
                Some((part, n)) if n > 0 && n <= part.len() && n <= bytes.len() - used => {
                    bytes[used..used + n].copy_from_slice(&part[..n]);
                    used += n;
                }
                Some(_) => return Err(Error::Protocol),
                None => endpoint.idle(),
            }
        }
        Ok(())
    }
    fn header(&mut self, start: u64, cancel: &impl Fn() -> bool) -> Result<Header, Error> {
        let mut bytes = [0; HEADER];
        self.read(&mut bytes, start, cancel)?;
        Header::parse(bytes)
    }
    /// Execute a document in its own native realm and return bounded DOM effects.
    pub fn render_document(
        &mut self,
        html: &str,
        cancel: impl Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        self.request(Kind::Document, html.as_bytes(), &cancel)
    }
    /// Execute an encoded JPQ1 request. Context decoding belongs to the realm;
    /// the transport validates generations, ownership and complete copied bytes.
    pub fn render_page(
        &mut self,
        context: &[u8],
        cancel: impl Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        self.request(Kind::Page, context, &cancel)
    }
    /// Evaluate one bounded script; recoverable engine errors preserve the realm.
    /// Cancellation, EOF, malformed replies and deadlines kill/reap the endpoint.
    pub fn eval(&mut self, source: &str, cancel: impl Fn() -> bool) -> Result<String, Error> {
        let bytes = self.request(Kind::Eval, source.as_bytes(), &cancel)?;
        match String::from_utf8(bytes) {
            Ok(text) => Ok(text),
            Err(_) => {
                self.endpoint.take();
                Err(Error::Protocol)
            }
        }
    }
    fn request(
        &mut self,
        kind: Kind,
        source: &[u8],
        cancel: &impl Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        if source.len() > MAX_SOURCE || (kind == Kind::Eval && source.contains(&0)) {
            return Err(Error::Input);
        }
        let result = self.evaluate(kind, source, cancel);
        if matches!(result,Err(error) if !matches!(error,Error::Engine(_)|Error::Input)) {
            self.endpoint.take();
        }
        result
    }
    fn evaluate(
        &mut self,
        kind: Kind,
        source: &[u8],
        cancel: &impl Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        self.generation = self.generation.checked_add(1).ok_or(Error::Unavailable)?;
        let generation = self.generation;
        let bytes = Header {
            kind,
            generation,
            length: source.len(),
            status: 0,
        }
        .bytes()?;
        let start = self.endpoint.as_ref().ok_or(Error::Unavailable)?.ticks();
        // Keep header/body packet boundaries separate so receivers never need
        // to retain bytes belonging to a different record or untrusted length.
        for part in [bytes.as_slice(), source] {
            let mut used = 0;
            while used < part.len() {
                self.check(start, cancel)?;
                let endpoint = self.endpoint.as_mut().ok_or(Error::Unavailable)?;
                let count = endpoint.send_slice(&part[used..])?;
                if count > part.len() - used {
                    return Err(Error::Protocol);
                }
                if count > 0 {
                    used += count;
                } else {
                    endpoint.idle();
                }
            }
        }
        let reply = self.header(start, cancel)?;
        if reply.generation != generation {
            return Err(Error::Protocol);
        }
        match reply.kind {
            Kind::Failure => Err(Error::Engine(reply.status)),
            Kind::Result => {
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(reply.length)
                    .map_err(|_| Error::Memory)?;
                bytes.resize(reply.length, 0);
                self.read(&mut bytes, start, cancel)?;
                Ok(bytes)
            }
            _ => Err(Error::Protocol),
        }
    }
}
#[cfg(target_os = "none")]
pub mod native;
