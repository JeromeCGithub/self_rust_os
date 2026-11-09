//! Private native channel/endpoint-binding acceptance probe.
use userlib::{
    ipc::{self, Binding, Endpoint, Kind, Rights, SendError},
    recordln, supervision, sys,
};
fn event() -> ipc::Event {
    loop {
        if let Some(event) = ipc::next().expect("owned port event") {
            return event;
        }
        sys::wait_events(sys::Events::PORT, 100).expect("bounded port wait");
    }
}
pub fn peer() {
    let mut endpoint = Endpoint::bootstrap(7).expect("single explicit bootstrap grant");
    let control = Endpoint::bootstrap(8).expect("independent native control grant");
    assert!(Endpoint::bootstrap(7).is_err());
    assert_eq!(
        control.send(b"forged"),
        Err(SendError::Invalid),
        "receive-only grant cannot send"
    );
    endpoint
        .set_readable(false)
        .expect("pause data readiness until control");
    endpoint.send(b"hello").expect("peer hello");
    let go = event();
    assert_eq!(go.endpoint, control.raw().unwrap());
    assert_eq!(&go.payload[..2], b"go");
    endpoint.set_readable(true).expect("resume FIFO data");
    for n in 0..8 {
        let input = event();
        assert_eq!(input.kind, Kind::Message);
        assert_eq!(input.endpoint, endpoint.raw().unwrap());
        assert_eq!(input.payload[0], n);
    }
    endpoint
        .set_readable(false)
        .expect("wait for control without draining data");
    endpoint.send(b"drained").expect("drain acknowledgement");
    let exit = event();
    assert_eq!(exit.endpoint, control.raw().unwrap());
    assert_eq!(&exit.payload[..4], b"exit");
}
pub fn run() {
    validation();
    let [server, mut peer] = Endpoint::pair().expect("native channel pair");
    let [control, mut control_peer] = Endpoint::pair().expect("independent control channel");
    let old = peer.raw().unwrap();
    let child = ipc::spawn(
        "/BIN/ZINIT.SVC",
        &["--ipc-peer"],
        &[],
        &mut [
            Binding {
                endpoint: &mut peer,
                tag: 7,
                rights: Rights::ALL,
            },
            Binding {
                endpoint: &mut control_peer,
                tag: 8,
                rights: Rights::RECEIVE,
            },
        ],
    )
    .expect("atomic endpoint-bound spawn");
    assert_eq!(
        peer.raw(),
        None,
        "successful move invalidates parent ownership wrapper"
    );
    let hello = event();
    assert_eq!(hello.kind, Kind::Message);
    assert_eq!(
        hello.sender,
        Some(ipc::ProcessKey {
            pid: child,
            generation: 1
        })
    );
    assert_eq!(&hello.payload[..5], b"hello");
    // SAFETY: Only scalars cross close; the stale handle was previously owned by
    // this parent and must now fail the kernel's current-owner check.
    assert_eq!(
        unsafe { sys::syscall(123, old.0, 0, 0) },
        sys::SYSCALL_ERROR
    );
    for n in 0..8 {
        server.send(&[n]).expect("bounded FIFO fill");
    }
    assert_eq!(server.send(b"overflow"), Err(SendError::Full));
    server
        .request_writable(true)
        .expect("one-shot writable interest");
    control.send(b"go").expect("release native peer barrier");
    let mut writable = false;
    let mut drained = false;
    let mut closed = false;
    let mut completed = false;
    let mut released = false;
    loop {
        while let Some(message) = ipc::next().expect("parent channel drain") {
            if message.endpoint == control.raw().unwrap() {
                assert_eq!(message.kind, Kind::PeerClosed);
                continue;
            }
            assert_eq!(message.endpoint, server.raw().unwrap());
            match message.kind {
                Kind::Writable => {
                    assert!(!writable);
                    writable = true;
                }
                Kind::Message => {
                    assert_eq!(
                        message.sender,
                        Some(ipc::ProcessKey {
                            pid: child,
                            generation: 1
                        })
                    );
                    assert_eq!(&message.payload[..7], b"drained");
                    drained = true;
                }
                Kind::PeerClosed => {
                    assert!(drained, "queued data precedes peer close");
                    closed = true;
                }
            }
        }
        while let Some(message) = supervision::next_child().expect("child result") {
            match message {
                supervision::ChildEvent::Completed {
                    process,
                    cause,
                    status,
                } => {
                    assert_eq!(process.pid, child);
                    assert_eq!(cause, supervision::Cause::Exited);
                    assert_eq!(status, 0);
                    completed = true;
                }
                _ => panic!("unexpected orphan"),
            }
        }
        if drained && writable && !released {
            control
                .send(b"exit")
                .expect("allow peer exit after writable observation");
            released = true;
        }
        if drained && closed && completed {
            break;
        }
        sys::wait_events(sys::Events::PORT | sys::Events::CHILD, 100).expect("channel/exit wait");
    }
    assert!(
        writable,
        "peer stayed live until writable readiness was observed"
    );
    // A consumed hangup must not create an infinite event/wakeup loop.
    assert!(ipc::next().unwrap().is_none());
    recordln!(
        "[init-probe] IPC PASS fifo=8 moved=true closed=true writable={}",
        writable
    );
    loop {
        sys::wait_events(sys::Events::CHILD, 0).expect("probe parked");
    }
}

