use rustos_net_abi::{Error, Packet};
use rustos_ping::{Client, Config, Event, Transport};
use std::collections::VecDeque;

#[derive(Default)]
struct Fake {
    sent: Vec<Packet>,
    replies: VecDeque<Packet>,
    closed: usize,
    connect_error: bool,
    blocked: bool,
}
impl Transport for Fake {
    fn open(&mut self) -> Result<u64, Error> {
        Ok(77)
    }
    fn connect(&mut self, _: u64, _: [u8; 4]) -> Result<(), Error> {
        if self.connect_error {
            Err(Error::NoDevice)
        } else {
            Ok(())
        }
    }
    fn send(&mut self, _: u64, packet: &Packet) -> Result<usize, Error> {
        if self.blocked {
            return Err(Error::WouldBlock);
        }
        self.sent.push(packet.clone());
        Ok(packet.len as usize)
    }
    fn recv(&mut self, _: u64) -> Result<Packet, Error> {
        self.replies.pop_front().ok_or(Error::WouldBlock)
    }
    fn close(&mut self, _: u64) {
        self.closed += 1;
    }
}
fn config(count: u16) -> Config {
    Config {
        address: [8, 8, 8, 8],
        count,
    }
}

#[test]
fn parses_options_and_rejects_invalid_or_extra_arguments() {
    assert_eq!(Config::parse("ping 8.8.8.8"), Ok(config(4)));
    assert_eq!(Config::parse(" ping  -c 16 8.8.8.8 "), Ok(config(16)));
    for command in [
        "",
        "ping",
        "ping google.com",
        "ping 1.2.3.256",
        "ping 1.2.3",
        "ping -c 0 8.8.8.8",
        "ping -c 17 8.8.8.8",
        "ping 8.8.8.8 extra",
        "ping 224.0.0.1",
        "ping 0.0.0.0",
        "ping 255.255.255.255",
    ] {
        assert_eq!(Config::parse(command), Err(Error::Invalid), "{command}");
    }
}

#[test]
fn matches_payload_ignores_foreign_and_duplicate_and_closes_on_done() {
    let mut io = Fake::default();
    let mut client = Client::start(config(2), 0, &mut io).expect("valid ping should start");
    assert_eq!(client.step(0, &mut io), None);
    assert_eq!(io.sent.len(), 1);
    let mut reply = io.sent[0].clone();
    reply.address = [8; 4];
    let mut wrong = reply.clone();
    wrong.data[0] ^= 1;
    io.replies.push_back(wrong);
    assert_eq!(client.step(1, &mut io), None);
    io.replies.push_back(reply.clone());
    assert_eq!(
        client.step(2, &mut io),
        Some(Event::Reply {
            sequence: 1,
            ticks: 2
        })
    );
    io.replies.push_back(reply);
    assert_eq!(client.step(3, &mut io), None);
    client.step(19, &mut io);
    assert_eq!(io.sent.len(), 2);
    let mut reply = io.sent[1].clone();
    reply.address = [8; 4];
    io.replies.push_back(reply);
    assert_eq!(
        client.step(20, &mut io),
        Some(Event::Reply {
            sequence: 2,
            ticks: 1
        })
    );
    assert_eq!(
        client.step(21, &mut io),
        Some(Event::Done {
            sent: 2,
            received: 2
        })
    );
    assert_eq!(io.closed, 1);
    assert_eq!(client.step(22, &mut io), None);
}

#[test]
fn bounds_timeout_backpressure_and_partial_initialization_cleanup() {
    let mut io = Fake::default();
    let mut client = Client::start(config(1), u64::MAX - 10, &mut io).unwrap();
    client.step(u64::MAX - 10, &mut io);
    assert_eq!(
        client.step(44, &mut io),
        Some(Event::Timeout { sequence: 1 })
    );
    assert_eq!(
        client.step(45, &mut io),
        Some(Event::Done {
            sent: 1,
            received: 0
        })
    );
    assert_eq!(io.closed, 1);
    let mut io = Fake {
        connect_error: true,
        ..Fake::default()
    };
    assert!(Client::start(config(1), 0, &mut io).is_err());
    assert_eq!(io.closed, 1);
    let mut io = Fake {
        blocked: true,
        ..Fake::default()
    };
    let mut client = Client::start(config(1), 0, &mut io).unwrap();
    assert_eq!(client.step(0, &mut io), None);
    assert_eq!(
        client.step(55, &mut io),
        Some(Event::Failed(Error::WouldBlock))
    );
    assert_eq!(io.closed, 1);
}
