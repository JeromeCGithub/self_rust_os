//! Actual Ring 3 boundary/lifecycle/transport regression probe. The E2E runner
//! provisions /TMP/NET.CFG and local UDP/HTTP peers on a disposable guest disk.
#![no_std]
#![no_main]
use rustos_net_abi as abi;
use userlib::{
    net::{self, Error, Kind, Packet},
    println, sys,
};

fn pause() {
    sys::sleep_ticks(1).expect("probe timer wait");
}
fn until(mut condition: impl FnMut() -> bool) {
    let start = sys::ticks();
    while !condition() {
        assert!(
            sys::ticks().wrapping_sub(start) < 200,
            "bounded network probe deadline"
        );
        pause();
    }
}
fn write(path: &str, bytes: &[u8]) {
    assert_eq!(sys::write_file(path, bytes), Some(bytes.len()));
}
fn mode(value: &[u8]) {
    write("/TMP/NET.MOD", value);
}
fn read_handle() -> u64 {
    let mut bytes = [0; 8];
    assert_eq!(sys::read_file("/TMP/NET.HND", &mut bytes), Some(8));
    u64::from_le_bytes(bytes)
}
fn socket(kind: Kind) -> u64 {
    net::open(kind).expect("probe endpoint opens")
}

fn child(mode: u8) {
    if mode == b'f' {
        let foreign = read_handle();
        assert_eq!(net::close(foreign), Err(Error::BadHandle));
        assert_eq!(net::poll(foreign), Err(Error::BadHandle));
        assert_eq!(net::recv(foreign), Err(Error::BadHandle));
        assert_eq!(
            net::connect(foreign, [10, 0, 2, 2], 0),
            Err(Error::BadHandle)
        );
    }
    for _ in 0..4 {
        let _ = socket(Kind::Icmp);
    }
    write("/TMP/NET.RDY", b"1");
    if mode == b'k' {
        loop {
            pause();
        }
    }
    // Intentionally leave all four endpoints open: exit must release them.
}

fn boundary(handle: u64) {
    for pointer in [0, u64::MAX - 100, 0x1000000, 0x1ffbfff, 0xffff800000000000] {
        // SAFETY: Deliberately invalid pointers are never dereferenced in Ring 3.
        // The tested syscall must reject them before copying or forming a slice.
        let result = unsafe {
            sys::syscall(
                abi::SYS_RECV,
                handle,
                pointer,
                core::mem::size_of::<Packet>() as u64,
            )
        };
        assert_eq!(abi::decode(result), Err(Error::Invalid));
    }
    let mut packet = Packet::empty();
    // SAFETY: Deliberately wrong fixed-record length; pointer itself is valid.
    let result =
        unsafe { sys::syscall(abi::SYS_RECV, handle, &mut packet as *mut Packet as u64, 1) };
    assert_eq!(abi::decode(result), Err(Error::Invalid));
    // SAFETY: Invalid enum value passed as a scalar to test kernel validation.
    assert_eq!(
        abi::decode(unsafe { sys::syscall(abi::SYS_SOCKET, 99, 0, 0) }),
        Err(Error::Invalid)
    );
    // SAFETY: Invalid reserved endpoint bits; no pointer is involved.
    assert_eq!(
        abi::decode(unsafe { sys::syscall(abi::SYS_CONNECT, handle, 1 << 63, 0) }),
        Err(Error::Invalid)
    );
    net::connect(handle, [10, 0, 2, 2], 0).unwrap();
    packet.len = 65;
    assert_eq!(net::send(handle, &packet), Err(Error::MessageTooLarge));
    // Byte copies support unaligned records without manufacturing aligned refs.
    let mut unaligned = [0u8; 1033];
    // SAFETY: The complete 1032-byte output starting at byte one is writable and
    // stays live; no reply is queued, so the syscall returns WouldBlock.
    let raw = unsafe {
        sys::syscall(
            abi::SYS_RECV,
            handle,
            unaligned.as_mut_ptr().add(1) as u64,
            1032,
        )
    };
    assert_eq!(abi::decode(raw), Err(Error::WouldBlock));
}

