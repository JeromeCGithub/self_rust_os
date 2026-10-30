use crate::{limits, Error};
use core::fmt::{self, Write};

#[derive(Clone)]
pub(crate) struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Text<N> {
    pub fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub fn push(&mut self, s: &str) -> Result<(), Error> {
        let end = self
            .len
            .checked_add(s.len())
            .filter(|n| *n <= N)
            .ok_or(Error::TooLarge)?;
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).expect("Text only accepts UTF-8 strings")
    }
}
impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push(s).map_err(|_| fmt::Error)
    }
}

pub(crate) struct Url {
    pub host: Text<253>,
    pub tls: bool,
    pub port: u16,
    pub target: Text<{ limits::TARGET }>,
}
impl Url {
    #[inline(never)]
    pub fn parse(input: &str) -> Result<Self, Error> {
        let (scheme, rest) = input.split_once("://").ok_or(Error::InvalidUrl)?;
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(Error::UnsupportedScheme);
        }
        if !input.is_ascii() || input.bytes().any(|b| b <= 32 || b == 127 || b == b'\\') {
            return Err(Error::InvalidUrl);
        }
        let tls = scheme.eq_ignore_ascii_case("https");
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or(Error::InvalidUrl)?,
            ),
            None => (authority, if tls { 443 } else { 80 }),
        };
        let dns_host = host.strip_suffix('.').unwrap_or(host);
        if dns_host.is_empty()
            || host.len() > 253
            || dns_host.split('.').any(|l| {
                l.is_empty()
                    || l.len() > 63
                    || l.starts_with('-')
                    || l.ends_with('-')
                    || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(Error::InvalidUrl);
        }
        let mut parsed = Self {
            host: Text::new(),
            tls,
            port,
            target: Text::new(),
        };
        parsed.host.push(host)?;
        let path = rest[end..].split('#').next().ok_or(Error::InvalidUrl)?;
        if !path.starts_with('/') {
            parsed.target.push("/")?;
        }
        parsed.target.push(path)?;
        Ok(parsed)
    }
    pub fn authority<const N: usize>(&self, into: &mut Text<N>) -> Result<(), Error> {
        into.push(self.host.as_str())?;
        if self.port != if self.tls { 443 } else { 80 } {
            write!(into, ":{}", self.port).map_err(|_| Error::TooLarge)?;
        }
        Ok(())
    }
    #[inline(never)]
    pub fn redirect(&self, location: &str) -> Result<Self, Error> {
        if location.is_empty() {
            return Err(Error::Protocol);
        }
        if location.contains("://") {
            return Self::parse(location);
        }
        let mut next = Text::<{ limits::REQUEST }>::new();
        next.push(if self.tls { "https:" } else { "http:" })?;
        if location.starts_with("//") {
            next.push(location)?;
            return Self::parse(next.as_str());
        }
        next.push("//")?;
        self.authority(&mut next)?;
        if location.starts_with('/') {
            next.push(location)?;
        } else if location.starts_with('?') || location.starts_with('#') {
            let path = self
                .target
                .as_str()
                .split('?')
                .next()
                .ok_or(Error::InvalidUrl)?;
            next.push(path)?;
            next.push(location)?;
        } else {
            let path = self
                .target
                .as_str()
                .split('?')
                .next()
                .ok_or(Error::InvalidUrl)?;
            let slash = path.rfind('/').ok_or(Error::InvalidUrl)?;
            next.push(&path[..slash + 1])?;
            next.push(location)?;
        }
        Self::parse(next.as_str())
    }
}
