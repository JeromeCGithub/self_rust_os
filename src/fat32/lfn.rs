//! VFAT UTF-16 entry encoding and strict sequence/checksum decoding.
extern crate alloc;
#[cfg(all(test, not(target_os = "none")))]
use alloc::string::String;
use alloc::vec::Vec;
const POSITIONS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
pub(crate) fn valid(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with([' ', '.'])
        && name.encode_utf16().count() <= 255
        && !name.chars().any(|c| {
            c < ' '
                || c == '\u{ffff}'
                || matches!(
                    c,
                    '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|' | '\0'
                )
        })
}
pub(crate) fn checksum(name: &[u8; 11]) -> u8 {
    name.iter()
        .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte))
}
pub(crate) fn encode(name: &str, alias: &[u8; 11]) -> Result<Vec<[u8; 32]>, ()> {
    if !valid(name) {
        return Err(());
    }
    let units: Vec<u16> = name.encode_utf16().collect();
    let count = units.len().div_ceil(13);
    let mut result = Vec::new();
    result.try_reserve_exact(count).map_err(|_| ())?;
    for order in (1..=count).rev() {
        let mut entry = [0; 32];
        entry[0] = order as u8 | if order == count { 0x40 } else { 0 };
        entry[11] = 15;
        entry[13] = checksum(alias);
        for (index, position) in POSITIONS.iter().enumerate() {
            let offset = (order - 1) * 13 + index;
            let value = if offset < units.len() {
                units[offset]
            } else if offset == units.len() {
                0
            } else {
                0xffff
            };
            entry[*position..*position + 2].copy_from_slice(&value.to_le_bytes());
        }
        result.push(entry);
    }
    Ok(result)
}
pub(crate) struct Decoder {
    units: [u16; 260],
    remaining: u8,
    count: usize,
    sum: u8,
    active: bool,
}
impl Decoder {
    pub(crate) fn new() -> Self {
        Self {
            units: [0xffff; 260],
            remaining: 0,
            count: 0,
            sum: 0,
            active: false,
        }
    }
    pub(crate) fn reset(&mut self) {
        self.active = false;
        self.remaining = 0;
    }
    pub(crate) fn feed(&mut self, entry: &[u8; 32]) -> bool {
        let order = entry[0] & 31;
        if order == 0
            || order > 20
            || entry[0] & 0xa0 != 0
            || entry[11] != 15
            || entry[12] != 0
            || entry[26..28] != [0, 0]
        {
            self.reset();
            return false;
        }
        if entry[0] & 0x40 != 0 {
            self.units.fill(0xffff);
            self.remaining = order;
            self.count = order as usize;
            self.sum = entry[13];
            self.active = true;
        }
        if !self.active || self.remaining != order || self.sum != entry[13] {
            self.reset();
            return false;
        }
        for (index, position) in POSITIONS.iter().enumerate() {
            self.units[(order as usize - 1) * 13 + index] =
                u16::from_le_bytes([entry[*position], entry[*position + 1]]);
        }
        self.remaining -= 1;
        true
    }
    /// Decode into caller-owned bounded UTF-8 storage; no heap allocation.
    pub(crate) fn finish_into<'a>(
        &mut self,
        alias: &[u8; 11],
        output: &'a mut [u8; 765],
    ) -> Option<&'a str> {
        let complete = self.active && self.remaining == 0 && self.sum == checksum(alias);
        self.active = false;
        if !complete {
            return None;
        }
        let units = &self.units[..self.count * 13];
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        if end == 0
            || end > 255
            || (end < units.len() && units[end + 1..].iter().any(|&u| u != 0xffff))
        {
            return None;
        }
        let mut length = 0;
        for character in char::decode_utf16(units[..end].iter().copied()) {
            let character = character.ok()?;
            length += character.encode_utf8(&mut output[length..]).len();
        }
        let name = core::str::from_utf8(&output[..length]).ok()?;
        if valid(name) {
            Some(name)
        } else {
            None
        }
    }
    #[cfg(all(test, not(target_os = "none")))]
    pub(crate) fn finish(&mut self, alias: &[u8; 11]) -> Option<String> {
        self.finish_into(alias, &mut [0; 765]).map(String::from)
    }
}
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    #[test]
    fn unicode_round_trip_checks_sequence_and_checksum() {
        let alias = *b"LONGNA~1TXT";
        let records = encode("A long café 文書 filename.txt", &alias).unwrap();
        assert!(records.len() > 1);
        let mut decoder = Decoder::new();
        for entry in &records {
            assert!(decoder.feed(entry));
        }
        assert_eq!(
            decoder.finish(&alias).as_deref(),
            Some("A long café 文書 filename.txt")
        );
        let mut invalid = records.clone();
        invalid[0][13] ^= 1;
        for entry in &invalid {
            decoder.feed(entry);
        }
        assert_eq!(decoder.finish(&alias), None);
        decoder.reset();
        for entry in records.iter().rev() {
            decoder.feed(entry);
        }
        assert_eq!(decoder.finish(&alias), None);
    }
    #[test]
    fn maximum_names_fill_bounded_utf8_storage_and_reject_bad_padding() {
        let alias = *b"LONGNA~1TXT";
        for name in ["文".repeat(255), alloc::format!("{}é", "🦀".repeat(127))] {
            let entries = encode(&name, &alias).unwrap();
            assert_eq!(entries.len(), 20);
            let mut decoder = Decoder::new();
            for entry in &entries {
                assert!(decoder.feed(entry));
            }
            let mut output = [0; 765];
            assert_eq!(
                decoder.finish_into(&alias, &mut output),
                Some(name.as_str())
            );
            assert_eq!(decoder.finish_into(&alias, &mut output), None);
        }
        let mut entry = encode("name", &alias).unwrap()[0];
        // The name's terminator is at slot four. All later UTF-16 units must
        // be padding, even when the altered bytes form valid Unicode.
        entry[POSITIONS[5]..POSITIONS[5] + 2].copy_from_slice(&65u16.to_le_bytes());
        let mut decoder = Decoder::new();
        assert!(decoder.feed(&entry));
        assert_eq!(decoder.finish_into(&alias, &mut [0; 765]), None);
    }
    #[test]
    fn boundaries_and_malformed_utf16_are_bounded() {
        let alias = *b"LONGNA~1TXT";
        for size in [1, 13, 26, 255] {
            let name = "a".repeat(size);
            let entries = encode(&name, &alias).unwrap();
            let mut decoder = Decoder::new();
            for entry in &entries {
                decoder.feed(entry);
            }
            assert_eq!(decoder.finish(&alias), Some(name));
        }
        assert!(encode(&"a".repeat(256), &alias).is_err());
        for name in [
            "",
            ".",
            "..",
            "bad/name",
            "bad\\name",
            "bad*name",
            "trailing.",
        ] {
            assert!(!valid(name));
        }
        let mut entry = encode("name", &alias).unwrap()[0];
        entry[1..3].copy_from_slice(&0xd800u16.to_le_bytes());
        let mut decoder = Decoder::new();
        decoder.feed(&entry);
        assert_eq!(decoder.finish(&alias), None);
    }
}
