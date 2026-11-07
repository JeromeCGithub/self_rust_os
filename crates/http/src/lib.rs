//! Userspace HTTP protocol and socket orchestration, independent of the OS adapter.
#![no_std]
use rustos_net_abi::{Error as SocketError, Kind, Packet};
/// Request failure; a body callback may already have received partial output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    InvalidUrl,
    UnsupportedScheme,
    Protocol,
    Tls,
    Certificate,
    Entropy,
    Clock,
    InsecureRedirect,
    /// A method-preserving redirect would forward a POST body to another origin.
    CrossOriginRedirect,
    RangeIgnored,
    RangeUnsatisfiable,
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
    /// Cooperative cancellation, checked through DNS/connect/framing progress.
    fn cancelled(&self) -> bool {
        false
    }
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
/// Application-owned request cookie policy, consulted with the complete effective
/// URL on every redirect hop. Returned values are copied before the next call.
/// This interface does not expose response Set-Cookie or implement cookie policy.
pub trait RequestCookies {
    fn header(&mut self, url: &str) -> Result<&str, Error>;
}
impl RequestCookies for () {
    fn header(&mut self, _url: &str) -> Result<&str, Error> {
        Ok("")
    }
}
#[derive(Debug, Clone, Copy)]
/// Resolver, deadline and resource policy for an HTTP request.
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
pub enum Charset {
    Utf8,
    Windows1252,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Final response metadata after a successfully decoded body.
pub struct Response {
    /// HTTP status, including error statuses such as 404.
    pub status: u16,
    pub charset: Option<Charset>,
    /// Number of decoded bytes delivered to the callback.
    pub bytes: usize,
    /// Parsed single byte Content-Range. GetRange additionally validates it
    /// against requested coordinates and the decoded response body length.
    pub range: Option<ContentRange>,
}

/// Closed inclusive byte range. Validated against body budgets before I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub first: u64,
    pub last: u64,
}
/// Single byte Content-Range with a known representation length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentRange {
    Satisfied { first: u64, last: u64, total: u64 },
    Unsatisfied { total: u64 },
}

