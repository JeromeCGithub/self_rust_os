//! Controlled host fixture, speaking the same service payloads over three streams.
use rustos_service_abi::{ClientState, Level, Message, PeerKey, MAX_FRAME};
use std::{
    io::{self, Read, Write},
    thread,
    time::Duration,
};
fn read() -> Vec<u8> {
    let mut length = [0; 4];
    io::stdin().read_exact(&mut length).unwrap();
    let count = u32::from_le_bytes(length) as usize;
    assert!(count <= MAX_FRAME);
    let mut bytes = vec![0; count];
    io::stdin().read_exact(&mut bytes).unwrap();
    bytes
}
fn send(stream: &mut impl Write, message: Message<'_>) {
    let mut bytes = [0; MAX_FRAME];
    let count = message.encode(&mut bytes).unwrap();
    stream.write_all(&(count as u32).to_le_bytes()).unwrap();
    stream.write_all(&bytes[..count]).unwrap();
    stream.flush().unwrap();
}
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "notify".into());
    let init = read();
    let mut client = ClientState::new();
    client
        .accept_init(
            PeerKey {
                pid: 1,
                generation: 1,
            },
            Message::decode(&init).unwrap(),
        )
        .unwrap();
    send(
        &mut io::stderr(),
        client.log_frame(Level::Info, "host initialized").unwrap(),
    );
    if mode == "never-ready" {
        loop {
            thread::park();
        }
    }
    if mode == "malformed" {
        io::stdout().write_all(&u32::MAX.to_le_bytes()).unwrap();
        io::stdout().flush().unwrap();
        loop {
            thread::park();
        }
    }
    if let Some(ready) = client.ready_frame().unwrap() {
        send(&mut io::stdout(), ready);
    }
    client.ready_sent().unwrap();
    if mode == "exit" {
        let mut byte = [0];
        assert_eq!(io::stdin().read(&mut byte).unwrap(), 0);
        return;
    }
    if mode == "fail" {
        std::process::exit(23);
    }
    if mode == "wedge" {
        loop {
            thread::park();
        }
    }
    if mode == "flood" {
        let run = client.run().unwrap();
        thread::spawn(move || loop {
            send(
                &mut io::stderr(),
                Message::Log {
                    run,
                    level: Level::Info,
                    text: "bounded host flood",
                },
            );
        });
    }
    loop {
        thread::sleep(Duration::from_millis(20));
        if client.watchdog().is_some() {
            send(&mut io::stdout(), client.heartbeat_frame().unwrap());
        }
    }
}
