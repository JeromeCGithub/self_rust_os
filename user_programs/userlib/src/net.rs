//! Typed, nonblocking process-owned sockets. Protocol and hardware stay in Ring 0.
//! Call poll/recv/send regularly while active; sleep between attempts. Close every
//! successfully opened handle (the kernel also releases handles on exit/kill).
use crate::sys::syscall;
use rustos_net_abi as abi;
pub use rustos_net_abi::{Error, Info, Kind, Packet, CONNECTED, EOF, MAX_DATA, READABLE, WRITABLE};

/// Open a socket of the selected transport. Four per process, sixteen globally.
pub fn open(kind: Kind) -> Result<u64, Error> {
    // SAFETY: Scalar kind is a validated enum and unused arguments are zero.
    abi::decode(unsafe { syscall(abi::SYS_SOCKET, kind as u64, 0, 0) })
}
/// Configure remote IPv4 and port (zero for ICMP). TCP completes asynchronously.
pub fn connect(handle: u64, address: [u8; 4], port: u16) -> Result<(), Error> {
    // SAFETY: Only scalars; kernel checks handle ownership, address and port.
    abi::decode(unsafe { syscall(abi::SYS_CONNECT, handle, abi::endpoint(address, port), 0) })
        .map(|_| ())
}
/// Queue payload. Packet address must be zero; peer was selected by connect.
/// ICMP port is the sequence; UDP/TCP port is zero. TCP can accept a short write.
pub fn send(handle: u64, packet: &Packet) -> Result<usize, Error> {
    if usize::from(packet.len) > MAX_DATA {
        return Err(Error::MessageTooLarge);
    }
    // SAFETY: packet is a fully initialized, readable record that remains live
    // for the synchronous call. Kernel copies it and retains no user reference.
    abi::decode(unsafe {
        syscall(
            abi::SYS_SEND,
            handle,
            packet as *const Packet as u64,
            core::mem::size_of::<Packet>() as u64,
        )
    })
    .map(|n| n as usize)
}
/// Receive one datagram or TCP chunk; WouldBlock means retry after sleeping.
/// A zero-length TCP result means EOF. A zero-length UDP datagram remains valid.
pub fn recv(handle: u64) -> Result<Packet, Error> {
    let mut packet = Packet::empty();
    recv_into(handle, &mut packet)?;
    Ok(packet)
}
/// Receive into reusable caller storage to bound stack use for streaming clients.
/// Only read the record after success; the kernel initializes its complete output.
pub fn recv_into(handle: u64, packet: &mut Packet) -> Result<usize, Error> {
    // SAFETY: The exclusive writable record lives for the call. The kernel checks
    // its complete mapping, copies initialized bytes and retains no reference.
    abi::decode(unsafe {
        syscall(
            abi::SYS_RECV,
            handle,
            packet as *mut Packet as u64,
            core::mem::size_of::<Packet>() as u64,
        )
    })
    .map(|n| n as usize)
}
/// Return readiness while also making bounded protocol/device progress.
pub fn poll(handle: u64) -> Result<u64, Error> {
    // SAFETY: Scalars only; kernel checks ownership of the opaque handle.
    abi::decode(unsafe { syscall(abi::SYS_POLL, handle, 0, 0) })
}
/// Release a socket immediately. TCP close currently discards pending data.
pub fn close(handle: u64) -> Result<(), Error> {
    // SAFETY: Scalar handle; stale/foreign handles are rejected by the kernel.
    abi::decode(unsafe { syscall(abi::SYS_CLOSE, handle, 0, 0) }).map(|_| ())
}
/// Interface presence, link, static addresses and current endpoint usage.
pub fn info() -> Result<Info, Error> {
    let mut info = Info::default();
    // SAFETY: Exclusive initialized padding-free output of the exact ABI size.
    abi::decode(unsafe {
        syscall(
            abi::SYS_INFO,
            &mut info as *mut Info as u64,
            core::mem::size_of::<Info>() as u64,
            0,
        )
    })?;
    if info.version != abi::VERSION {
        return Err(Error::Invalid);
    }
    Ok(info)
}
