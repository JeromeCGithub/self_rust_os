use crate::*;
use alloc::string::ToString;

fn valid(input: &str) -> Result<(), Error> {
    if input.len() > 2048
        || !input.is_ascii()
        || input.bytes().any(|b| b <= 32 || b == 127 || b == b'\\')
    {
        return Err(Error::InvalidUrl);
    }
    Ok(())
}
fn absolute(input: &str) -> Result<String, Error> {
    valid(input)?;
    let (scheme, rest) = input.split_once("://").ok_or(Error::InvalidUrl)?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(Error::InvalidUrl);
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let (host, port) = authority
        .split_once(':')
        .map(|(h, p)| (h, Some(p)))
        .unwrap_or((authority, None));
    if host.is_empty()
        || host.len() > 253
        || host.split('.').any(|l| {
            l.is_empty()
                || l.len() > 63
                || l.starts_with('-')
                || l.ends_with('-')
                || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        || port.is_some_and(|p| p.parse::<u16>().ok().filter(|p| *p > 0).is_none())
    {
        return Err(Error::InvalidUrl);
    }
    let tail = &rest[end..];
    Ok(alloc::format!(
        "{}://{}{}{}",
        scheme.to_ascii_lowercase(),
        authority,
        if tail.starts_with('/') { "" } else { "/" },
        tail
    ))
}
/// Accept HTTP(S) addresses or turn words into an encoded web search URL.
pub fn address(input: &str) -> Result<String, Error> {
    let input = input.trim();
    if input.is_empty() || input.len() > 2048 || input.bytes().any(|b| b < 32 || b == 127) {
        return Err(Error::InvalidUrl);
    }
    if input.contains("://") {
        return absolute(input);
    }
    // Bare host:port is useful for development servers; other schemes are denied.
    if let Some((host, tail)) = input.split_once(':') {
        if !host.contains('.')
            || !tail
                .split('/')
                .next()
                .is_some_and(|p| p.parse::<u16>().is_ok())
        {
            return Err(Error::InvalidUrl);
        }
    }
    if !input.contains(' ') && (input.contains('.') || input.starts_with("localhost")) {
        return absolute(&alloc::format!("https://{input}"));
    }
    let mut out = String::from("https://www.google.com/search?q=");
    use core::fmt::Write;
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    if out.len() > 2048 {
        return Err(Error::InvalidUrl);
    }
    Ok(out)
}
/// Resolve relative references, queries and fragments; deny active/local schemes.
pub fn resolve(base: &str, reference: &str) -> Result<String, Error> {
    valid(reference)?;
    let base = absolute(base)?;
    if reference.contains("://") {
        return absolute(reference);
    }
    if reference
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|p| p.contains(':'))
    {
        return Err(Error::InvalidUrl);
    }
    let scheme_end = base.find("://").ok_or(Error::InvalidUrl)?;
    if reference.starts_with("//") {
        return absolute(&alloc::format!("{}:{reference}", &base[..scheme_end]));
    }
    let origin_end = scheme_end + 3 + base[scheme_end + 3..].find('/').ok_or(Error::InvalidUrl)?;
    let origin = &base[..origin_end];
    let old_path = base[origin_end..]
        .split(['?', '#'])
        .next()
        .ok_or(Error::InvalidUrl)?;
    if reference.is_empty() {
        return Ok(base);
    }
    if reference.starts_with('#') {
        return Ok(alloc::format!(
            "{}{reference}",
            base.split('#').next().ok_or(Error::InvalidUrl)?
        ));
    }
    if reference.starts_with('?') {
        return Ok(alloc::format!("{origin}{old_path}{reference}"));
    }
    let path = if reference.starts_with('/') {
        reference.to_string()
    } else {
        alloc::format!(
            "{}{reference}",
            &old_path[..old_path.rfind('/').ok_or(Error::InvalidUrl)? + 1]
        )
    };
    let split = path.find(['?', '#']).unwrap_or(path.len());
    let mut segments = Vec::new();
    for part in path[..split].split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(part),
        }
    }
    let slash = if path[..split].ends_with('/')
        || path[..split].ends_with("/.")
        || path[..split].ends_with("/..")
    {
        "/"
    } else {
        ""
    };
    absolute(&alloc::format!(
        "{origin}/{}{}{tail}",
        segments.join("/"),
        if segments.is_empty() { "" } else { slash },
        tail = &path[split..]
    ))
}
