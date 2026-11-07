//! In-memory host-only document cookies. No domain cookies or authentication store.
use crate::resolve;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
/// Maximum serialized Cookie header and number of host-only entries per session.
pub const MAX_COOKIE_BYTES: usize = 4096;
const MAX_COOKIES: usize = 32;
#[derive(Clone)]
struct Cookie {
    host: String,
    path: String,
    name: String,
    value: String,
    secure: bool,
    expires: Option<u64>,
}
/// Private values deliberately do not implement Debug. The loader owns this jar;
/// workers receive only the cookies visible to the current document.
#[derive(Default, Clone)]
pub struct CookieJar {
    entries: Vec<Cookie>,
}
fn scope(url: &str) -> Option<(String, String, bool)> {
    let url = resolve(url, "").ok()?;
    let (scheme, rest) = url.split_once("://")?;
    let (authority, tail) = rest.split_once('/')?;
    let host = authority.split(':').next()?.to_ascii_lowercase();
    let path = alloc::format!("/{}", tail.split(['?', '#']).next()?);
    Some((host, path, scheme == "https"))
}
fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie
        || (request.starts_with(cookie)
            && (cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/')))
}
// Bounded IMF-fixdate expiry profile. Unrecognized Expires attributes are ignored
// as required for an invalid cookie-date; Max-Age takes precedence.
fn expiry(text: &str) -> Option<u64> {
    let parts: Vec<_> = text.split_ascii_whitespace().collect();
    if parts.len() != 6 || parts[5] != "GMT" {
        return None;
    }
    let year = parts[3].parse::<u64>().ok()?;
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| m.eq_ignore_ascii_case(parts[2]))?;
    let leap = |y: u64| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mut months = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if leap(year) {
        months[1] = 29;
    }
    let day = parts[1].parse::<u64>().ok()?;
    if day == 0 || day > months[month] {
        return None;
    }
    let time: Vec<_> = parts[4].split(':').collect();
    if time.len() != 3 {
        return None;
    }
    let h = time[0].parse::<u64>().ok()?;
    let m = time[1].parse::<u64>().ok()?;
    let s = time[2].parse::<u64>().ok()?;
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    let leap_before = |y: u64| (y - 1) / 4 - (y - 1) / 100 + (y - 1) / 400;
    let days = (year - 1970) * 365 + leap_before(year) - leap_before(1970)
        + months[..month].iter().sum::<u64>()
        + day
        - 1;
    Some(days * 86400 + h * 3600 + m * 60 + s)
}
impl CookieJar {
    /// Apply a script assignment atomically. Domain/HttpOnly cookies are outside
    /// this profile and rejected. There is no disk persistence or implicit eviction.
    pub fn set_document(&mut self, url: &str, assignment: &str, now: u64) -> bool {
        if assignment.len() > MAX_COOKIE_BYTES
            || !assignment.is_ascii()
            || assignment.bytes().any(|b| b < 32 || b == 127)
        {
            return false;
        }
        let Some((host, request_path, tls)) = scope(url) else {
            return false;
        };
        let mut parts = assignment.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.trim().split_once('=')) else {
            return false;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || !value.bytes().all(|b| {
                b == 0x21
                    || (0x23..=0x2b).contains(&b)
                    || (0x2d..=0x3a).contains(&b)
                    || (0x3c..=0x5b).contains(&b)
                    || (0x5d..=0x7e).contains(&b)
            })
        {
            return false;
        }
        let mut path = match request_path.rfind('/') {
            Some(n) if n > 0 => request_path[..n].to_string(),
            _ => "/".to_string(),
        };
        let mut secure = false;
        let mut expires = None;
        let mut max_age = None;
        for part in parts {
            let (key, v) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
            let key = key.trim();
            let v = v.trim();
            if key.eq_ignore_ascii_case("domain") || key.eq_ignore_ascii_case("httponly") {
                return false;
            }
            if key.eq_ignore_ascii_case("secure") {
                secure = true;
            }
            if key.eq_ignore_ascii_case("path") && v.starts_with('/') {
                path = v.to_string();
            }
            if key.eq_ignore_ascii_case("expires") {
                expires = expiry(v);
            }
            if key.eq_ignore_ascii_case("max-age") && !v.starts_with('+') {
                max_age = v.parse::<i64>().ok();
            }
        }
        if secure && !tls
            || name.starts_with("__Secure-") && !secure
            || name.starts_with("__Host-") && (!secure || path != "/")
        {
            return false;
        }
        if let Some(age) = max_age {
            expires = Some(if age <= 0 {
                0
            } else {
                now.saturating_add(age as u64)
            });
        }
        self.entries.retain(|c| c.expires.is_none_or(|e| e > now));
        let same = |c: &Cookie| c.host == host && c.path == path && c.name == name;
        // Insecure scripts must not replace a secure cookie on an overlapping path.
        if !tls
            && self.entries.iter().any(|c| {
                c.secure && c.host == host && c.name == name && path_matches(&path, &c.path)
            })
        {
            return false;
        }
        if expires.is_some_and(|e| e <= now) {
            self.entries.retain(|c| !same(c));
            return true;
        }
        let index = self.entries.iter().position(same);
        let bytes: usize = self
            .entries
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != index)
            .map(|(_, c)| c.name.len() + c.value.len() + 3)
            .sum();
        if index.is_none() && self.entries.len() >= MAX_COOKIES
            || bytes + name.len() + value.len() + 3 > MAX_COOKIE_BYTES
        {
            return false;
        }
        let cookie = Cookie {
            host,
            path,
            name: name.to_string(),
            value: value.to_string(),
            secure,
            expires,
        };
        match index {
            Some(i) => self.entries[i] = cookie,
            None => self.entries.push(cookie),
        }
        true
    }
    /// Return only host/path/transport-matching unexpired values, longest path
    /// first. Ports intentionally do not scope cookies (RFC6265 responsibility).
    pub fn header(&mut self, url: &str, now: u64) -> String {
        self.entries.retain(|c| c.expires.is_none_or(|e| e > now));
        let Some((host, path, tls)) = scope(url) else {
            return String::new();
        };
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|c| c.host == host && (!c.secure || tls) && path_matches(&path, &c.path))
            .collect();
        entries.sort_by_key(|c| core::cmp::Reverse(c.path.len()));
        let mut result = String::new();
        for c in entries {
            if !result.is_empty() {
                result.push_str("; ");
            }
            result.push_str(&c.name);
            result.push('=');
            result.push_str(&c.value);
        }
        result
    }
    /// Script-visible entries with scope/remaining lifetime, for a fresh sealed
    /// realm. This private snapshot is not an HTTP header and must not be logged.
    pub fn snapshot(&mut self, url: &str, now: u64) -> String {
        self.entries.retain(|c| c.expires.is_none_or(|e| e > now));
        let Some((host, path, tls)) = scope(url) else {
            return String::new();
        };
        let mut result = String::new();
        for c in self
            .entries
            .iter()
            .filter(|c| c.host == host && (!c.secure || tls) && path_matches(&path, &c.path))
        {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&alloc::format!(
                "{}={}; Path={}{}",
                c.name,
                c.value,
                c.path,
                if c.secure { "; Secure" } else { "" }
            ));
            if let Some(expiry) = c.expires {
                result.push_str(&alloc::format!("; Max-Age={}", expiry - now));
            }
        }
        result
    }
}
