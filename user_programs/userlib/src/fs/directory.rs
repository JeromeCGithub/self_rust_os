//! Bounded directory transport/decoding, testable without native syscalls.
extern crate alloc;
use alloc::{string::String, vec::Vec};
/// Directory metadata with its full UTF-8 name.
#[derive(Debug)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
}
pub(crate) fn read_with(
    max_bytes: usize,
    max_entries: usize,
    mut call: impl FnMut(Option<&mut [u8]>) -> Result<usize, ()>,
) -> Result<Vec<DirectoryEntry>, ()> {
    for _ in 0..4 {
        let size = call(None)?;
        if size > max_bytes {
            return Err(());
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| ())?;
        bytes.resize(size, 0);
        let count = match call(Some(&mut bytes)) {
            Ok(count) if count <= size => count,
            Ok(_) => return Err(()),
            Err(()) => continue,
        };
        let mut entries = Vec::new();
        let mut position = 0usize;
        while position < count {
            if entries.len() >= max_entries {
                return Err(());
            }
            let header = bytes
                .get(position..position.checked_add(16).ok_or(())?)
                .ok_or(())?;
            let length = usize::from(u16::from_le_bytes([header[0], header[1]]));
            let name_length = usize::from(u16::from_le_bytes([header[2], header[3]]));
            let end = position.checked_add(length).ok_or(())?;
            if length < 24
                || length % 8 != 0
                || name_length == 0
                || 16 + name_length >= length
                || end > count
            {
                return Err(());
            }
            let raw = &bytes[position + 16..position + 16 + name_length];
            if raw.contains(&0) || bytes[position + 16 + name_length] != 0 {
                return Err(());
            }
            let text = core::str::from_utf8(raw).map_err(|_| ())?;
            let mut name = String::new();
            name.try_reserve_exact(text.len()).map_err(|_| ())?;
            name.push_str(text);
            entries.try_reserve(1).map_err(|_| ())?;
            entries.push(DirectoryEntry {
                name,
                is_directory: header[4] & 16 != 0,
                size: u64::from_le_bytes(header[8..16].try_into().map_err(|_| ())?),
            });
            position = end;
        }
        return Ok(entries);
    }
    Err(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(name: &str) -> Vec<u8> {
        let length = (17 + name.len() + 7) & !7;
        let mut bytes = alloc::vec![0;length];
        bytes[..2].copy_from_slice(&(length as u16).to_le_bytes());
        bytes[2..4].copy_from_slice(&(name.len() as u16).to_le_bytes());
        bytes[4] = 16;
        bytes[8..16].copy_from_slice(&42u64.to_le_bytes());
        bytes[16..16 + name.len()].copy_from_slice(name.as_bytes());
        bytes
    }
    fn read(bytes: &[u8], entries: usize) -> Result<Vec<DirectoryEntry>, ()> {
        read_with(1024, entries, |output| {
            if let Some(output) = output {
                output[..bytes.len()].copy_from_slice(bytes);
            }
            Ok(bytes.len())
        })
    }
    #[test]
    fn budget_is_checked_before_allocating_or_copying() {
        let mut calls = 0;
        assert!(read_with(64, 4, |_| {
            calls += 1;
            Ok(65)
        })
        .is_err());
        assert_eq!(calls, 1);
        assert!(read_with(0, 0, |_| Ok(0)).unwrap().is_empty());
    }
    #[test]
    fn full_names_metadata_and_entry_limit() {
        let bytes = record("friendly-name.UNIT");
        let entries = read(&bytes, 1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "friendly-name.UNIT");
        assert!(entries[0].is_directory);
        assert_eq!(entries[0].size, 42);
        assert!(read(&bytes, 0).is_err());
        let two = [bytes.as_slice(), bytes.as_slice()].concat();
        assert!(read(&two, 1).is_err());
    }
    #[test]
    fn rejects_partial_malformed_records_and_utf8() {
        let good = record("a.UNIT");
        for n in 1..good.len() {
            assert!(read(&good[..n], 2).is_err());
        }
        for (offset, value) in [(0, 0), (1, 1), (2, 255), (16, 255), (22, 1)] {
            let mut bytes = good.clone();
            bytes[offset] = value;
            assert!(read(&bytes, 2).is_err(), "offset {offset}");
        }
    }
    #[test]
    fn size_races_retry_with_a_new_query_but_never_return_partial_lists() {
        let bytes = record("a.UNIT");
        let mut calls = 0;
        let entries = read_with(1024, 2, |output| {
            calls += 1;
            if let Some(output) = output {
                if calls == 2 {
                    return Err(());
                }
                output[..bytes.len()].copy_from_slice(&bytes);
            }
            Ok(bytes.len())
        })
        .unwrap();
        assert_eq!(calls, 4);
        assert_eq!(entries.len(), 1);
        let mut copies = 0;
        assert!(read_with(64, 2, |output| {
            if output.is_some() {
                copies += 1;
                Err(())
            } else {
                Ok(24)
            }
        })
        .is_err());
        assert_eq!(copies, 4);
    }
}
