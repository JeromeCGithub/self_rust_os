//! Native principal descriptions. Decoding bytes never grants kernel authority.
/// Query the calling process's kernel principal; no foreign PID selector exists.
pub const SYS_PRINCIPAL: u64 = 129;
/// Irreversibly reduce the single-threaded caller's ambient mechanism rights.
/// Args: known capability bits, flags=0, reserved=0. Native PID 1 is refused.
pub const SYS_ATTENUATE: u64 = 130;
/// Version-1 explicit little-endian output size, without Rust padding.
pub const SIZE: usize = 40;
/// System scope is reserved by kernel boot; later scopes use fresh larger IDs.
pub const SYSTEM_SCOPE: u64 = 1;
pub const INIT: u32 = 1;
pub const LEGACY: u32 = 2;
/// Malformed, noncanonical or unsupported principal description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;
/// Upper bounds on ambient mechanisms. Scope/handle ownership checks still apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities(u64);
impl Capabilities {
    pub const FILE_READ: Self = Self(1);
    pub const FILE_WRITE: Self = Self(2);
    pub const SYSTEM_WRITE: Self = Self(4);
    pub const IPC: Self = Self(8);
    pub const SPAWN: Self = Self(16);
    pub const THREADS: Self = Self(32);
    pub const NETWORK: Self = Self(64);
    pub const AUDIO: Self = Self(128);
    pub const SCOPE_CREATE: Self = Self(256);
    pub const ALL: Self = Self(511);
    pub const NONE: Self = Self(0);
    pub fn from_bits(bits: u64) -> Result<Self, Invalid> {
        if bits & !Self::ALL.0 != 0 {
            return Err(Invalid);
        }
        Ok(Self(bits))
    }
    pub const fn bits(self) -> u64 {
        self.0
    }
    /// Check the native syscall mechanism domain; object checks remain required.
    /// Factories/ambient file operations require their domain. Explicitly owned
    /// handles, cleanup and private progress retain their existing validation.
    /// Unknown syscalls fail closed, even with ALL; add new operations explicitly.
    pub fn permits(self, number: u64, first: u64, second: u64) -> bool {
        match number {
            2 | 4 => first == 0 || self.contains(Self::FILE_READ),
            5 | 6 | 27 | 33 => self.contains(Self::FILE_READ),
            7 | 12 => self.contains(Self::FILE_WRITE),
            8 | 29 | 30 | 126 | 127 => self.contains(Self::SPAWN),
            23 => {
                // Linux-valued flags only, not a Linux syscall ABI. O_CREAT has
                // a write side effect even with O_RDONLY. TRUNC requires write.
                let access = second & 3;
                if second & !(3 | 0o100 | 0o1000) != 0
                    || access == 3
                    || second & 0o1000 != 0 && access == 0
                {
                    return false;
                }
                let reads = access != 1;
                let writes = access != 0 || second & (0o100 | 0o1000) != 0;
                (!reads || self.contains(Self::FILE_READ))
                    && (!writes || self.contains(Self::FILE_WRITE))
            }
            25 => first == 1 || first == 2 || self.contains(Self::FILE_WRITE),
            64..=67 | 69 | 70 => self.contains(Self::NETWORK),
            82 | 83 => self.contains(Self::AUDIO),
            84 => second == 0 || self.contains(Self::AUDIO),
            98 => self.contains(Self::THREADS),
            120 => self.contains(Self::IPC),
            // Display operations retain their existing PID-0/ownership gates;
            // M6 will supply explicit native session display leases.
            0
            | 1
            | 3
            | 9..=11
            | 13..=22
            | 24
            | 26
            | 28
            | 31..=32
            | 34..=37
            | 68
            | 80..=81
            | 85
            | 96..=97
            | 99..=109
            | 121..=125
            | 128..=130 => true,
            _ => false,
        }
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    /// Explicit grants may select a subset of the source's known rights.
    pub fn attenuate(self, requested: Self) -> Result<Self, Invalid> {
        if requested.0 & !self.0 != 0 {
            return Err(Invalid);
        }
        Ok(requested)
    }
    /// Ordinary spawn never inherits global-system writes or scope factories.
    pub fn ordinary_child(self) -> Self {
        Self(self.0 & !(Self::SYSTEM_WRITE.0 | Self::SCOPE_CREATE.0))
    }
}
/// Snapshot of kernel-owned metadata, never an authentication token or launch grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Principal {
    pub user: u32,
    pub session: u64,
    pub scope: u64,
    pub flags: u32,
    pub capabilities: Capabilities,
}
impl Principal {
    fn validate(self) -> Result<(), Invalid> {
        if self.user == u32::MAX
            || self.scope == 0
            || self.scope == u64::MAX
            || self.session == u64::MAX
            || self.flags & !(INIT | LEGACY) != 0
            || self.flags == (INIT | LEGACY)
            || self.user != 0 && self.session == 0
            || self.scope == SYSTEM_SCOPE && (self.user != 0 || self.session != 0)
            || self.flags != 0
                && (self.user != 0 || self.session != 0 || self.scope != SYSTEM_SCOPE)
        {
            return Err(Invalid);
        }
        Ok(())
    }
    /// Encode only a consistent description. These bytes confer no authority.
    pub fn encode(self) -> Result<[u8; SIZE], Invalid> {
        self.validate()?;
        let mut bytes = [0; SIZE];
        bytes[..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.flags.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.user.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.session.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.scope.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.capabilities.bits().to_le_bytes());
        Ok(bytes)
    }
    /// Decode exact version-1 data with canonical reserved bytes and known rights.
    pub fn decode(bytes: &[u8]) -> Result<Self, Invalid> {
        if bytes.len() != SIZE || bytes[..4] != 1u32.to_le_bytes() || bytes[12..16] != [0; 4] {
            return Err(Invalid);
        }
        let value = Self {
            flags: u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| Invalid)?),
            user: u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| Invalid)?),
            session: u64::from_le_bytes(bytes[16..24].try_into().map_err(|_| Invalid)?),
            scope: u64::from_le_bytes(bytes[24..32].try_into().map_err(|_| Invalid)?),
            capabilities: Capabilities::from_bits(u64::from_le_bytes(
                bytes[32..40].try_into().map_err(|_| Invalid)?,
            ))?,
        };
        value.validate()?;
        Ok(value)
    }
    /// Preserve native user/session/scope while removing init and factory rights.
    /// This is construction policy, not authentication of a caller's description.
    pub fn ordinary_child(self) -> Self {
        Self {
            flags: self.flags & !INIT,
            capabilities: self.capabilities.ordinary_child(),
            ..self
        }
    }
}
