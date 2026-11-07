//! Canonical copied standard-I/O words: one to three binary bytes, no pointers.
pub(super) struct Packet {
    bytes: [u8; 3],
    length: usize,
}
impl Packet {
    pub(super) fn from_bytes(source: &[u8]) -> Option<Self> {
        if !(1..=3).contains(&source.len()) {
            return None;
        }
        let mut bytes = [0; 3];
        bytes[..source.len()].copy_from_slice(source);
        Some(Self {
            bytes,
            length: source.len(),
        })
    }
    pub(super) fn from_word(word: u32) -> Option<Self> {
        let length = (word >> 24) as usize;
        if !(1..=3).contains(&length) {
            return None;
        }
        let bytes = [word as u8, (word >> 8) as u8, (word >> 16) as u8];
        if bytes[length..].iter().any(|byte| *byte != 0) {
            return None;
        }
        Some(Self { bytes, length })
    }
    pub(super) fn word(&self) -> u32 {
        ((self.length as u32) << 24)
            | u32::from(self.bytes[0])
            | (u32::from(self.bytes[1]) << 8)
            | (u32::from(self.bytes[2]) << 16)
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
    pub(super) fn parts(&self) -> ([u8; 3], usize) {
        (self.bytes, self.length)
    }
}
