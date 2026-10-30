//! Ring 3 ping policy: parsing, cadence, payload matching, deadlines and statistics.
#![no_std]
use rustos_net_abi::{Error, Packet};

/// Validated command options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Numeric unicast IPv4 address.
    pub address: [u8; 4],
    /// Number of probes, from one through sixteen.
    pub count: u16,
}
impl Config {
    /// Parse the full command line, including the word ping.
    pub fn parse(command: &str) -> Result<Self, Error> {
        let mut words = command.split_ascii_whitespace();
        if words.next() != Some("ping") {
            return Err(Error::Invalid);
        }
        let mut address = words.next().ok_or(Error::Invalid)?;
        let mut count = 4;
        if address == "-c" {
            count = words
                .next()
                .ok_or(Error::Invalid)?
                .parse::<u16>()
                .map_err(|_| Error::Invalid)?;
            address = words.next().ok_or(Error::Invalid)?;
        }
        if !(1..=16).contains(&count) || words.next().is_some() {
            return Err(Error::Invalid);
        }
        let address = address
            .parse::<core::net::Ipv4Addr>()
            .map_err(|_| Error::Invalid)?
            .octets();
        if address[0] == 0 || address[0] == 127 || address[0] >= 224 {
            return Err(Error::Invalid);
        }
        Ok(Self { address, count })
    }
}

/// Syscall adapter; tests supply simulated sockets and monotonic PIT ticks.
pub trait Transport {
    /// Open an ICMP endpoint.
    fn open(&mut self) -> Result<u64, Error>;
    /// Configure destination.
    fn connect(&mut self, handle: u64, address: [u8; 4]) -> Result<(), Error>;
    /// Send one echo payload/sequence.
    fn send(&mut self, handle: u64, packet: &Packet) -> Result<usize, Error>;
    /// Receive one reply, without blocking.
    fn recv(&mut self, handle: u64) -> Result<Packet, Error>;
    /// Release an endpoint.
    fn close(&mut self, handle: u64);
}

/// One observable command event. Each step performs bounded work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Validated, previously unseen echo reply.
    Reply { sequence: u16, ticks: u64 },
    /// One probe expired.
    Timeout { sequence: u16 },
    /// Endpoint closed and command complete.
    Done { sent: u16, received: u16 },
    /// Endpoint closed after an error.
    Failed(Error),
}

/// A nonblocking command, independent of rendering and kernel implementation.
pub struct Client {
    config: Config,
    handle: u64,
    nonce: u64,
    sent: u16,
    received: u16,
    resolved: u16,
    times: [u64; 16],
    next_send_base: u64,
    active: bool,
}
impl Client {
    /// Open/connect a client, closing a partially initialized endpoint on failure.
    pub fn start(config: Config, now: u64, transport: &mut impl Transport) -> Result<Self, Error> {
        if !(1..=16).contains(&config.count) {
            return Err(Error::Invalid);
        }
        let handle = transport.open()?;
        if let Err(error) = transport.connect(handle, config.address) {
            transport.close(handle);
            return Err(error);
        }
        Ok(Self {
            config,
            handle,
            nonce: now ^ handle.rotate_left(17),
            sent: 0,
            received: 0,
            resolved: 0,
            times: [0; 16],
            next_send_base: now,
            active: true,
        })
    }
    /// Advance one bounded step.
    pub fn step(&mut self, now: u64, transport: &mut impl Transport) -> Option<Event> {
        if !self.active {
            return None;
        }
        if self.sent == self.config.count && self.resolved.count_ones() == u32::from(self.sent) {
            self.finish(transport);
            return Some(Event::Done {
                sent: self.sent,
                received: self.received,
            });
        }
        match transport.recv(self.handle) {
            Ok(reply) => {
                let sequence = reply.port;
                if reply.address == self.config.address && sequence > 0 && sequence <= self.sent {
                    let bit = 1 << (sequence - 1);
                    if self.resolved & bit == 0
                        && reply.len == 56
                        && reply.data[..56] == self.payload(sequence)
                    {
                        self.resolved |= bit;
                        self.received += 1;
                        return Some(Event::Reply {
                            sequence,
                            ticks: now.wrapping_sub(self.times[sequence as usize - 1]),
                        });
                    }
                }
            }
            Err(Error::WouldBlock) => (),
            Err(error) => {
                self.finish(transport);
                return Some(Event::Failed(error));
            }
        }
        for index in 0..self.sent {
            let bit = 1 << index;
            if self.resolved & bit == 0 && now.wrapping_sub(self.times[index as usize]) >= 55 {
                self.resolved |= bit;
                return Some(Event::Timeout {
                    sequence: index + 1,
                });
            }
        }
        if self.sent < self.config.count
            && (self.sent == 0 || now.wrapping_sub(self.next_send_base) >= 19)
        {
            let mut packet = Packet::empty();
            packet.port = self.sent + 1;
            packet.len = 56;
            packet.data[..56].copy_from_slice(&self.payload(packet.port));
            match transport.send(self.handle, &packet) {
                Ok(56) => {
                    self.times[self.sent as usize] = now;
                    self.sent += 1;
                    self.next_send_base = now;
                }
                Err(Error::WouldBlock) if now.wrapping_sub(self.next_send_base) < 55 => (),
                Err(error) => {
                    self.finish(transport);
                    return Some(Event::Failed(error));
                }
                Ok(_) => {
                    self.finish(transport);
                    return Some(Event::Failed(Error::Invalid));
                }
            }
        }
        None
    }

    fn payload(&self, sequence: u16) -> [u8; 56] {
        let mut bytes = [0; 56];
        bytes[..8].copy_from_slice(&self.nonce.to_le_bytes());
        bytes[8..10].copy_from_slice(&sequence.to_be_bytes());
        for (index, byte) in bytes[10..].iter_mut().enumerate() {
            *byte = index as u8;
        }
        bytes
    }
    fn finish(&mut self, transport: &mut impl Transport) {
        transport.close(self.handle);
        self.active = false;
    }
}