/// Bounded request body owned by the caller for the duration of the exchange.
/// Form POST fixes the content type; arbitrary header injection is not exposed.
#[derive(Clone, Copy)]
pub enum Request<'a> {
    Get,
    GetRange(ByteRange),
    FormPost(&'a [u8]),
}
enum Outcome {
    Response(Response),
    Redirect(u16),
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
    body: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<Response, Error> {
    get_with_url(t, url, options, body, |_| {})
}

/// GET and observe the final response URL once, after success and socket close.
/// This lets document clients resolve relative resources after HTTP redirects.
pub fn get_with_url(
    t: &mut impl Transport,
    url: &str,
    options: Options,
    body: impl FnMut(&[u8]) -> Result<(), Error>,
    effective: impl FnMut(&str),
) -> Result<Response, Error> {
    request_with_url(t, url, Request::Get, options, body, effective)
}

/// GET or form POST with final-URL observation, bounded short-write progress,
/// cancellation and socket cleanup. 301/302/303 turn POST into GET; 307/308
/// replay a body only to the same origin. HTTPS downgrades are always rejected.
pub fn request_with_url(
    t: &mut impl Transport,
    url: &str,
    request: Request<'_>,
    options: Options,
    body: impl FnMut(&[u8]) -> Result<(), Error>,
    effective: impl FnMut(&str),
) -> Result<Response, Error> {
    request_with_cookies(t, url, request, options, body, effective, &mut ())
}
/// HTTP request with per-hop application cookie selection and normal TLS rules.
pub fn request_with_cookies(
    t: &mut impl Transport,
    url: &str,
    mut request: Request<'_>,
    options: Options,
    mut body: impl FnMut(&[u8]) -> Result<(), Error>,
    mut effective: impl FnMut(&str),
    cookies: &mut impl RequestCookies,
) -> Result<Response, Error> {
    if options.timeout_ticks == 0 || options.dns_port == 0 {
        return Err(Error::InvalidUrl);
    }
    if t.cancelled() {
        return Err(Error::Cancelled);
    }
    if matches!(request,Request::FormPost(data) if data.len()>limits::REQUEST_BODY) {
        return Err(Error::TooLarge);
    }
    if let Request::GetRange(range) = request {
        if range
            .last
            .checked_sub(range.first)
            .and_then(|n| n.checked_add(1))
            .is_none_or(|n| n > options.max_body as u64)
        {
            return Err(Error::TooLarge);
        }
    }
    let mut url = url::Url::parse(url)?;
    for redirect in 0..=options.redirects {
        if t.cancelled() {
            return Err(Error::Cancelled);
        }
        let ip = dns::resolve(t, url.host.as_str(), options)?;
        let handle = t.open(Kind::Tcp).map_err(Error::Socket)?;
        let mut location = url::Text::<{ limits::TARGET }>::new();
        let outcome = exchange(
            t,
            handle,
            ip,
            &url,
            request,
            options,
            &mut location,
            &mut body,
            cookies,
        )
        .map_err(|error| t.transport_error().unwrap_or(error));
        t.close(handle);
        let status = match outcome? {
            Outcome::Response(response) => {
                let mut final_url = url::Text::<{ limits::REQUEST }>::new();
                final_url.push(if url.tls { "https://" } else { "http://" })?;
                url.authority(&mut final_url)?;
                final_url.push(url.target.as_str())?;
                effective(final_url.as_str());
                return Ok(response);
            }
            Outcome::Redirect(status) => status,
        };
        if redirect == options.redirects {
            return Err(Error::RedirectLimit);
        }
        let next = url.redirect(location.as_str())?;
        if url.tls && !next.tls {
            return Err(Error::InsecureRedirect);
        }
        if matches!(request, Request::FormPost(_)) {
            if [301, 302, 303].contains(&status) {
                request = Request::Get;
            } else if url.tls != next.tls
                || url.port != next.port
                || !url.host.as_str().eq_ignore_ascii_case(next.host.as_str())
            {
                return Err(Error::CrossOriginRedirect);
            }
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
    request: Request<'_>,
    options: Options,
    location: &mut url::Text<{ limits::TARGET }>,
    body: &mut impl FnMut(&[u8]) -> Result<(), Error>,
    cookies: &mut impl RequestCookies,
) -> Result<Outcome, Error> {
    use rustos_net_abi::{CONNECTED, EOF};
    let start = t.ticks();
    t.connect(handle, ip, url.port).map_err(Error::Socket)?;
    loop {
        if t.cancelled() {
            return Err(Error::Cancelled);
        }
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
    let mut current = url::Text::<{ limits::REQUEST }>::new();
    current.push(if url.tls { "https://" } else { "http://" })?;
    url.authority(&mut current)?;
    current.push(url.target.as_str())?;
    send_request(
        t,
        handle,
        url,
        request,
        start,
        options,
        cookies.header(current.as_str())?,
    )?;
    let mut reader = wire::Reader::new(t, handle, start, options);
    let head = wire::headers(&mut reader, location)?;
    if [301, 302, 303, 307, 308].contains(&head.status) && !location.as_str().is_empty() {
        return Ok(Outcome::Redirect(head.status));
    }
    let expected = if let Request::GetRange(requested) = request {
        if head.status == 200 {
            return Err(Error::RangeIgnored);
        }
        if head.status == 416 {
            return Err(Error::RangeUnsatisfiable);
        }
        if head.status != 206 {
            return Err(Error::Protocol);
        }
        let Some(ContentRange::Satisfied { first, last, total }) = head.range else {
            return Err(Error::Protocol);
        };
        if first != requested.first || last != requested.last.min(total - 1) {
            return Err(Error::Protocol);
        }
        let length = usize::try_from(last - first + 1).map_err(|_| Error::TooLarge)?;
        if head.length.is_some_and(|n| n != length) {
            return Err(Error::Protocol);
        }
        reader.options.max_body = length;
        Some(length)
    } else {
        None
    };
    let bytes = wire::body(&mut reader, &head, body)?;
    if expected.is_some_and(|length| length != bytes) {
        return Err(Error::Truncated);
    }
    Ok(Outcome::Response(Response {
        status: head.status,
        charset: head.charset,
        bytes,
        range: head.range,
    }))
}

#[inline(never)]
fn send_request(
    t: &mut impl Transport,
    handle: u64,
    url: &url::Url,
    method: Request<'_>,
    start: u64,
    options: Options,
    cookie: &str,
) -> Result<(), Error> {
    use core::fmt::Write;
    let mut request = url::Text::<{ limits::REQUEST }>::new();
    request.push(match method {
        Request::Get | Request::GetRange(_) => "GET ",
        Request::FormPost(_) => "POST ",
    })?;
    request.push(url.target.as_str())?;
    request.push(" HTTP/1.1\r\nHost: ")?;
    url.authority(&mut request)?;
    request.push("\r\nUser-Agent: RustOS-curl/1\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n")?;
    if cookie.len() > 4096 {
        return Err(Error::TooLarge);
    }
    if !cookie.is_ascii() || cookie.bytes().any(|b| !(32..127).contains(&b)) {
        return Err(Error::Protocol);
    }
    if !cookie.is_empty() {
        request.push("Cookie: ")?;
        request.push(cookie)?;
        request.push("\r\n")?;
    }
    if let Request::GetRange(range) = method {
        write!(
            &mut request,
            "Range: bytes={}-{}\r\n",
            range.first, range.last
        )
        .map_err(|_| Error::TooLarge)?;
    }
    let data = match method {
        Request::Get | Request::GetRange(_) => &[][..],
        Request::FormPost(data) => {
            write!(
                &mut request,
                "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
                data.len()
            )
            .map_err(|_| Error::TooLarge)?;
            data
        }
    };
    request.push("\r\n")?;
    for bytes in [request.as_str().as_bytes(), data] {
        let mut sent = 0;
        while sent < bytes.len() {
            if t.cancelled() {
                return Err(Error::Cancelled);
            }
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
    }
    Ok(())
}
