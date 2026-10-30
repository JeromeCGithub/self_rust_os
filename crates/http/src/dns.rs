//! Bounded DNS A lookups over connected UDP, with compressed names and CNAME chains.
use crate::url::Text;
use crate::{Error, Options, Transport};
use rustos_net_abi::{Error as SocketError, Kind, Packet};

fn word(bytes: &[u8], at: usize) -> Result<u16, Error> {
    let b = bytes
        .get(at..at.checked_add(2).ok_or(Error::Dns)?)
        .ok_or(Error::Dns)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}
fn name(bytes: &[u8], cursor: &mut usize) -> Result<Text<253>, Error> {
    let mut out = Text::new();
    let mut at = *cursor;
    let mut jumped = false;
    for _ in 0..128 {
        let len = *bytes.get(at).ok_or(Error::Dns)?;
        if len & 0xc0 == 0xc0 {
            let ptr = usize::from(word(bytes, at)? & 0x3fff);
            if !jumped {
                *cursor = at + 2;
                jumped = true;
            }
            at = ptr;
            continue;
        }
        if len & 0xc0 != 0 {
            return Err(Error::Dns);
        }
        at += 1;
        if len == 0 {
            if !jumped {
                *cursor = at;
            }
            return Ok(out);
        }
        let label = bytes.get(at..at + usize::from(len)).ok_or(Error::Dns)?;
        if !label
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
        {
            return Err(Error::Dns);
        }
        if !out.as_str().is_empty() {
            out.push(".")?;
        }
        out.push(core::str::from_utf8(label).map_err(|_| Error::Dns)?)?;
        at += usize::from(len);
    }
    Err(Error::Dns)
}
fn query(host: &str, id: u16, into: &mut [u8; 512]) -> Result<usize, Error> {
    into[..12].copy_from_slice(&[0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    into[..2].copy_from_slice(&id.to_be_bytes());
    let mut at = 12;
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 || at + label.len() + 1 + 5 > into.len() {
            return Err(Error::Dns);
        }
        into[at] = label.len() as u8;
        at += 1;
        into[at..at + label.len()].copy_from_slice(label.as_bytes());
        at += label.len();
    }
    into[at..at + 5].copy_from_slice(&[0, 0, 1, 0, 1]);
    Ok(at + 5)
}
#[inline(never)]
fn answer(bytes: &[u8], host: &str, id: u16) -> Result<[u8; 4], Error> {
    if word(bytes, 0)? != id || word(bytes, 2)? & 0xfa0f != 0x8000 || word(bytes, 4)? != 1 {
        return Err(Error::Dns);
    }
    let count = usize::from(word(bytes, 6)?);
    if count == 0 || count > 64 {
        return Err(Error::Dns);
    }
    let mut at = 12;
    if !name(bytes, &mut at)?
        .as_str()
        .eq_ignore_ascii_case(host.trim_end_matches('.'))
        || word(bytes, at)? != 1
        || word(bytes, at + 2)? != 1
    {
        return Err(Error::Dns);
    }
    let start = at + 4;
    let mut wanted = Text::<253>::new();
    wanted.push(host.trim_end_matches('.'))?;
    for _ in 0..8 {
        at = start;
        let mut cname = None;
        for _ in 0..count {
            let owner = name(bytes, &mut at)?;
            let kind = word(bytes, at)?;
            let class = word(bytes, at + 2)?;
            let len = usize::from(word(bytes, at + 8)?);
            at += 10;
            let end = at
                .checked_add(len)
                .filter(|end| *end <= bytes.len())
                .ok_or(Error::Dns)?;
            if class == 1 && owner.as_str().eq_ignore_ascii_case(wanted.as_str()) {
                if kind == 1 && len == 4 {
                    return Ok([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
                }
                if kind == 5 {
                    let mut cursor = at;
                    let target = name(bytes, &mut cursor)?;
                    if cursor != end {
                        return Err(Error::Dns);
                    }
                    cname = Some(target);
                }
            }
            at = end;
        }
        wanted = cname.ok_or(Error::Dns)?;
    }
    Err(Error::Dns)
}

#[inline(never)]
pub(crate) fn resolve(
    t: &mut impl Transport,
    host: &str,
    options: Options,
) -> Result<[u8; 4], Error> {
    if let Ok(ip) = host.parse::<core::net::Ipv4Addr>() {
        return Ok(ip.octets());
    }
    let handle = t.open(Kind::Udp).map_err(Error::Socket)?;
    let result = (|| {
        t.connect(handle, options.dns_server, options.dns_port)
            .map_err(Error::Socket)?;
        let id = (t.ticks() as u16) ^ (handle as u16).rotate_left(7) ^ 0x6d31;
        let mut buffer = [0; 512];
        let len = query(host, id, &mut buffer)?;
        let mut packet = Packet::empty();
        let start = t.ticks();
        let mut sends = 0;
        let mut last = start;
        loop {
            if t.ticks().wrapping_sub(start) >= options.timeout_ticks.min(54) {
                return Err(Error::Timeout);
            }
            if sends < 3 && (sends == 0 || t.ticks().wrapping_sub(last) >= 18) {
                match t.send(handle, &buffer[..len]) {
                    Ok(n) if n == len => {
                        sends += 1;
                        last = t.ticks();
                    }
                    Err(SocketError::WouldBlock) => (),
                    Err(e) => return Err(Error::Socket(e)),
                    _ => return Err(Error::Protocol),
                }
            }
            match t.recv(handle, &mut packet) {
                Ok(()) => {
                    let bytes = &packet.data[..usize::from(packet.len).min(packet.data.len())];
                    if packet.address == options.dns_server
                        && packet.port == options.dns_port
                        && bytes.len() >= 2
                        && word(bytes, 0)? == id
                    {
                        return answer(bytes, host, id);
                    }
                }
                Err(SocketError::WouldBlock) => (),
                Err(e) => return Err(Error::Socket(e)),
            }
            t.sleep();
        }
    })();
    t.close(handle);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_and_compressed_a_response() {
        let mut b = [0; 512];
        let n = query("example.test", 42, &mut b).unwrap();
        b[2] = 0x81;
        b[3] = 0x80;
        b[7] = 1;
        b[n..n + 16].copy_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 10, 0, 2, 2]);
        assert_eq!(answer(&b[..n + 16], "example.test", 42), Ok([10, 0, 2, 2]));
        for len in 0..n + 16 {
            assert!(answer(&b[..len], "example.test", 42).is_err());
        }
        assert!(answer(&b[..n + 16], "other.test", 42).is_err());
        b[2] |= 2;
        assert!(answer(&b[..n + 16], "example.test", 42).is_err());
    }
    #[test]
    fn name_pointer_cycle_is_bounded() {
        let mut at = 0;
        assert!(name(&[0xc0, 0], &mut at).is_err());
    }
}
