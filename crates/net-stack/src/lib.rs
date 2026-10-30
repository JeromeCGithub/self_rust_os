//! Hardware-independent kernel network service. Ethernet is supplied as a Device.
#![no_std]
extern crate alloc;
use alloc::vec::Vec;
use rustos_net_abi::{Error, Kind, Packet, CONNECTED, EOF, MAX_DATA, READABLE, WRITABLE};
use smoltcp::{
    iface::{Config, Interface, PollIngressSingleResult, SocketHandle, SocketSet},
    phy::{ChecksumCapabilities, Device},
    socket::{icmp, tcp, udp},
    time::{Duration, Instant},
    wire::{EthernetAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpEndpoint, Ipv4Address},
};

const SLOTS: usize = 16;
const PER_PROCESS: usize = 4;
// Enough receive credit for a burst of full Ethernet frames, within the
// e1000's usable DMA ring. Allocate fallibly only when a TCP socket opens.
const TCP_RX_BYTES: usize = 32 * 1024;

fn buffer<T: Clone>(value: T, len: usize) -> Result<Vec<T>, Error> {
    let mut data = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|_| Error::NoResources)?;
    data.resize(len, value);
    Ok(data)
}

struct Slot {
    owner: u32,
    generation: u32,
    socket: SocketHandle,
    kind: Kind,
    peer: Option<IpEndpoint>,
    ident: u16,
    local_port: u16,
}

/// Protocol service with bounded progress and process-owned sockets.
/// No IRQ or scheduler calls; the caller serializes access in syscall context.
pub struct Network<D: Device> {
    device: D,
    iface: Interface,
    sockets: SocketSet<'static>,
    slots: [Option<Slot>; SLOTS],
    generation: u32,
}

