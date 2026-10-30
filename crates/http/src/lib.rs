//! Userspace HTTP protocol and socket orchestration, independent of the OS adapter.
#![no_std]
use rustos_net_abi::{Error as SocketError, Kind, Packet};
/// Request failure; a body callback may already have received partial output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidUrl,
    UnsupportedScheme,
    Protocol,
    Tls,
    Certificate,
    Entropy,
    Clock,
    InsecureRedirect,
    Truncated,
    TooLarge,
    Dns,
    Timeout,
    RedirectLimit,
    Output,
    Socket(SocketError),
}
/// Process socket adapter. Calls are nonblocking; sleep yields until a later tick.
/// Receive writes into reusable initialized storage, avoiding full-record copies.
pub trait Transport {
    fn open(&mut self, kind: Kind) -> Result<u64, SocketError>;
    fn connect(&mut self, handle: u64, address: [u8; 4], port: u16) -> Result<(), SocketError>;
    fn send(&mut self, handle: u64, data: &[u8]) -> Result<usize, SocketError>;
    fn recv(&mut self, handle: u64, packet: &mut Packet) -> Result<(), SocketError>;
    fn poll(&mut self, handle: u64) -> Result<u64, SocketError>;
    fn close(&mut self, handle: u64);
    fn ticks(&self) -> u64;
    fn sleep(&mut self);
    /// Upgrade an established TCP socket before sending HTTP plaintext.
    fn start_tls(
        &mut self,
        _handle: u64,
        _host: &str,
        _timeout: u64,
        _ca_file: Option<&str>,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedScheme)
    }
    /// Preserve a TLS record failure through the socket-oriented transport API.
    fn transport_error(&self) -> Option<Error> {
        None
    }
}
#[derive(Debug, Clone, Copy)]
/// Resolver, deadline and resource policy for a GET request.
pub struct Options {
    /// Recursive IPv4 DNS server (QEMU defaults to 10.0.2.3).
    pub dns_server: [u8; 4],
    /// DNS server UDP port.
    pub dns_port: u16,
    /// Total PIT ticks per TCP connection; DNS has an additional 54-tick cap.
    pub timeout_ticks: u64,
    /// Maximum number of followed redirects.
    pub redirects: u8,
    /// Maximum decoded body bytes sent to the callback.
    pub max_body: usize,
    /// Optional additional DER CA file; Mozilla public roots remain enabled.
    pub tls_ca_file: Option<&'static str>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            dns_server: [10, 0, 2, 3],
            dns_port: 53,
            timeout_ticks: 180,
            redirects: 5,
            max_body: limits::BODY,
            tls_ca_file: None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Final response metadata after a successfully decoded body.
pub struct Response {
    /// HTTP status, including error statuses such as 404.
    pub status: u16,
    /// Number of decoded bytes delivered to the callback.
    pub bytes: usize,
}

mod dns;
pub mod limits;
mod url;
mod wire;

/// Perform a streaming HTTP GET. DNS, TCP and response framing are userspace.
/// The callback receives decoded body bytes; an error closes the active socket.
/// HTTP error statuses are successful responses. Redirect bodies are discarded.
#[inline(never)]
pub fn get(
    t: &mut impl Transport,
    url: &str,
    options: Options,
    mut body: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<Response, Error> {
    if options.timeout_ticks == 0 || options.dns_port == 0 {
        return Err(Error::InvalidUrl);
    }
    let mut url = url::Url::parse(url)?;
    for redirect in 0..=options.redirects {
        let ip = dns::resolve(t, url.host.as_str(), options)?;
        let handle = t.open(Kind::Tcp).map_err(Error::Socket)?;
        let mut location = url::Text::<{ limits::TARGET }>::new();
        let outcome = exchange(t, handle, ip, &url, options, &mut location, &mut body)
            .map_err(|error| t.transport_error().unwrap_or(error));
        t.close(handle);
        if let Some(response) = outcome? {
            return Ok(response);
        }
        if redirect == options.redirects {
            return Err(Error::RedirectLimit);
        }
        let next = url.redirect(location.as_str())?;
        if url.tls && !next.tls {
            return Err(Error::InsecureRedirect);
        }
        url = next;
    }
    Err(Error::RedirectLimit)
}

// Keep connection, request construction, DNS and framing in distinct stack
// frames. Separate phases bound peak stack use together with application callers.
#[inline(never)]
fn exchange(
    t: &mut impl Transport,
    handle: u64,
    ip: [u8; 4],
    url: &url::Url,
    options: Options,
    location: &mut url::Text<{ limits::TARGET }>,
    body: &mut impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<Option<Response>, Error> {
    use rustos_net_abi::{CONNECTED, EOF};
    let start = t.ticks();
    t.connect(handle, ip, url.port).map_err(Error::Socket)?;
    loop {
        if t.ticks().wrapping_sub(start) >= options.timeout_ticks {
            return Err(Error::Timeout);
        }
        let flags = t.poll(handle).map_err(Error::Socket)?;
        if flags & CONNECTED != 0 {
            break;
        }
        if flags & EOF != 0 {
            return Err(Error::Socket(SocketError::Closed));
        }
        t.sleep();
    }
    if url.tls {
        let remaining = options
            .timeout_ticks
            .saturating_sub(t.ticks().wrapping_sub(start));
        if remaining == 0 {
            return Err(Error::Timeout);
        }
        t.start_tls(handle, url.host.as_str(), remaining, options.tls_ca_file)?;
    }
    send_request(t, handle, url, start, options)?;
    let mut reader = wire::Reader::new(t, handle, start, options);
    let head = wire::headers(&mut reader, location)?;
    if [301, 302, 303, 307, 308].contains(&head.status) && !location.as_str().is_empty() {
        return Ok(None);
    }
    let bytes = wire::body(&mut reader, &head, body)?;
    Ok(Some(Response {
        status: head.status,
        bytes,
    }))
}

#[inline(never)]
fn send_request(
    t: &mut impl Transport,
    handle: u64,
    url: &url::Url,
    start: u64,
    options: Options,
) -> Result<(), Error> {
    let mut request = url::Text::<{ limits::REQUEST }>::new();
    request.push("GET ")?;
    request.push(url.target.as_str())?;
    request.push(" HTTP/1.1\r\nHost: ")?;
    url.authority(&mut request)?;
    request.push("\r\nUser-Agent: RustOS-curl/1\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n")?;
    let bytes = request.as_str().as_bytes();
    let mut sent = 0;
    while sent < bytes.len() {
        if t.ticks().wrapping_sub(start) >= options.timeout_ticks {
            return Err(Error::Timeout);
        }
        let chunk = &bytes[sent..bytes.len().min(sent + 1024)];
        match t.send(handle, chunk) {
            Ok(n) if n > 0 && n <= chunk.len() => sent += n,
            Ok(0) | Err(SocketError::WouldBlock) => t.sleep(),
            Err(e) => return Err(Error::Socket(e)),
            _ => return Err(Error::Protocol),
        }
    }
    Ok(())
}
