//! Copied page context and session effects, separate from document mutations.
use crate::{dom::Changes, Error};
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
/// Trusted controller snapshot. Cookie values must never be included in logs.
#[derive(Clone, Copy)]
pub struct PageRequest<'a> {
    pub html: &'a str,
    pub url: &'a str,
    /// Newline-delimited host-only assignments from CookieJar::snapshot, not an
    /// HTTP Cookie header. Scope and remaining lifetime survive realm bootstrap.
    pub cookie_snapshot: &'a str,
    pub epoch_seconds: u64,
}
/// Untrusted effects revalidated by the browser controller before publication.
pub struct PageReply {
    pub dom: Changes,
    pub cookies: Vec<String>,
    pub navigation: Option<String>,
}
impl PageRequest<'_> {
    /// Encode JPQ1 within the complete 128 KiB worker input envelope.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        validate_request(*self)?;
        let mut bytes = Vec::from(&b"JPQ1"[..]);
        bytes.extend_from_slice(&self.epoch_seconds.to_le_bytes());
        put(&mut bytes, self.url.as_bytes(), 2048)?;
        put(&mut bytes, self.cookie_snapshot.as_bytes(), 4096)?;
        put(&mut bytes, self.html.as_bytes(), 128 * 1024)?;
        if bytes.len() > 128 * 1024 {
            return Err(Error::Limit);
        }
        Ok(bytes)
    }
    /// Borrow checked UTF-8 fields; reject incomplete, oversized and trailing data.
    pub fn decode(bytes: &[u8]) -> Result<PageRequest<'_>, Error> {
        if bytes.len() > 128 * 1024 || bytes.get(..4) != Some(b"JPQ1") {
            return Err(Error::Limit);
        }
        let epoch_seconds = u64::from_le_bytes(
            bytes
                .get(4..12)
                .ok_or(Error::Limit)?
                .try_into()
                .map_err(|_| Error::Limit)?,
        );
        let mut reader = Reader { bytes, at: 12 };
        let url = reader.text(2048)?;
        let cookie_snapshot = reader.text(4096)?;
        let html = reader.text(128 * 1024)?;
        if reader.at != bytes.len() {
            return Err(Error::Limit);
        }
        let request = PageRequest {
            html,
            url,
            cookie_snapshot,
            epoch_seconds,
        };
        validate_request(request)?;
        Ok(request)
    }
}
impl PageReply {
    /// Validate the complete untrusted worker reply before publishing any DOM or
    /// cookie mutation. Navigation remains an HTTP(S) controller decision.
    pub fn commit(
        &self,
        document: &mut crate::Document,
        jar: &mut crate::cookies::CookieJar,
        url: &str,
        now: u64,
    ) -> Result<Option<String>, Error> {
        self.encode()?;
        let base = crate::resolve(url, "")?;
        let navigation = self
            .navigation
            .as_ref()
            .map(|next| crate::resolve(&base, next))
            .transpose()?;
        if base.starts_with("https://")
            && navigation
                .as_ref()
                .is_some_and(|n| !n.starts_with("https://"))
        {
            return Err(Error::InvalidUrl);
        }
        let mut next = jar.clone();
        for cookie in &self.cookies {
            if !next.set_document(&base, cookie, now) {
                return Err(Error::Limit);
            }
        }
        document.apply_changes(&self.dom)?;
        *jar = next;
        Ok(navigation)
    }
    /// Encode JPR1 with the DOM and all session effects in one 8192-byte budget.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.cookies.len() > 32 {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::from(&b"JPR1"[..]);
        bytes.extend_from_slice(&(self.cookies.len() as u32).to_le_bytes());
        put(&mut bytes, &self.dom.encode()?, 8192)?;
        for cookie in &self.cookies {
            if !header_text(cookie) {
                return Err(Error::Limit);
            }
            put(&mut bytes, cookie.as_bytes(), 4096)?;
        }
        if self.navigation.as_deref().is_some_and(|url| url.is_empty()) {
            return Err(Error::InvalidUrl);
        }
        let nav = self.navigation.as_deref().unwrap_or("");
        if !header_text(nav) {
            return Err(Error::InvalidUrl);
        }
        put(&mut bytes, nav.as_bytes(), 2048)?;
        if bytes.len() > 8192 {
            return Err(Error::Limit);
        }
        Ok(bytes)
    }
    /// Decode copied effects without trusting their origin or navigation policy.
    /// Call commit before exposing any effect to the active application session.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 8192 || bytes.get(..4) != Some(b"JPR1") {
            return Err(Error::Limit);
        }
        let mut reader = Reader { bytes, at: 4 };
        let count = reader.number()?;
        if count > 32 {
            return Err(Error::Limit);
        }
        let dom = Changes::decode(reader.field(8192)?)?;
        let mut cookies = Vec::new();
        for _ in 0..count {
            let value = reader.text(4096)?;
            if !header_text(value) {
                return Err(Error::Limit);
            }
            cookies.push(value.to_string());
        }
        let nav = reader.text(2048)?;
        if !header_text(nav) || reader.at != bytes.len() {
            return Err(Error::Limit);
        }
        Ok(Self {
            dom,
            cookies,
            navigation: if nav.is_empty() {
                None
            } else {
                Some(nav.to_string())
            },
        })
    }
}
fn header_text(text: &str) -> bool {
    text.is_ascii() && text.bytes().all(|b| (32..127).contains(&b))
}
fn validate_request(request: PageRequest<'_>) -> Result<(), Error> {
    if request.url.len() > 2048
        || !request.cookie_snapshot.is_ascii()
        || request
            .cookie_snapshot
            .bytes()
            .any(|b| b != b'\n' && !(32..127).contains(&b))
        || request.cookie_snapshot.len() > 4096
    {
        return Err(Error::Limit);
    }
    crate::resolve(request.url, "")?;
    Ok(())
}
fn put(bytes: &mut Vec<u8>, value: &[u8], limit: usize) -> Result<(), Error> {
    if value.len() > limit {
        return Err(Error::Limit);
    }
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value);
    Ok(())
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn number(&mut self) -> Result<usize, Error> {
        let value = u32::from_le_bytes(
            self.bytes
                .get(self.at..self.at + 4)
                .ok_or(Error::Limit)?
                .try_into()
                .map_err(|_| Error::Limit)?,
        ) as usize;
        self.at += 4;
        Ok(value)
    }
    fn field(&mut self, limit: usize) -> Result<&'a [u8], Error> {
        let n = self.number()?;
        if n > limit {
            return Err(Error::Limit);
        }
        let end = self.at.checked_add(n).ok_or(Error::Limit)?;
        let value = self.bytes.get(self.at..end).ok_or(Error::Limit)?;
        self.at = end;
        Ok(value)
    }
    fn text(&mut self, limit: usize) -> Result<&'a str, Error> {
        core::str::from_utf8(self.field(limit)?).map_err(|_| Error::Encoding)
    }
}
