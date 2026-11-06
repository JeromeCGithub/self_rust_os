//! Streaming HTTP and HTTPS requests. URL parsing, DNS and HTTP run in Ring 3.
//! The callback receives decoded body bytes; sockets close on success or error.
//! HTTPS uses the optional TLS feature, verified certificates and platform time/RNG.
use crate::{net, sys};
pub use rustos_http::{Charset, Error, Options, Request, Response};

#[derive(Default)]
struct Sockets {
    #[cfg(feature = "tls")]
    tls: Option<(u64, crate::tls::Connection)>,
    failure: Option<Error>,
    #[cfg(feature = "alloc")]
    cancel: Option<crate::io::Cancellation>,
}
impl rustos_http::Transport for Sockets {
    fn cancelled(&self) -> bool {
        #[cfg(feature = "alloc")]
        {
            self.cancel
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
        }
        #[cfg(not(feature = "alloc"))]
        {
            false
        }
    }
    fn open(&mut self, kind: net::Kind) -> Result<u64, net::Error> {
        net::open(kind)
    }
    fn connect(&mut self, h: u64, ip: [u8; 4], port: u16) -> Result<(), net::Error> {
        net::connect(h, ip, port)
    }
    fn send(&mut self, h: u64, data: &[u8]) -> Result<usize, net::Error> {
        #[cfg(feature = "tls")]
        if let Some((owner, tls)) = self.tls.as_mut() {
            if *owner == h {
                return tls.write(data).map_err(|error| {
                    self.failure = Some(tls_error(error));
                    net::Error::Closed
                });
            }
        }
        if data.len() > net::MAX_DATA {
            return Err(net::Error::MessageTooLarge);
        }
        let mut packet = net::Packet::empty();
        packet.len = data.len() as u16;
        packet.data[..data.len()].copy_from_slice(data);
        net::send(h, &packet)
    }
    fn recv(&mut self, h: u64, packet: &mut net::Packet) -> Result<(), net::Error> {
        #[cfg(feature = "tls")]
        if let Some((owner, tls)) = self.tls.as_mut() {
            if *owner == h {
                *packet = net::Packet::empty();
                return tls
                    .read(&mut packet.data)
                    .map(|n| packet.len = n as u16)
                    .map_err(|error| {
                        self.failure = Some(tls_error(error));
                        net::Error::Closed
                    });
            }
        }
        net::recv_into(h, packet).map(|_| ())
    }
    fn poll(&mut self, h: u64) -> Result<u64, net::Error> {
        net::poll(h)
    }
    fn close(&mut self, h: u64) {
        #[cfg(feature = "tls")]
        if self.tls.as_ref().is_some_and(|(owner, _)| *owner == h) {
            self.tls.take();
            return;
        }
        let _ = net::close(h);
    }
    fn transport_error(&self) -> Option<Error> {
        self.failure
    }
    #[cfg(feature = "tls")]
    fn start_tls(
        &mut self,
        h: u64,
        host: &str,
        timeout: u64,
        ca: Option<&str>,
    ) -> Result<(), Error> {
        let connection = match self.cancel.clone() {
            Some(token) => crate::tls::Connection::upgrade_cancellable(h, host, timeout, ca, token),
            None => crate::tls::Connection::upgrade(h, host, timeout, ca),
        }
        .map_err(tls_error)?;
        self.tls = Some((h, connection));
        Ok(())
    }
    fn ticks(&self) -> u64 {
        sys::ticks()
    }
    fn sleep(&mut self) {
        let _ = sys::sleep_ticks(1);
    }
}
/// GET a URL with the QEMU DNS profile and default resource/deadline limits.
pub fn get(url: &str, body: impl FnMut(&[u8]) -> Result<(), Error>) -> Result<Response, Error> {
    get_with(url, Options::default(), body)
}
/// GET with explicit DNS, redirect, body-size and per-connection deadline limits.
pub fn get_with(
    url: &str,
    options: Options,
    body: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<Response, Error> {
    rustos_http::get(&mut Sockets::default(), url, options, body)
}
/// Human-readable curl/SDK error without exposing protocol internals to callers.
pub fn error_message(error: Error) -> &'static str {
    match error {
        Error::Cancelled => "request cancelled",
        Error::UnsupportedScheme => "unsupported URL scheme",
        Error::Tls => "TLS protocol error",
        Error::Certificate => "TLS certificate rejected",
        Error::Entropy => "secure randomness unavailable",
        Error::Clock => "UTC clock unavailable",
        Error::InsecureRedirect => "HTTPS downgrade refused",
        Error::CrossOriginRedirect => "POST redirect to another origin refused",
        Error::InvalidUrl => "invalid HTTP or HTTPS URL",
        Error::Dns => "DNS lookup failed",
        Error::Timeout => "request timed out",
        Error::Truncated => "response ended early",
        Error::TooLarge => "response or URL exceeds limits",
        Error::Protocol => "invalid or unsupported HTTP response",
        Error::RedirectLimit => "too many redirects",
        Error::Output => "output failed",
        Error::Socket(net::Error::NoDevice) => "network unavailable",
        Error::Socket(_) => "socket operation failed",
    }
}

