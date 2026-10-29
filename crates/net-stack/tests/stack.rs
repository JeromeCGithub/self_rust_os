use rustos_net_abi::{Error, Kind, Packet};
use rustos_net_stack::Network;
use smoltcp::{
    phy::{self, Device, DeviceCapabilities, Medium},
    time::Instant,
};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

#[derive(Default)]
struct Queues {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}
#[derive(Clone, Default)]
struct Mock(Rc<RefCell<Queues>>);
struct Rx(Vec<u8>);
struct Tx(Mock);
impl phy::RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
impl phy::TxToken for Tx {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut bytes = vec![0; len];
        let result = f(&mut bytes);
        self.0 .0.borrow_mut().tx.push_back(bytes);
        result
    }
}
impl Device for Mock {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx)> {
        let bytes = self.0.borrow_mut().rx.pop_front()?;
        Some((Rx(bytes), Tx(self.clone())))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx> {
        Some(Tx(self.clone()))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

#[test]
fn quotas_foreign_stale_handles_and_cleanup() {
    let mut net = Network::new(Mock::default(), [2, 1, 2, 3, 4, 5]);
    let handle = net.open(11, Kind::Icmp).expect("first socket must open");
    assert_eq!(net.close(12, handle), Err(Error::BadHandle));
    net.close(11, handle).unwrap();
    let newer = net.open(11, Kind::Icmp).unwrap();
    assert_ne!(handle, newer);
    assert_eq!(net.close(11, handle), Err(Error::BadHandle));
    for _ in 0..3 {
        net.open(11, Kind::Udp).unwrap();
    }
    assert_eq!(net.open(11, Kind::Tcp), Err(Error::NoResources));
    net.cleanup(11);
    assert_eq!(net.count(), 0);
    for _ in 0..64 {
        let h = net.open(11, Kind::Icmp).unwrap();
        net.close(11, h).unwrap();
    }
    assert_eq!(net.count(), 0);
}

#[test]
fn validates_destinations_payloads_and_unconnected_operations() {
    let mut net = Network::new(Mock::default(), [2, 1, 2, 3, 4, 5]);
    let h = net.open(1, Kind::Icmp).unwrap();
    assert_eq!(net.send(1, h, &Packet::empty()), Err(Error::NotConnected));
    for ip in [[0, 0, 0, 0], [224, 0, 0, 1], [255; 4], [10, 0, 2, 255]] {
        assert_eq!(net.connect(1, h, ip, 0), Err(Error::Invalid));
    }
    net.connect(1, h, [8, 8, 8, 8], 0).unwrap();
    let mut packet = Packet::empty();
    packet.len = 65;
    assert_eq!(net.send(1, h, &packet), Err(Error::MessageTooLarge));
    assert_eq!(net.recv(1, h), Err(Error::WouldBlock));
}

#[test]
fn remote_ip_uses_gateway_arp_and_ingress_is_bounded() {
    let mock = Mock::default();
    let mut net = Network::new(mock.clone(), [2, 1, 2, 3, 4, 5]);
    let h = net.open(1, Kind::Icmp).unwrap();
    net.connect(1, h, [8, 8, 8, 8], 0).unwrap();
    let mut packet = Packet::empty();
    packet.port = 7;
    packet.len = 4;
    packet.data[..4].copy_from_slice(b"test");
    assert_eq!(net.send(1, h, &packet), Ok(4));
    net.progress(0);
    let queues = mock.0.borrow();
    let arp = queues
        .tx
        .iter()
        .find(|f| f.get(12..14) == Some(&[8, 6]))
        .expect("gateway ARP request");
    assert_eq!(&arp[38..42], &[10, 0, 2, 2]);
    drop(queues);
    for _ in 0..100 {
        mock.0.borrow_mut().rx.push_back(vec![0; 60]);
    }
    net.progress(55);
    assert_eq!(
        mock.0.borrow().rx.len(),
        92,
        "one service call consumes at most eight frames"
    );
}

#[test]
fn pending_tcp_handshake_is_not_eof() {
    let mut net = Network::new(Mock::default(), [2, 1, 2, 3, 4, 5]);
    let h = net.open(1, Kind::Tcp).unwrap();
    net.connect(1, h, [10, 0, 2, 2], 80).unwrap();
    assert_eq!(net.poll(1, h).unwrap() & rustos_net_abi::EOF, 0);
    assert_eq!(net.recv(1, h), Err(Error::WouldBlock));
}
