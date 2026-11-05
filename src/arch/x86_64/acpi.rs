//! Bounded ACPI interrupt-controller topology parsing.

/// Maximum number of schedulable logical CPUs.
pub const MAX_CPUS: usize = 32;
/// Enabled xAPIC processors and the local interrupt-controller address.
#[derive(Debug, PartialEq, Eq)]
pub struct Topology {
    /// Firmware APIC identifiers; not assumed contiguous.
    pub ids: [u8; MAX_CPUS],
    /// Number of enabled processors.
    pub count: usize,
    /// Physical local APIC register page.
    pub local_apic: u64,
}
/// Invalid or unsupported firmware data.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Malformed length, checksum, enabled identity or APIC address.
    Invalid,
    /// Processor identities require unsupported x2APIC addressing.
    Unsupported,
    /// More than 32 enabled logical processors were supplied.
    TooManyCpus,
}

/// Parse one complete, checksummed MADT.
pub fn parse_madt(bytes: &[u8]) -> Result<Topology, Error> {
    if bytes.len() < 44
        || &bytes[..4] != b"APIC"
        || u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| Error::Invalid)?) as usize
            != bytes.len()
        || bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0
    {
        return Err(Error::Invalid);
    }
    let mut result = Topology {
        ids: [0; MAX_CPUS],
        count: 0,
        local_apic: u64::from(u32::from_le_bytes(
            bytes[36..40].try_into().map_err(|_| Error::Invalid)?,
        )),
    };
    let mut pos = 44;
    while pos < bytes.len() {
        if bytes.len() - pos < 2 {
            return Err(Error::Invalid);
        }
        let len = bytes[pos + 1] as usize;
        if len < 2 || len > bytes.len() - pos {
            return Err(Error::Invalid);
        }
        let b = &bytes[pos..pos + len];
        let cpu = match b[0] {
            0 if len == 8 => Some((
                u32::from(b[3]),
                u32::from_le_bytes(b[4..8].try_into().map_err(|_| Error::Invalid)?),
            )),
            9 if len == 16 => Some((
                u32::from_le_bytes(b[4..8].try_into().map_err(|_| Error::Invalid)?),
                u32::from_le_bytes(b[8..12].try_into().map_err(|_| Error::Invalid)?),
            )),
            5 if len == 12 => {
                result.local_apic =
                    u64::from_le_bytes(b[4..12].try_into().map_err(|_| Error::Invalid)?);
                None
            }
            0 | 5 | 9 => return Err(Error::Invalid),
            _ => None,
        };
        if let Some((id, flags)) = cpu {
            if flags & 1 != 0 {
                let id = u8::try_from(id).map_err(|_| Error::Unsupported)?;
                if result.ids[..result.count].contains(&id) {
                    return Err(Error::Invalid);
                }
                if result.count == MAX_CPUS {
                    return Err(Error::TooManyCpus);
                }
                result.ids[result.count] = id;
                result.count += 1;
            }
        }
        pos += len;
    }
    if result.count == 0 || result.local_apic == 0 || result.local_apic & 4095 != 0 {
        return Err(Error::Invalid);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::{vec, vec::Vec};
    fn table(ids: &[(u8, bool)]) -> Vec<u8> {
        let mut b = vec![0; 44];
        b[..4].copy_from_slice(b"APIC");
        b[36..40].copy_from_slice(&0xfee00000u32.to_le_bytes());
        for &(id, enabled) in ids {
            b.extend_from_slice(&[0, 8, id, id, u8::from(enabled), 0, 0, 0]);
        }
        seal(&mut b);
        b
    }
    fn seal(b: &mut [u8]) {
        let len = b.len() as u32;
        b[4..8].copy_from_slice(&len.to_le_bytes());
        b[9] = 0;
        b[9] = 0u8.wrapping_sub(b.iter().fold(0u8, |s, v| s.wrapping_add(*v)));
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn accepts_one_through_thirty_two_and_sparse_ids() {
        for n in 1..=32 {
            let ids: Vec<_> = (0..n).map(|i| ((i * 3) as u8, true)).collect();
            let t = parse_madt(&table(&ids)).expect("valid enabled CPUs");
            assert_eq!(t.count, n);
            assert_eq!(&t.ids[..n], &ids.iter().map(|v| v.0).collect::<Vec<_>>());
        }
        let t = parse_madt(&table(&[(9, true), (17, false), (44, true)])).unwrap();
        assert_eq!(&t.ids[..t.count], &[9, 44]);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn rejects_corruption_duplicates_overflow_and_truncation() {
        assert_eq!(
            parse_madt(&table(&[(2, true), (2, true)])),
            Err(Error::Invalid)
        );
        assert_eq!(
            parse_madt(&table(&(0..33).map(|i| (i, true)).collect::<Vec<_>>())),
            Err(Error::TooManyCpus)
        );
        let mut b = table(&[(1, true)]);
        b[9] ^= 1;
        assert_eq!(parse_madt(&b), Err(Error::Invalid));
        let mut b = table(&[(1, true)]);
        b[45] = 0;
        seal(&mut b);
        assert_eq!(parse_madt(&b), Err(Error::Invalid));
        assert_eq!(parse_madt(&b[..43]), Err(Error::Invalid));
    }
}
