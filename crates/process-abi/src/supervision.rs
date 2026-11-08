//! Native owned-child event wire format, independent of Rust structure layout.
/// Return the caller's generation-tagged process identity (other arguments zero).
pub const SYS_SELF: u64 = 35;
/// Consume a child event: writable output pointer, exact byte count, zero.
pub const SYS_CHILD_NEXT: u64 = 36;
/// Terminate an owned child: process key, zero, zero.
pub const SYS_TERMINATE: u64 = 37;
/// Complete event size in bytes, including reserved zero fields.
pub const EVENT_SIZE: usize = 40;
/// Currently supported wire version.
pub const VERSION: u32 = 1;
/// Process identity. PIDs are not reused; generation one is currently mandatory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessKey {
    pub pid: u32,
    pub generation: u32,
}
impl ProcessKey {
    /// Encode identity in a scalar without truncation.
    pub const fn encode(self) -> u64 {
        (self.generation as u64) << 32 | self.pid as u64
    }
    /// Reject unsupported generations and sentinel PIDs.
    pub fn decode(value: u64) -> Option<Self> {
        let key = Self {
            pid: value as u32,
            generation: (value >> 32) as u32,
        };
        (key.generation == 1 && key.pid != u32::MAX).then_some(key)
    }
}
/// Why execution stopped; explicit exit(9) is not a forced termination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Exited,
    Killed,
    Fault,
}
/// A live adopted orphan or a stopped child. Status is exit value or fault vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildEvent {
    Adopted(ProcessKey),
    Completed {
        process: ProcessKey,
        cause: Cause,
        status: u64,
    },
}
impl ChildEvent {
    /// Encode an initialized padding-free little-endian record.
    pub fn encode(self) -> [u8; EVENT_SIZE] {
        let (tag, key, cause, status) = match self {
            Self::Adopted(k) => (1u32, k, 0u32, 0),
            Self::Completed {
                process,
                cause,
                status,
            } => (
                2,
                process,
                match cause {
                    Cause::Exited => 0,
                    Cause::Killed => 1,
                    Cause::Fault => 2,
                },
                status,
            ),
        };
        let mut out = [0; EVENT_SIZE];
        out[0..4].copy_from_slice(&VERSION.to_le_bytes());
        out[4..8].copy_from_slice(&tag.to_le_bytes());
        out[8..16].copy_from_slice(&key.encode().to_le_bytes());
        out[16..20].copy_from_slice(&cause.to_le_bytes());
        out[24..32].copy_from_slice(&status.to_le_bytes());
        out
    }
    /// Validate all tags and reserved bytes before exposing a typed notification.
    pub fn decode(bytes: &[u8; EVENT_SIZE]) -> Option<Self> {
        let word = |offset| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        if word(0) != VERSION || word(20) != 0 || bytes[32..].iter().any(|&x| x != 0) {
            return None;
        }
        let key = ProcessKey::decode(u64::from_le_bytes(bytes[8..16].try_into().ok()?))?;
        let status = u64::from_le_bytes(bytes[24..32].try_into().ok()?);
        match (word(4), word(16)) {
            (1, 0) if status == 0 => Some(Self::Adopted(key)),
            (2, c @ 0..=2) => Some(Self::Completed {
                process: key,
                cause: match c {
                    0 => Cause::Exited,
                    1 => Cause::Killed,
                    _ => Cause::Fault,
                },
                status,
            }),
            _ => None,
        }
    }
}