#[cfg(feature = "tls")]
fn tls_error(error: crate::tls::Error) -> Error {
    match error {
        crate::tls::Error::Cancelled => Error::Cancelled,
        crate::tls::Error::Certificate => Error::Certificate,
        crate::tls::Error::Entropy => Error::Entropy,
        crate::tls::Error::Clock => Error::Clock,
        crate::tls::Error::Timeout => Error::Timeout,
        crate::tls::Error::Truncated => Error::Truncated,
        crate::tls::Error::TooLarge | crate::tls::Error::Memory => Error::TooLarge,
        _ => Error::Tls,
    }
}

/// GET with an observer for the final URL (including followed redirects).
/// The observer runs on success after the request socket has been closed.
pub fn get_with_url(
    url: &str,
    options: Options,
    body: impl FnMut(&[u8]) -> Result<(), Error>,
    effective: impl FnMut(&str),
) -> Result<Response, Error> {
    rustos_http::get_with_url(&mut Sockets::default(), url, options, body, effective)
}

/// GET with owned cancellation through DNS, TCP, TLS handshake and HTTP framing.
/// Cancellation closes the owned socket and never relaxes certificate validation.
#[cfg(feature = "alloc")]
pub fn get_cancellable(
    url: &str,
    options: Options,
    body: impl FnMut(&[u8]) -> Result<(), Error>,
    effective: impl FnMut(&str),
    cancel: crate::io::Cancellation,
) -> Result<Response, Error> {
    request_cancellable(url, Request::Get, options, body, effective, cancel)
}

/// GET or bounded form POST with the same certificate, cancellation and cleanup
/// policy as GET. Body data remains borrowed only for this synchronous call.
#[cfg(feature = "alloc")]
pub fn request_cancellable(
    url: &str,
    request: Request<'_>,
    options: Options,
    body: impl FnMut(&[u8]) -> Result<(), Error>,
    effective: impl FnMut(&str),
    cancel: crate::io::Cancellation,
) -> Result<Response, Error> {
    let mut sockets = Sockets {
        cancel: Some(cancel),
        ..Sockets::default()
    };
    rustos_http::request_with_url(&mut sockets, url, request, options, body, effective)
}

/// Borrowed GET/form POST for clients that do not need an allocated cancel token.
pub fn request_with_url(
    url: &str,
    request: Request<'_>,
    options: Options,
    body: impl FnMut(&[u8]) -> Result<(), Error>,
    effective: impl FnMut(&str),
) -> Result<Response, Error> {
    rustos_http::request_with_url(
        &mut Sockets::default(),
        url,
        request,
        options,
        body,
        effective,
    )
}
