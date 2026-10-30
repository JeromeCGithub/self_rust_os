//! Incremental HTTP/1 framing over a buffered TCP byte stream.
use crate::url::Text;
use crate::{limits, Error, Options, Transport};
use rustos_net_abi::{Error as SocketError, Packet};

pub(crate) struct Reader<'a, T: Transport> {
    pub t: &'a mut T,
    pub handle: u64,
    pub start: u64,
    pub options: Options,
    data: Packet,
    at: usize,
    len: usize,
}
impl<'a, T: Transport> Reader<'a, T> {
    pub fn new(t: &'a mut T, handle: u64, start: u64, options: Options) -> Self {
        Self {
            t,
            handle,
            start,
            options,
            data: Packet::empty(),
            at: 0,
            len: 0,
        }
    }
    pub fn deadline(&self) -> Result<(), Error> {
        if self.t.ticks().wrapping_sub(self.start) >= self.options.timeout_ticks {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.at == self.len {
            self.deadline()?;
            match self.t.recv(self.handle, &mut self.data) {
                Ok(()) => {
                    self.len = usize::from(self.data.len);
                    self.at = 0;
                    if self.len > self.data.data.len() {
                        return Err(Error::Protocol);
                    }
                    if self.len == 0 {
                        return Ok(0);
                    }
                }
                Err(SocketError::WouldBlock) => self.t.sleep(),
                Err(e) => return Err(Error::Socket(e)),
            }
        }
        let n = out.len().min(self.len - self.at);
        out[..n].copy_from_slice(&self.data.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
    pub fn byte(&mut self) -> Result<u8, Error> {
        let mut b = [0];
        if self.read(&mut b)? == 0 {
            Err(Error::Truncated)
        } else {
            Ok(b[0])
        }
    }
    pub fn line(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        let mut len = 0;
        loop {
            let b = self.byte()?;
            if b == b'\r' {
                if self.byte()? != b'\n' {
                    return Err(Error::Protocol);
                }
                return Ok(len);
            }
            if b == b'\n' || (b < 32 && b != b'\t') || b == 127 {
                return Err(Error::Protocol);
            }
            if len == out.len() {
                return Err(Error::TooLarge);
            }
            out[len] = b;
            len += 1;
        }
    }
}
pub(crate) struct Head {
    pub status: u16,
    pub length: Option<usize>,
    pub chunked: bool,
}
fn number(bytes: &[u8], radix: u32) -> Result<usize, Error> {
    if bytes.is_empty() {
        return Err(Error::Protocol);
    }
    bytes.iter().try_fold(0usize, |n, b| {
        let digit = (*b as char).to_digit(radix).ok_or(Error::Protocol)?;
        n.checked_mul(radix as usize)
            .and_then(|n| n.checked_add(digit as usize))
            .ok_or(Error::TooLarge)
    })
}
fn trim(mut b: &[u8]) -> &[u8] {
    while b.first().is_some_and(|b| *b == b' ' || *b == b'\t') {
        b = &b[1..];
    }
    while b.last().is_some_and(|b| *b == b' ' || *b == b'\t') {
        b = &b[..b.len() - 1];
    }
    b
}
fn field(line: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let at = line
        .iter()
        .position(|b| *b == b':')
        .ok_or(Error::Protocol)?;
    if at == 0
        || !line[..at]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
    {
        return Err(Error::Protocol);
    }
    Ok((&line[..at], trim(&line[at + 1..])))
}
#[inline(never)]
pub(crate) fn headers(
    r: &mut Reader<'_, impl Transport>,
    location: &mut Text<{ limits::TARGET }>,
) -> Result<Head, Error> {
    let mut line = [0; limits::LINE];
    let mut total = 0;
    for _ in 0..9 {
        let len = r.line(&mut line)?;
        total += len + 2;
        if len < 12
            || !(line.starts_with(b"HTTP/1.1 ") || line.starts_with(b"HTTP/1.0 "))
            || (len > 12 && line[12] != b' ')
        {
            return Err(Error::Protocol);
        }
        let status = number(&line[9..12], 10)? as u16;
        if !(100..600).contains(&status) {
            return Err(Error::Protocol);
        }
        let mut head = Head {
            status,
            length: None,
            chunked: false,
        };
        loop {
            let n = r.line(&mut line)?;
            total += n + 2;
            if total > limits::HEADERS {
                return Err(Error::TooLarge);
            }
            if n == 0 {
                break;
            }
            let (k, v) = field(&line[..n])?;
            if k.eq_ignore_ascii_case(b"content-length") {
                if head.length.is_some() {
                    return Err(Error::Protocol);
                }
                head.length = Some(number(v, 10)?);
            }
            if k.eq_ignore_ascii_case(b"transfer-encoding") {
                if head.chunked || !v.eq_ignore_ascii_case(b"chunked") {
                    return Err(Error::Protocol);
                }
                head.chunked = true;
            }
            if k.eq_ignore_ascii_case(b"content-encoding") && !v.eq_ignore_ascii_case(b"identity") {
                return Err(Error::Protocol);
            }
            if k.eq_ignore_ascii_case(b"location") {
                if !location.as_str().is_empty() {
                    return Err(Error::Protocol);
                }
                location.push(core::str::from_utf8(v).map_err(|_| Error::Protocol)?)?;
            }
        }
        if head.chunked && head.length.is_some() {
            return Err(Error::Protocol);
        }
        if status == 101 {
            return Err(Error::Protocol);
        }
        if status >= 200 {
            return Ok(head);
        }
    }
    Err(Error::Protocol)
}
#[inline(never)]
pub(crate) fn body(
    r: &mut Reader<'_, impl Transport>,
    head: &Head,
    mut sink: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<usize, Error> {
    if head.status == 204 || head.status == 304 {
        return Ok(0);
    }
    let mut buffer = [0; 1024];
    let mut metadata = [0; limits::LINE];
    let mut total = 0usize;
    let mut remaining = head.length;
    if let Some(n) = remaining {
        if n > r.options.max_body {
            return Err(Error::TooLarge);
        }
    }
    loop {
        if head.chunked {
            let n = r.line(&mut metadata)?;
            let size = &metadata[..n]
                .split(|b| *b == b';')
                .next()
                .ok_or(Error::Protocol)?;
            let count = number(size, 16)?;
            if count == 0 {
                let mut trailers = 0;
                loop {
                    let n = r.line(&mut metadata)?;
                    trailers += n + 2;
                    if trailers > limits::TRAILERS {
                        return Err(Error::TooLarge);
                    }
                    if n == 0 {
                        return Ok(total);
                    }
                    let (k, _) = field(&metadata[..n])?;
                    if k.eq_ignore_ascii_case(b"content-length")
                        || k.eq_ignore_ascii_case(b"transfer-encoding")
                    {
                        return Err(Error::Protocol);
                    }
                }
            }
            remaining = Some(count);
        }
        if let Some(n) = remaining {
            if total
                .checked_add(n)
                .filter(|v| *v <= r.options.max_body)
                .is_none()
            {
                return Err(Error::TooLarge);
            }
        }
        loop {
            let capacity = remaining.unwrap_or(buffer.len()).min(buffer.len());
            if capacity == 0 {
                break;
            }
            let n = r.read(&mut buffer[..capacity])?;
            if n == 0 {
                return if remaining.is_some() {
                    Err(Error::Truncated)
                } else {
                    Ok(total)
                };
            }
            total = total
                .checked_add(n)
                .filter(|v| *v <= r.options.max_body)
                .ok_or(Error::TooLarge)?;
            sink(&buffer[..n])?;
            if let Some(left) = remaining.as_mut() {
                *left -= n;
            }
        }
        if !head.chunked {
            return Ok(total);
        }
        if r.byte()? != b'\r' || r.byte()? != b'\n' {
            return Err(Error::Protocol);
        }
    }
}