fn lifecycle(handle: u64) {
    let baseline = net::info().unwrap().open_sockets;
    write("/TMP/NET.HND", &handle.to_le_bytes());
    for setting in [b'f', b'k'] {
        mode(&[setting]);
        write("/TMP/NET.RDY", b"0");
        let child = sys::spawn("/BIN/ZNETTEST.BIN").unwrap();
        until(|| {
            let mut ready = [0; 1];
            sys::read_file("/TMP/NET.RDY", &mut ready) == Some(1) && ready[0] == b'1'
        });
        if setting == b'k' {
            sys::kill(child).unwrap();
        }
        let exit = sys::waitpid(child).unwrap();
        assert_eq!(exit, if setting == b'k' { 9 } else { 0 });
        assert!(
            sys::waitpid(child).is_err(),
            "wait must reap the child exactly once"
        );
        assert_eq!(
            net::info().unwrap().open_sockets,
            baseline,
            "exit/kill must release child's four sockets"
        );
    }
    mode(b"r");
    for _ in 0..64 {
        let socket = socket(Kind::Udp);
        net::close(socket).unwrap();
        assert_eq!(net::close(socket), Err(Error::BadHandle));
    }
    let mut handles = [0; 3];
    for handle in &mut handles {
        *handle = socket(Kind::Icmp);
    }
    assert_eq!(net::open(Kind::Udp), Err(Error::NoResources));
    for handle in handles {
        net::close(handle).unwrap();
    }
    assert_eq!(net::info().unwrap().open_sockets, baseline);
}

fn receive(handle: u64) -> Packet {
    let mut result = None;
    until(|| match net::recv(handle) {
        Ok(packet) => {
            result = Some(packet);
            true
        }
        Err(Error::WouldBlock) => false,
        Err(error) => panic!("receive failed: {:?}", error),
    });
    result.expect("receive deadline produced a result")
}

fn transports(udp_port: u16, tcp_port: u16) {
    let udp = socket(Kind::Udp);
    net::connect(udp, [10, 0, 2, 2], udp_port).unwrap();
    for len in [0, 1024] {
        let mut packet = Packet::empty();
        packet.len = len;
        for (index, byte) in packet.data.iter_mut().enumerate() {
            *byte = index as u8;
        }
        assert_eq!(net::send(udp, &packet), Ok(len as usize));
        let reply = receive(udp);
        assert_eq!(reply.address, [10, 0, 2, 2]);
        assert_eq!(reply.port, udp_port);
        assert_eq!(reply.len, len);
        assert_eq!(&reply.data[..len as usize], &packet.data[..len as usize]);
    }
    net::close(udp).unwrap();
    let tcp = socket(Kind::Tcp);
    net::connect(tcp, [10, 0, 2, 2], tcp_port).unwrap();
    until(|| net::poll(tcp).unwrap() & net::CONNECTED != 0);
    let request = b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let mut sent = 0;
    until(|| {
        let mut packet = Packet::empty();
        packet.len = (request.len() - sent) as u16;
        packet.data[..packet.len as usize].copy_from_slice(&request[sent..]);
        match net::send(tcp, &packet) {
            Ok(n) => sent += n,
            Err(Error::WouldBlock) => (),
            Err(error) => panic!("TCP send failed: {:?}", error),
        }
        sent == request.len()
    });
    let mut response = [0; 1024];
    let mut length = 0;
    loop {
        let packet = receive(tcp);
        if packet.len == 0 {
            break;
        }
        let n = packet.len as usize;
        assert!(length + n <= response.len());
        response[length..length + n].copy_from_slice(&packet.data[..n]);
        length += n;
    }
    assert!(response[..length].starts_with(b"HTTP/1.0 200 OK\r\n"));
    assert!(response[..length].ends_with(b"rustos-http-ok"));
    net::close(tcp).unwrap();
}

#[no_mangle]
pub extern "C" fn user_main() {
    sys::graphics_mode();
    let mut setting = [0];
    if sys::read_file("/TMP/NET.MOD", &mut setting) == Some(1)
        && (setting[0] == b'f' || setting[0] == b'k')
    {
        child(setting[0]);
        return;
    }
    let mut config = [0; 4];
    if sys::read_file("/TMP/NET.CFG", &mut config) != Some(4) {
        println!("[nettest] Run through e2e/network.py --probe to provision peers");
        sys::exit(1);
    }
    let baseline = net::info().unwrap().open_sockets;
    let handle = socket(Kind::Icmp);
    boundary(handle);
    lifecycle(handle);
    net::close(handle).unwrap();
    transports(
        u16::from_le_bytes([config[0], config[1]]),
        u16::from_le_bytes([config[2], config[3]]),
    );
    assert_eq!(net::info().unwrap().open_sockets, baseline);
    println!("[nettest] PASS: boundary lifecycle UDP TCP HTTP");
}
