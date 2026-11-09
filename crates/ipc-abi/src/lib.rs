//! Native bounded channel ABI. Encodings are explicitly little endian.
#![no_std]
pub use rustos_process_abi::supervision::ProcessKey;
/// Channel syscalls append to the native table without changing legacy operations.
pub const SYS_PAIR: u64 = 120;
pub const SYS_SEND: u64 = 121;
pub const SYS_NEXT: u64 = 122;
pub const SYS_CLOSE: u64 = 123;
pub const SYS_INTEREST: u64 = 124;
pub const SYS_BOOTSTRAP: u64 = 125;
pub const SYS_SPAWN_BOUND: u64 = 126;
/// Nonblocking send found the peer's receive queue full.
pub const SEND_FULL: u64 = 2;
/// Nonblocking send found its peer endpoint closed.
pub const SEND_CLOSED: u64 = 3;
/// Complete interest mask: receive readiness and optional one-shot writable signal.
pub const INTEREST_READ: u64 = 1;
pub const INTEREST_WRITE: u64 = 2;
pub const INTEREST_ALL: u64 = 3;
pub const MAX_PAYLOAD: usize = 256;
pub const MAX_MESSAGES: usize = 8;
pub const MAX_CHANNELS: usize = 128;
pub const MAX_HANDLES: usize = 128;
pub const MAX_BINDINGS: usize = 8;
pub const EVENT_SIZE: usize = 32 + MAX_PAYLOAD;
pub const BINDING_SIZE: usize = 24;
pub const SPAWN_SIZE: usize = 64;
/// Opaque IDs remain below every syscall scheduling/error sentinel.
pub const MAX_ID: u64 = i64::MAX as u64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub u64);
impl Handle {
    pub const fn valid(self) -> bool {
        self.0 != 0 && self.0 <= MAX_ID
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(u32);
impl Rights {
    pub const SEND: Self = Self(1);
    pub const RECEIVE: Self = Self(2);
    pub const TRANSFER: Self = Self(4);
    pub const ALL: Self = Self(7);
    pub fn from_bits(bits: u32) -> Result<Self, Invalid> {
        if bits != 0 && bits & !Self::ALL.0 == 0 {
            Ok(Self(bits))
        } else {
            Err(Invalid)
        }
    }
    pub const fn bits(self) -> u32 {
        self.0
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Message,
    PeerClosed,
    Writable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub endpoint: Handle,
    pub kind: Kind,
    pub sender: Option<ProcessKey>,
    pub length: usize,
    pub payload: [u8; MAX_PAYLOAD],
}
impl Event {
    /// Construct a canonical owned copy; the kernel supplies sender identity.
    pub fn message(endpoint: Handle, sender: ProcessKey, bytes: &[u8]) -> Result<Self, Invalid> {
        if !endpoint.valid()
            || bytes.len() > MAX_PAYLOAD
            || ProcessKey::decode(sender.encode()) != Some(sender)
        {
            return Err(Invalid);
        }
        let mut payload = [0; MAX_PAYLOAD];
        payload[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            endpoint,
            kind: Kind::Message,
            sender: Some(sender),
            length: bytes.len(),
            payload,
        })
    }
    /// Signals carry neither a forged peer identity nor message bytes.
    pub fn signal(endpoint: Handle, kind: Kind) -> Result<Self, Invalid> {
        if !endpoint.valid() || kind == Kind::Message {
            return Err(Invalid);
        }
        Ok(Self {
            endpoint,
            kind,
            sender: None,
            length: 0,
            payload: [0; MAX_PAYLOAD],
        })
    }
    pub fn encode(&self) -> Result<[u8; EVENT_SIZE], Invalid> {
        if !self.endpoint.valid()
            || self.length > MAX_PAYLOAD
            || self.payload[self.length..].iter().any(|&b| b != 0)
        {
            return Err(Invalid);
        }
        let sender = match (self.kind, self.sender) {
            (Kind::Message, Some(key)) if ProcessKey::decode(key.encode()) == Some(key) => {
                key.encode()
            }
            (Kind::PeerClosed | Kind::Writable, None) if self.length == 0 => 0,
            _ => return Err(Invalid),
        };
        let tag: u32 = match self.kind {
            Kind::Message => 1,
            Kind::PeerClosed => 2,
            Kind::Writable => 3,
        };
        let mut out = [0; EVENT_SIZE];
        out[..4].copy_from_slice(&1u32.to_le_bytes());
        out[4..8].copy_from_slice(&tag.to_le_bytes());
        out[8..16].copy_from_slice(&self.endpoint.0.to_le_bytes());
        out[16..24].copy_from_slice(&sender.to_le_bytes());
        out[24..28].copy_from_slice(&(self.length as u32).to_le_bytes());
        out[32..32 + self.length].copy_from_slice(&self.payload[..self.length]);
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Invalid> {
        if bytes.len() != EVENT_SIZE || word32(bytes, 0)? != 1 || word32(bytes, 28)? != 0 {
            return Err(Invalid);
        }
        let endpoint = Handle(word64(bytes, 8)?);
        let sender = word64(bytes, 16)?;
        let length = word32(bytes, 24)? as usize;
        if length > MAX_PAYLOAD || bytes[32 + length..].iter().any(|&b| b != 0) {
            return Err(Invalid);
        }
        match word32(bytes, 4)? {
            1 => Self::message(
                endpoint,
                ProcessKey::decode(sender).ok_or(Invalid)?,
                &bytes[32..32 + length],
            ),
            tag @ 2..=3 if sender == 0 && length == 0 => Self::signal(
                endpoint,
                if tag == 2 {
                    Kind::PeerClosed
                } else {
                    Kind::Writable
                },
            ),
            _ => Err(Invalid),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub source: Handle,
    pub tag: u32,
    pub rights: Rights,
}
impl Binding {
    /// One moved endpoint, a child bootstrap tag and attenuated rights.
    pub fn encode(self) -> Result<[u8; BINDING_SIZE], Invalid> {
        if !self.source.valid() || self.tag == 0 {
            return Err(Invalid);
        }
        Rights::from_bits(self.rights.bits())?;
        let mut out = [0; BINDING_SIZE];
        out[..8].copy_from_slice(&self.source.0.to_le_bytes());
        out[8..12].copy_from_slice(&self.tag.to_le_bytes());
        out[12..16].copy_from_slice(&self.rights.bits().to_le_bytes());
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Invalid> {
        if bytes.len() != BINDING_SIZE || word64(bytes, 16)? != 0 {
            return Err(Invalid);
        }
        let binding = Self {
            source: Handle(word64(bytes, 0)?),
            tag: word32(bytes, 8)?,
            rights: Rights::from_bits(word32(bytes, 12)?)?,
        };
        if !binding.source.valid() || binding.tag == 0 {
            return Err(Invalid);
        }
        Ok(binding)
    }
}
fn word32(bytes: &[u8], offset: usize) -> Result<u32, Invalid> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Invalid)?
            .try_into()
            .map_err(|_| Invalid)?,
    ))
}
fn word64(bytes: &[u8], offset: usize) -> Result<u64, Invalid> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(Invalid)?
            .try_into()
            .map_err(|_| Invalid)?,
    ))
}

