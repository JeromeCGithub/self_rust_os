//! Ring 3 adapter and bounded terminal formatting for the reusable ping policy.
pub use rustos_ping::{Client, Config, Event};
use userlib::net::{self, Error, Kind, Packet};

pub struct Sockets;
impl rustos_ping::Transport for Sockets {
    fn open(&mut self) -> Result<u64, Error> {
        net::open(Kind::Icmp)
    }
    fn connect(&mut self, handle: u64, address: [u8; 4]) -> Result<(), Error> {
        net::connect(handle, address, 0)
    }
    fn send(&mut self, handle: u64, packet: &Packet) -> Result<usize, Error> {
        net::send(handle, packet)
    }
    fn recv(&mut self, handle: u64) -> Result<Packet, Error> {
        net::recv(handle)
    }
    fn close(&mut self, handle: u64) {
        let _ = net::close(handle);
    }
}

pub fn error(error: Error) -> &'static [u8] {
    match error {
        Error::NoDevice => b"ping: network unavailable",
        Error::NoResources => b"ping: socket limit reached",
        Error::WouldBlock => b"ping: transmit queue timed out",
        _ => b"ping: network operation failed",
    }
}
