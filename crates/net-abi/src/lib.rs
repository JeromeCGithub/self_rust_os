//! Versioned socket ABI shared by Ring 0 and Ring 3. No hardware or syscall code.
#![no_std]

/// Maximum bytes in one socket operation (stream transfers can be short).
pub const MAX_DATA: usize = 1024;
/// Networking ABI version.
pub const VERSION: u32 = 2;
/// Create an ICMP echo, UDP, or TCP socket.
pub const SYS_SOCKET: u64 = 64;
/// Associate a remote IPv4 address/port; starts an asynchronous TCP handshake.
pub const SYS_CONNECT: u64 = 65;
/// Send a packet or stream chunk.
pub const SYS_SEND: u64 = 66;
/// Receive a packet or stream chunk without blocking.
pub const SYS_RECV: u64 = 67;
/// Release a socket and its queued data.
pub const SYS_CLOSE: u64 = 68;
/// Make bounded network progress and return readiness bits.
pub const SYS_POLL: u64 = 69;
/// Query interface configuration and resource usage.
pub const SYS_INFO: u64 = 70;
/// Data can be read.
pub const READABLE: u64 = 1;
/// Data can be queued for transmission.
pub const WRITABLE: u64 = 2;
/// The connection is established (or a datagram peer is configured).
pub const CONNECTED: u64 = 4;
/// The TCP receive half is closed.
pub const EOF: u64 = 8;

/// Supported transports. The kernel validates raw values before conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Kind {
    /// Restricted ICMPv4 Echo datagrams. Packet port carries the echo sequence.
    Icmp = 1,
    /// Connected UDP datagrams, with an automatically assigned local port.
    Udp = 2,
    /// Outgoing TCP byte stream, with an automatically assigned local port.
    Tcp = 3,
}

impl Kind {
    /// Validate an untrusted socket kind.
    pub fn from_raw(value: u64) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Icmp),
            2 => Ok(Self::Udp),
            3 => Ok(Self::Tcp),
            _ => Err(Error::Invalid),
        }
    }
}

/// Typed network failures; encoded below the existing syscall control sentinels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    /// Invalid argument, record, address, or user buffer.
    Invalid = 1,
    /// No data or queue space yet; retry after sleeping.
    WouldBlock = 2,
    /// No usable Ethernet interface.
    NoDevice = 3,
    /// Unknown, stale, or foreign process handle.
    BadHandle = 4,
    /// Socket quota or handle generation space exhausted.
    NoResources = 5,
    /// No remote peer or connection not established.
    NotConnected = 6,
    /// Payload or datagram exceeds the supported limit.
    MessageTooLarge = 7,
    /// Connection no longer usable.
    Closed = 8,
}

const ERROR_BASE: u64 = u64::MAX - 255;
impl Error {
    /// Encode a failure without colliding with kernel exit/block sentinels.
    pub const fn encode(self) -> u64 {
        ERROR_BASE + self as u64
    }
}

/// Decode a syscall value. The preexisting generic error also maps to Invalid.
pub fn decode(value: u64) -> Result<u64, Error> {
    match value.checked_sub(ERROR_BASE) {
        Some(1) => Err(Error::Invalid),
        Some(2) => Err(Error::WouldBlock),
        Some(3) => Err(Error::NoDevice),
        Some(4) => Err(Error::BadHandle),
        Some(5) => Err(Error::NoResources),
        Some(6) => Err(Error::NotConnected),
        Some(7) => Err(Error::MessageTooLarge),
        Some(8) => Err(Error::Closed),
        Some(_) => Err(Error::Invalid),
        None => Ok(value),
    }
}

/// Fixed size, padding-free record. Metadata integers use x86-64 native endian.
/// On send, address must be zero; connect chooses the peer. Only ICMP uses port
/// as its sequence. On receive address/port identify the peer (ICMP: sequence).
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Packet {
    /// IPv4 octets, in network order.
    pub address: [u8; 4],
    /// UDP source port, ICMP sequence, or zero for TCP.
    pub port: u16,
    /// Number of valid payload bytes.
    pub len: u16,
    /// Payload; bytes beyond len are zero on receive.
    pub data: [u8; MAX_DATA],
}
impl Packet {
    /// Fully initialized empty packet.
    pub const fn empty() -> Self {
        Self {
            address: [0; 4],
            port: 0,
            len: 0,
            data: [0; MAX_DATA],
        }
    }
}

/// Padding-free interface snapshot. State bit 0 means present, bit 1 link up.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Info {
    /// Record version.
    pub version: u32,
    /// Interface state flags.
    pub state: u32,
    /// Static IPv4 address.
    pub address: [u8; 4],
    /// Default gateway.
    pub gateway: [u8; 4],
    /// Hardware address.
    pub mac: [u8; 6],
    /// IPv4 prefix length.
    pub prefix: u8,
    /// Reserved, always zero.
    pub reserved: u8,
    /// IP MTU in bytes.
    pub mtu: u32,
    /// Number of live sockets globally.
    pub open_sockets: u32,
}

/// Pack IPv4 octets and a native port into the connect scalar. High 16 bits zero.
pub const fn endpoint(address: [u8; 4], port: u16) -> u64 {
    ((u32::from_be_bytes(address) as u64) << 16) | port as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_have_no_padding_and_expected_offsets() {
        assert_eq!(core::mem::size_of::<Packet>(), 1032);
        assert_eq!(core::mem::offset_of!(Packet, data), 8);
        assert_eq!(core::mem::size_of::<Info>(), 32);
        assert_eq!(core::mem::offset_of!(Info, mtu), 24);
    }
    #[test]
    fn error_encoding_does_not_alias_control_flow() {
        for error in [
            Error::Invalid,
            Error::WouldBlock,
            Error::NoDevice,
            Error::BadHandle,
            Error::NoResources,
            Error::NotConnected,
            Error::MessageTooLarge,
            Error::Closed,
        ] {
            assert_eq!(decode(error.encode()), Err(error));
            assert!(error.encode() < u64::MAX - 2);
        }
        assert_eq!(decode(u64::MAX - 1), Err(Error::Invalid));
        assert_eq!(decode(0), Ok(0));
        assert_eq!(Kind::from_raw(999), Err(Error::Invalid));
    }
}