/// Version-two launch frame for the new bound-spawn syscall only. Version one
/// and connected stdio remain unchanged; v2 currently has zero flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spawn {
    pub argv: u64,
    pub argc: u64,
    pub envp: u64,
    pub envc: u64,
    pub bindings: u64,
    pub binding_count: u64,
}
impl Spawn {
    fn validate(self) -> Result<(), Invalid> {
        if self.argc > rustos_process_abi::MAX_VECTOR as u64
            || self.envc > rustos_process_abi::MAX_VECTOR as u64
            || self.binding_count > MAX_BINDINGS as u64
        {
            return Err(Invalid);
        }
        Ok(())
    }
    pub fn encode(self) -> Result<[u8; SPAWN_SIZE], Invalid> {
        self.validate()?;
        let fields = [
            2,
            0,
            self.argv,
            self.argc,
            self.envp,
            self.envc,
            self.bindings,
            self.binding_count,
        ];
        let mut bytes = [0; SPAWN_SIZE];
        for (field, out) in fields.iter().zip(bytes.chunks_exact_mut(8)) {
            out.copy_from_slice(&field.to_le_bytes());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Invalid> {
        if bytes.len() != SPAWN_SIZE || word64(bytes, 0)? != 2 || word64(bytes, 8)? != 0 {
            return Err(Invalid);
        }
        let request = Self {
            argv: word64(bytes, 16)?,
            argc: word64(bytes, 24)?,
            envp: word64(bytes, 32)?,
            envc: word64(bytes, 40)?,
            bindings: word64(bytes, 48)?,
            binding_count: word64(bytes, 56)?,
        };
        request.validate()?;
        Ok(request)
    }
}