impl<D: Device> Network<D> {
    /// Configure the initial static QEMU IPv4 profile. Fixed table capacities
    /// always accommodate this single address and default route.
    pub fn new(mut device: D, mac: [u8; 6]) -> Self {
        let mut config = Config::new(EthernetAddress(mac).into());
        // This seed is for protocol state, not cryptographic entropy or TLS.
        config.random_seed =
            u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 13, 37]);
        let mut iface = Interface::new(config, &mut device, Instant::from_millis(0));
        iface.update_ip_addrs(|addresses| {
            assert!(
                addresses
                    .push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24))
                    .is_ok(),
                "one address fits interface table"
            );
        });
        assert!(
            iface
                .routes_mut()
                .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
                .is_ok(),
            "one route fits routing table"
        );
        Self {
            device,
            iface,
            sockets: SocketSet::new(Vec::with_capacity(SLOTS)),
            slots: core::array::from_fn(|_| None),
            generation: 0,
        }
    }

    fn index(&self, pid: u32, handle: u64) -> Result<usize, Error> {
        let index = (handle & 255).checked_sub(1).ok_or(Error::BadHandle)? as usize;
        let slot = self
            .slots
            .get(index)
            .and_then(Option::as_ref)
            .ok_or(Error::BadHandle)?;
        if slot.owner != pid || u64::from(slot.generation) != handle >> 8 {
            return Err(Error::BadHandle);
        }
        Ok(index)
    }

    /// Open a transport endpoint. At most four per PID and sixteen globally.
    pub fn open(&mut self, pid: u32, kind: Kind) -> Result<u64, Error> {
        if self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.owner == pid)
            .count()
            >= PER_PROCESS
        {
            return Err(Error::NoResources);
        }
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(Error::NoResources)?;
        self.generation = self.generation.checked_add(1).ok_or(Error::NoResources)?;
        let generation = self.generation;
        // Low four bits distinguish every live slot even after identifier wrap.
        let ident = (generation as u16)
            .wrapping_mul(16)
            .wrapping_add(index as u16);
        let mut local_port = 49152 + (generation % 16384) as u16;
        // Ports remain unique among all live endpoints, including across wrap.
        for _ in 0..SLOTS {
            if !self
                .slots
                .iter()
                .flatten()
                .any(|s| s.local_port == local_port)
            {
                break;
            }
            local_port = if local_port == 65535 {
                49152
            } else {
                local_port + 1
            };
        }
        let socket = match kind {
            Kind::Icmp => {
                let rx = icmp::PacketBuffer::new(
                    buffer(icmp::PacketMetadata::EMPTY, 4)?,
                    buffer(0, 512)?,
                );
                let tx = icmp::PacketBuffer::new(
                    buffer(icmp::PacketMetadata::EMPTY, 4)?,
                    buffer(0, 512)?,
                );
                let mut socket = icmp::Socket::new(rx, tx);
                socket
                    .bind(icmp::Endpoint::Ident(ident))
                    .map_err(|_| Error::Invalid)?;
                self.sockets.add(socket)
            }
            Kind::Udp => {
                let rx = udp::PacketBuffer::new(
                    buffer(udp::PacketMetadata::EMPTY, 4)?,
                    buffer(0, 4096)?,
                );
                let tx = udp::PacketBuffer::new(
                    buffer(udp::PacketMetadata::EMPTY, 4)?,
                    buffer(0, 4096)?,
                );
                let mut socket = udp::Socket::new(rx, tx);
                socket.bind(local_port).map_err(|_| Error::NoResources)?;
                self.sockets.add(socket)
            }
            Kind::Tcp => {
                let mut socket = tcp::Socket::new(
                    tcp::SocketBuffer::new(buffer(0, TCP_RX_BYTES)?),
                    tcp::SocketBuffer::new(buffer(0, 4096)?),
                );
                socket.set_timeout(Some(Duration::from_secs(10)));
                self.sockets.add(socket)
            }
        };
        self.slots[index] = Some(Slot {
            owner: pid,
            generation,
            socket,
            kind,
            peer: None,
            ident,
            local_port,
        });
        Ok((u64::from(generation) << 8) | (index as u64 + 1))
    }

    /// Configure a unicast peer. TCP begins an asynchronous handshake; poll for
    /// CONNECTED before sending. Datagram sockets cannot be rebound accidentally.
    pub fn connect(
        &mut self,
        pid: u32,
        handle: u64,
        address: [u8; 4],
        port: u16,
    ) -> Result<(), Error> {
        let index = self.index(pid, handle)?;
        let slot = self.slots[index].as_mut().ok_or(Error::BadHandle)?;
        if address[0] == 0
            || address[0] == 127
            || address[0] >= 224
            || address == [10, 0, 2, 255]
            || address == [10, 0, 2, 0]
            || address == [10, 0, 2, 15]
        {
            return Err(Error::Invalid);
        }
        if (slot.kind == Kind::Icmp) != (port == 0) || slot.peer.is_some() {
            return Err(Error::Invalid);
        }
        let peer = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(address)), port);
        if slot.kind == Kind::Tcp {
            self.sockets
                .get_mut::<tcp::Socket>(slot.socket)
                .connect(self.iface.context(), peer, slot.local_port)
                .map_err(|_| Error::Invalid)?;
        }
        slot.peer = Some(peer);
        Ok(())
    }

    /// Queue payload, returning accepted bytes. Stream writes may be short.
    pub fn send(&mut self, pid: u32, handle: u64, packet: &Packet) -> Result<usize, Error> {
        let index = self.index(pid, handle)?;
        let slot = self.slots[index].as_ref().ok_or(Error::BadHandle)?;
        let peer = slot.peer.ok_or(Error::NotConnected)?;
        let len = usize::from(packet.len);
        if len > MAX_DATA || (slot.kind == Kind::Icmp && len > 64) {
            return Err(Error::MessageTooLarge);
        }
        if packet.address != [0; 4] || (slot.kind != Kind::Icmp && packet.port != 0) {
            return Err(Error::Invalid);
        }
        match slot.kind {
            Kind::Icmp => {
                let repr = Icmpv4Repr::EchoRequest {
                    ident: slot.ident,
                    seq_no: packet.port,
                    data: &packet.data[..len],
                };
                let bytes = self
                    .sockets
                    .get_mut::<icmp::Socket>(slot.socket)
                    .send(repr.buffer_len(), peer.addr)
                    .map_err(|_| Error::WouldBlock)?;
                repr.emit(
                    &mut Icmpv4Packet::new_unchecked(bytes),
                    &ChecksumCapabilities::default(),
                );
                Ok(len)
            }
            Kind::Udp => {
                self.sockets
                    .get_mut::<udp::Socket>(slot.socket)
                    .send_slice(&packet.data[..len], peer)
                    .map_err(|_| Error::WouldBlock)?;
                Ok(len)
            }
            Kind::Tcp => {
                let socket = self.sockets.get_mut::<tcp::Socket>(slot.socket);
                if !socket.may_send() {
                    return Err(Error::NotConnected);
                }
                if len != 0 && !socket.can_send() {
                    return Err(Error::WouldBlock);
                }
                socket
                    .send_slice(&packet.data[..len])
                    .map_err(|_| Error::Closed)
            }
        }
    }

    /// Receive payload into an owned initialized packet. Only connected peers
    /// are delivered; draining foreign traffic is bounded to four datagrams.
    pub fn recv(&mut self, pid: u32, handle: u64) -> Result<Packet, Error> {
        let index = self.index(pid, handle)?;
        let slot = self.slots[index].as_ref().ok_or(Error::BadHandle)?;
        let peer = slot.peer.ok_or(Error::NotConnected)?;
        let mut result = Packet::empty();
        match slot.kind {
            Kind::Icmp => {
                let socket = self.sockets.get_mut::<icmp::Socket>(slot.socket);
                for _ in 0..4 {
                    let (bytes, source) = socket.recv().map_err(|_| Error::WouldBlock)?;
                    if source != peer.addr {
                        continue;
                    }
                    let packet = match Icmpv4Packet::new_checked(bytes) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    if let Ok(Icmpv4Repr::EchoReply {
                        ident,
                        seq_no,
                        data,
                    }) = Icmpv4Repr::parse(&packet, &ChecksumCapabilities::default())
                    {
                        if ident != slot.ident || data.len() > 64 {
                            continue;
                        }
                        result.port = seq_no;
                        result.len = data.len() as u16;
                        result.data[..data.len()].copy_from_slice(data);
                        let IpAddress::Ipv4(ip) = source;
                        result.address = ip.octets();
                        return Ok(result);
                    }
                }
                Err(Error::WouldBlock)
            }
            Kind::Udp => {
                let socket = self.sockets.get_mut::<udp::Socket>(slot.socket);
                for _ in 0..4 {
                    let (data, meta) = socket.recv().map_err(|_| Error::WouldBlock)?;
                    if meta.endpoint != peer {
                        continue;
                    }
                    if data.len() > MAX_DATA {
                        return Err(Error::MessageTooLarge);
                    }
                    result.len = data.len() as u16;
                    result.port = meta.endpoint.port;
                    let IpAddress::Ipv4(ip) = meta.endpoint.addr;
                    result.address = ip.octets();
                    result.data[..data.len()].copy_from_slice(data);
                    return Ok(result);
                }
                Err(Error::WouldBlock)
            }
            Kind::Tcp => {
                let socket = self.sockets.get_mut::<tcp::Socket>(slot.socket);
                if !socket.can_recv() {
                    return if socket.state() == tcp::State::SynSent {
                        Err(Error::WouldBlock)
                    } else if !socket.may_recv() {
                        Ok(result)
                    } else {
                        Err(Error::WouldBlock)
                    };
                }
                let len = socket
                    .recv_slice(&mut result.data)
                    .map_err(|_| Error::Closed)?;
                result.len = len as u16;
                let IpAddress::Ipv4(ip) = peer.addr;
                result.address = ip.octets();
                Ok(result)
            }
        }
    }

    /// Readiness snapshot after the caller has run progress.
    pub fn poll(&self, pid: u32, handle: u64) -> Result<u64, Error> {
        let index = self.index(pid, handle)?;
        let slot = self.slots[index].as_ref().ok_or(Error::BadHandle)?;
        let (readable, writable, connected, eof) = match slot.kind {
            Kind::Icmp => {
                let s = self.sockets.get::<icmp::Socket>(slot.socket);
                (s.can_recv(), s.can_send(), slot.peer.is_some(), false)
            }
            Kind::Udp => {
                let s = self.sockets.get::<udp::Socket>(slot.socket);
                (s.can_recv(), s.can_send(), slot.peer.is_some(), false)
            }
            Kind::Tcp => {
                let s = self.sockets.get::<tcp::Socket>(slot.socket);
                (
                    s.can_recv(),
                    s.can_send(),
                    s.state() == tcp::State::Established,
                    slot.peer.is_some() && s.state() != tcp::State::SynSent && !s.may_recv(),
                )
            }
        };
        Ok(if readable { READABLE } else { 0 }
            | if writable { WRITABLE } else { 0 }
            | if connected { CONNECTED } else { 0 }
            | if eof { EOF } else { 0 })
    }

    /// Close an endpoint immediately, dropping queues. TCP connection
    /// state is discarded immediately; a graceful FIN API is not exposed yet.
    pub fn close(&mut self, pid: u32, handle: u64) -> Result<(), Error> {
        let index = self.index(pid, handle)?;
        let slot = self.slots[index].take().ok_or(Error::BadHandle)?;
        if slot.kind == Kind::Tcp {
            self.sockets.get_mut::<tcp::Socket>(slot.socket).abort();
        }
        self.sockets.remove(slot.socket);
        Ok(())
    }

    /// Drop every endpoint owned by a dead process without allocating.
    pub fn cleanup(&mut self, pid: u32) {
        for index in 0..SLOTS {
            if self.slots[index].as_ref().is_some_and(|s| s.owner == pid) {
                if let Some(slot) = self.slots[index].take() {
                    self.sockets.remove(slot.socket);
                }
            }
        }
    }

    /// Service at most eight received frames and one bounded egress pass.
    pub fn progress(&mut self, millis: i64) {
        let now = Instant::from_millis(millis.max(0));
        for _ in 0..8 {
            if self
                .iface
                .poll_ingress_single(now, &mut self.device, &mut self.sockets)
                == PollIngressSingleResult::None
            {
                break;
            }
        }
        self.iface
            .poll_egress(now, &mut self.device, &mut self.sockets);
    }

    /// Read-only access to device status; protocol state remains encapsulated.
    pub fn device(&self) -> &D {
        &self.device
    }

    /// Number of live endpoints.
    pub fn count(&self) -> usize {
        self.slots.iter().flatten().count()
    }
}
