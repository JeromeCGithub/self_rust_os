//! TLS 1.3 using Rustls and RustCrypto. No sockets, hardware or HTTP policy here.
#![no_std]
extern crate alloc;
use alloc::{sync::Arc, vec, vec::Vec};
use core::time::Duration;
use rustls::{
    client::UnbufferedClientConnection,
    pki_types::{CertificateDer, ServerName, UnixTime},
    time_provider::TimeProvider,
    unbuffered::ConnectionState,
    ClientConfig, RootCertStore,
};
/// TLS errors never permit a fallback to plaintext or unchecked certificates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Certificate,
    Entropy,
    Clock,
    Protocol,
    Truncated,
    Timeout,
    Io,
    TooLarge,
    Memory,
}
/// Blocking, deadline-bounded raw TCP transport supplied by the application SDK.
pub trait Io {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error>;
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Error>;
    fn check_deadline(&self) -> Result<(), Error>;
}
#[derive(Debug)]
struct Clock(u64);
impl TimeProvider for Clock {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(self.0)))
    }
}
/// A verified TLS client with bounded encrypted and plaintext buffers.
pub struct Session {
    conn: UnbufferedClientConnection,
    incoming: Vec<u8>,
    in_len: usize,
    outgoing: Vec<u8>,
    out_len: usize,
    plain: Vec<u8>,
    plain_at: usize,
    peer_closed: bool,
    failure: Option<Error>,
    fatal_alert_attempted: bool,
}
const RECORD: usize = 18437;
fn storage(size: usize) -> Result<Vec<u8>, Error> {
    let mut b = Vec::new();
    b.try_reserve_exact(size).map_err(|_| Error::Memory)?;
    b.resize(size, 0);
    Ok(b)
}
fn map(error: rustls::Error) -> Error {
    match error {
        rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented => {
            Error::Certificate
        }
        rustls::Error::FailedToGetRandomBytes => Error::Entropy,
        _ => Error::Protocol,
    }
}
enum Step {
    Progress,
    Ready,
    Data,
    Read,
    Closed,
    Written,
}
enum Write<'a> {
    None,
    Data(&'a [u8]),
    Close,
}
impl Session {
    /// Use Mozilla roots plus an explicitly supplied DER CA, with actual UTC time.
    #[inline(never)]
    pub fn new(host: &str, unix_seconds: u64, extra_ca: Option<&[u8]>) -> Result<Self, Error> {
        if !(1577836800..=253402300799).contains(&unix_seconds) {
            return Err(Error::Clock);
        }
        getrandom::getrandom(&mut [0; 32]).map_err(|_| Error::Entropy)?;
        let mut provider = rustls_rustcrypto::provider();
        provider.cipher_suites = vec![
            rustls_rustcrypto::TLS13_CHACHA20_POLY1305_SHA256,
            rustls_rustcrypto::TLS13_AES_128_GCM_SHA256,
            rustls_rustcrypto::TLS13_AES_256_GCM_SHA384,
        ];
        provider
            .kx_groups
            .retain(|group| group.name() == rustls::NamedGroup::X25519);
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(ca) = extra_ca {
            roots
                .add(CertificateDer::from(ca))
                .map_err(|_| Error::Certificate)?;
        }
        let mut config =
            ClientConfig::builder_with_details(Arc::new(provider), Arc::new(Clock(unix_seconds)))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(map)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.max_fragment_size = Some(4096);
        let name = ServerName::try_from(host)
            .map_err(|_| Error::Certificate)?
            .to_owned();
        let conn = UnbufferedClientConnection::new(Arc::new(config), name).map_err(map)?;
        Ok(Self {
            conn,
            incoming: storage(RECORD)?,
            in_len: 0,
            outgoing: storage(RECORD)?,
            out_len: 0,
            plain: Vec::new(),
            plain_at: 0,
            peer_closed: false,
            failure: None,
            fatal_alert_attempted: false,
        })
    }
    /// Complete the authenticated handshake before accepting application writes.
    pub fn handshake(&mut self, io: &mut impl Io) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        loop {
            match self.step(io, Write::None)? {
                Step::Ready => return Ok(()),
                Step::Read => self.receive(io)?,
                Step::Closed => return Err(Error::Truncated),
                _ => (),
            }
        }
    }
    /// Encrypt and transmit one bounded application write, handling partial TCP I/O.
    pub fn write(&mut self, io: &mut impl Io, data: &[u8]) -> Result<usize, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if data.len() > 16384 {
            return Err(Error::TooLarge);
        }
        loop {
            match self.step(io, Write::Data(data))? {
                Step::Written => return Ok(data.len()),
                Step::Read => self.receive(io)?,
                Step::Closed => return Err(Error::Truncated),
                _ => (),
            }
        }
    }
    /// Receive authenticated bytes; zero requires the peer's TLS close_notify.
    pub fn read(&mut self, io: &mut impl Io, out: &mut [u8]) -> Result<usize, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if out.is_empty() {
            return Ok(0);
        }
        io.check_deadline()?;
        loop {
            if self.plain_at < self.plain.len() {
                let n = out.len().min(self.plain.len() - self.plain_at);
                out[..n].copy_from_slice(&self.plain[self.plain_at..self.plain_at + n]);
                self.plain_at += n;
                if self.plain_at == self.plain.len() {
                    self.plain.clear();
                    self.plain_at = 0;
                }
                return Ok(n);
            }
            if self.peer_closed {
                return Ok(0);
            }
            match self.step(io, Write::None)? {
                Step::Read | Step::Ready => self.receive(io)?,
                Step::Closed => return Ok(0),
                _ => (),
            }
        }
    }
    /// Attempt a TLS close notification within the transport's existing deadline.
    pub fn close(&mut self, io: &mut impl Io) -> Result<(), Error> {
        if self.failure.is_some() && self.fatal_alert_attempted {
            return Ok(());
        }
        loop {
            match self.step(io, Write::Close)? {
                Step::Written | Step::Closed => return Ok(()),
                Step::Read => self.receive(io)?,
                _ => (),
            }
        }
    }
    fn receive(&mut self, io: &mut impl Io) -> Result<(), Error> {
        io.check_deadline()?;
        if self.in_len == self.incoming.len() {
            return Err(Error::TooLarge);
        }
        let n = io.read(&mut self.incoming[self.in_len..])?;
        if n == 0 {
            return Err(Error::Truncated);
        }
        if n > self.incoming.len() - self.in_len {
            return Err(Error::Protocol);
        }
        self.in_len += n;
        Ok(())
    }
    #[inline(never)]
    fn step(&mut self, io: &mut impl Io, write: Write<'_>) -> Result<Step, Error> {
        io.check_deadline()?;
        let status = self
            .conn
            .process_tls_records(&mut self.incoming[..self.in_len]);
        let mut discard = status.discard;
        let state = match status.state {
            Ok(state) => state,
            Err(error) => {
                let error = map(error);
                self.failure = Some(error);
                // Honor Rustls's consumed prefix even on error. Its handshake
                // deframer can still refer to the remaining caller-owned flight.
                if discard > self.in_len {
                    return Err(Error::Protocol);
                }
                self.incoming.copy_within(discard..self.in_len, 0);
                self.in_len -= discard;
                return Err(error);
            }
        };
        let action = match state {
            ConnectionState::EncodeTlsData(mut encode) => {
                let n = encode
                    .encode(&mut self.outgoing[self.out_len..])
                    .map_err(|_| Error::TooLarge)?;
                self.out_len += n;
                if self.failure.is_some() {
                    // This fatal connection can never carry application traffic
                    // again. Ship its encoded alert directly: asking Rustls for
                    // another state first would reparse the rejected peer flight.
                    // No subsequent drive is allowed, including repeated close.
                    self.fatal_alert_attempted = true;
                    io.write_all(&self.outgoing[..self.out_len])?;
                    self.out_len = 0;
                    Step::Written
                } else {
                    Step::Progress
                }
            }
            ConnectionState::TransmitTlsData(transmit) => {
                io.write_all(&self.outgoing[..self.out_len])?;
                self.out_len = 0;
                transmit.done();
                if self.failure.is_some() {
                    Step::Written
                } else {
                    Step::Progress
                }
            }
            ConnectionState::BlockedHandshake => {
                if self.failure.is_some() {
                    Step::Closed
                } else {
                    Step::Read
                }
            }
            ConnectionState::WriteTraffic(mut traffic) => match write {
                Write::None => Step::Ready,
                Write::Data(data) => {
                    let n = traffic
                        .encrypt(data, &mut self.outgoing)
                        .map_err(|_| Error::Protocol)?;
                    io.write_all(&self.outgoing[..n])?;
                    Step::Written
                }
                Write::Close => {
                    let n = traffic
                        .queue_close_notify(&mut self.outgoing)
                        .map_err(|_| Error::Protocol)?;
                    io.write_all(&self.outgoing[..n])?;
                    Step::Written
                }
            },
            ConnectionState::ReadTraffic(mut traffic) => {
                if let Some(record) = traffic.next_record() {
                    let record = record.map_err(map)?;
                    if self.plain.len() + record.payload.len() > 65536 {
                        return Err(Error::TooLarge);
                    }
                    self.plain
                        .try_reserve(record.payload.len())
                        .map_err(|_| Error::Memory)?;
                    self.plain.extend_from_slice(record.payload);
                    discard += record.discard;
                }
                Step::Data
            }
            ConnectionState::PeerClosed | ConnectionState::Closed => {
                self.peer_closed = true;
                Step::Closed
            }
            _ => return Err(Error::Protocol),
        };
        if discard > self.in_len {
            return Err(Error::Protocol);
        }
        self.incoming.copy_within(discard..self.in_len, 0);
        self.in_len -= discard;
        Ok(action)
    }
}