fn raw(number: u64, a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: This private regression probe passes only scalar syscall registers.
    // Deliberately invalid pointer values are never dereferenced by Rust; the
    // kernel must reject them. Valid test buffers stay live through each call.
    unsafe { sys::syscall(number, a, b, c) }
}
fn validation() {
    use ipc::abi;
    let error = sys::SYSCALL_ERROR;
    for (pointer, size, reserved) in [
        (0, 16, 0),
        (0x400000, 16, 0),
        (u64::MAX - 8, 16, 0),
        (0x3ffc00000 - 8, 16, 0),
        (0x400000000 - 8, 16, 0),
    ] {
        assert_eq!(raw(abi::SYS_PAIR, pointer, size, reserved), error);
    }
    {
        let [a, mut b] = Endpoint::pair().unwrap();
        let [_c, mut d] = Endpoint::pair().unwrap();
        let before = b.raw();
        let other = d.raw();
        assert!(
            ipc::spawn(
                "/BIN/ZINIT.SVC",
                &["--ipc-peer"],
                &[],
                &mut [
                    Binding {
                        endpoint: &mut b,
                        tag: 7,
                        rights: Rights::ALL
                    },
                    Binding {
                        endpoint: &mut d,
                        tag: 7,
                        rights: Rights::ALL
                    }
                ]
            )
            .is_err(),
            "duplicate grant tags must fail atomically"
        );
        assert_eq!(b.raw(), before);
        assert_eq!(d.raw(), other);
        for path in ["/BIN/ABSENT.SVC", "/BIN/BAD.SVC"] {
            assert!(ipc::spawn(
                path,
                &[],
                &[],
                &mut [Binding {
                    endpoint: &mut b,
                    tag: 7,
                    rights: Rights::ALL
                }]
            )
            .is_err());
            assert_eq!(b.raw(), before);
        }
        assert!(
            supervision::next_child().unwrap().is_none(),
            "failed launches must not publish zombies"
        );
        b.send(b"retained").unwrap();
        for (pointer, size, reserved) in [
            (0, abi::EVENT_SIZE as u64, 0),
            (0x400000, abi::EVENT_SIZE as u64, 0),
            (0x400000000 - 8, abi::EVENT_SIZE as u64, 0),
        ] {
            assert_eq!(raw(abi::SYS_NEXT, pointer, size, reserved), error);
        }
        let event = ipc::next().unwrap().unwrap();
        assert_eq!(event.endpoint, a.raw().unwrap());
        assert_eq!(&event.payload[..8], b"retained");
        assert_eq!(raw(abi::SYS_CLOSE, b.raw().unwrap().0, 1, 0), error);
        assert_eq!(raw(abi::SYS_INTEREST, b.raw().unwrap().0, 4, 0), error);
        assert_eq!(raw(abi::SYS_BOOTSTRAP, u32::MAX as u64 + 1, 0, 0), error);
    }
    assert!(ipc::next().unwrap().is_none());
    // Exhaust the caller's endpoint budget after invalid pair attempts: an
    // output-copy failure must not have leaked hidden handles or channel slots.
    {
        let mut pairs = alloc::vec::Vec::new();
        pairs.try_reserve_exact(abi::MAX_HANDLES / 2).unwrap();
        for _ in 0..abi::MAX_HANDLES / 2 {
            pairs.push(Endpoint::pair().expect("reusable endpoint quota"));
        }
        assert!(Endpoint::pair().is_err());
    }
    assert!(
        Endpoint::pair().is_ok(),
        "dropping every owned pair must restore admission"
    );
    let path = b"/BIN/ZINIT.SVC\0";
    let valid = abi::Spawn {
        argv: 0,
        argc: 0,
        envp: 0,
        envc: 0,
        bindings: 0,
        binding_count: 0,
    }
    .encode()
    .unwrap();
    for (offset, value) in [
        (0, 1u64),
        (8, 1),
        (24, 4097),
        (40, 4097),
        (56, 9),
        (56, 1),
        (24, 1),
    ] {
        let mut request = valid;
        request[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        assert_eq!(
            raw(
                abi::SYS_SPAWN_BOUND,
                path.as_ptr() as u64,
                request.as_ptr() as u64,
                abi::SPAWN_SIZE as u64
            ),
            error
        );
    }
    assert!(supervision::next_child().unwrap().is_none());
    recordln!("[init-probe] IPC validation PASS pointers=true rollback=true quota=true");
}
pub fn quick_peer() {
    let endpoint = Endpoint::bootstrap(7).expect("quick child grant");
    endpoint.send(b"done").unwrap();
}
fn quick_roundtrip() {
    let [server, mut peer] = Endpoint::pair().unwrap();
    let child = ipc::spawn(
        "/BIN/ZINIT.SVC",
        &["--ipc-quick"],
        &[],
        &mut [Binding {
            endpoint: &mut peer,
            tag: 7,
            rights: Rights::SEND,
        }],
    )
    .unwrap();
    let mut data = false;
    let mut closed = false;
    let mut completed = false;
    loop {
        while let Some(event) = ipc::next().unwrap() {
            assert_eq!(event.endpoint, server.raw().unwrap());
            match event.kind {
                Kind::Message => {
                    assert_eq!(
                        event.sender,
                        Some(ipc::ProcessKey {
                            pid: child,
                            generation: 1
                        })
                    );
                    assert_eq!(&event.payload[..4], b"done");
                    data = true;
                }
                Kind::PeerClosed => {
                    assert!(data);
                    closed = true;
                }
                _ => panic!("unrequested writable signal"),
            }
        }
        while let Some(event) = supervision::next_child().unwrap() {
            match event {
                supervision::ChildEvent::Completed {
                    process,
                    cause,
                    status,
                } => {
                    assert_eq!(process.pid, child);
                    assert_eq!(cause, supervision::Cause::Exited);
                    assert_eq!(status, 0);
                    completed = true;
                }
                _ => panic!("unrequested adoption"),
            }
        }
        if data && closed && completed {
            break;
        }
        sys::wait_events(sys::Events::PORT | sys::Events::CHILD, 100).unwrap();
    }
}
pub fn lifecycle() {
    quick_roundtrip(); // Warm allocator and lazy startup paths before the baseline.
    let before = raw(107, 0, 0, 0);
    for _ in 0..64 {
        quick_roundtrip();
        assert_eq!(
            raw(107, 0, 0, 0),
            before,
            "bound-channel process frames must fully recover"
        );
    }
    recordln!(
        "[init-probe] IPC lifecycle PASS cycles=64 frames={}",
        before
    );
    loop {
        sys::wait_events(sys::Events::CHILD, 0).unwrap();
    }
}
