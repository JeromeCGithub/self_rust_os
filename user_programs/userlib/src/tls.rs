//! Verified TLS 1.3 streams over process-owned TCP sockets.
use crate::{net, sys, system};
use alloc::{boxed::Box, vec::Vec};
pub use rustos_tls::Error;

#[cfg(target_os = "none")]
fn random(output: &mut [u8]) -> Result<(), getrandom::Error> {
    system::random_bytes(output).map_err(|_| getrandom::Error::UNSUPPORTED)
}
#[cfg(target_os = "none")]
getrandom::register_custom_getrandom!(random);

struct Raw {
    handle: u64,
    start: u64,
    timeout: u64,
    packet: net::Packet,
    at: usize,
    len: usize,
}
impl rustos_tls::Io for Raw {
    fn check_deadline(&self) -> Result<(), Error> {
        if sys::ticks().wrapping_sub(self.start) >= self.timeout {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
    fn read(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        loop {
            self.check_deadline()?;
            if self.at < self.len {
                let n = out.len().min(self.len - self.at);
                out[..n].copy_from_slice(&self.packet.data[self.at..self.at + n]);
                self.at += n;
                return Ok(n);
            }
            match net::recv_into(self.handle, &mut self.packet) {
                Ok(n) => {
                    self.at = 0;
                    self.len = n;
                    if n == 0 {
                        return Ok(0);
                    }
                }
                Err(net::Error::WouldBlock) => {
                    sys::sleep_ticks(1).map_err(|_| Error::Io)?;
                }
                Err(_) => return Err(Error::Io),
            }
        }
    }
    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            self.check_deadline()?;
            let n = bytes.len().min(net::MAX_DATA);
            let mut packet = net::Packet::empty();
            packet.len = n as u16;
            packet.data[..n].copy_from_slice(&bytes[..n]);
            match net::send(self.handle, &packet) {
                Ok(n) if n > 0 && n <= bytes.len() => bytes = &bytes[n..],
                Ok(_) | Err(net::Error::WouldBlock) => {
                    sys::sleep_ticks(1).map_err(|_| Error::Io)?;
                }
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }
}
/// Owns a TCP socket after upgrade. Dropping always releases it, even on errors.
pub struct Connection {
    session: Box<rustos_tls::Session>,
    raw: Raw,
}
impl Connection {
    /// Upgrade a connected, owned socket. Failure leaves ownership with the caller.
    /// CA file adds trust explicitly; it never disables Mozilla root verification.
    #[inline(never)]
    pub fn upgrade(
        handle: u64,
        host: &str,
        timeout: u64,
        ca_file: Option<&str>,
    ) -> Result<Self, Error> {
        let now = system::unix_time().map_err(|_| Error::Clock)?;
        system::random_bytes(&mut [0; 32]).map_err(|_| Error::Entropy)?;
        let mut ca = Vec::new();
        if let Some(path) = ca_file {
            ca.try_reserve_exact(16384).map_err(|_| Error::Memory)?;
            ca.resize(16384, 0);
            let n = sys::read_file(path, &mut ca).ok_or(Error::Certificate)?;
            ca.truncate(n);
        }
        let session = rustos_tls::Session::new(host, now, ca_file.map(|_| ca.as_slice()))?;
        let mut connection = Self {
            session: Box::new(session),
            raw: Raw {
                handle,
                start: sys::ticks(),
                timeout,
                packet: net::Packet::empty(),
                at: 0,
                len: 0,
            },
        };
        // On failure, prevent Drop from closing the caller-owned socket twice.
        if let Err(error) = connection.session.handshake(&mut connection.raw) {
            let _ = connection.session.close(&mut connection.raw);
            connection.raw.handle = 0;
            return Err(error);
        }
        Ok(connection)
    }
    /// Write authenticated application bytes within the original connection deadline.
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        self.session.write(&mut self.raw, bytes)
    }
    /// Read authenticated bytes. Zero follows a TLS close notification.
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        self.session.read(&mut self.raw, output)
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if self.raw.handle != 0 {
            let _ = self.session.close(&mut self.raw);
            let _ = net::close(self.raw.handle);
        }
    }
}
/// Current private TLS heap use, useful for lifecycle diagnostics in applications.
#[cfg(target_os = "none")]
pub fn heap_used() -> usize {
    crate::heap::used()
}
