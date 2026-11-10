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
    image_validation();
    early_publication();
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

fn image_validation() {
    use ipc::abi;
    static IMAGE: [u8; 2] = [0xeb, 0xfe];
    let image = &IMAGE; // Legacy flat compatibility: an isolated user-mode loop.
    let request = abi::ImageSpawn {
        launch: abi::Spawn {
            argv: 0,
            argc: 0,
            envp: 0,
            envc: 0,
            bindings: 0,
            binding_count: 0,
        },
        image_length: image.len() as u64,
    }
    .encode()
    .unwrap();
    let before = raw(107, 0, 0, 0);
    let result = raw(
        abi::SYS_SPAWN_IMAGE,
        image.as_ptr() as u64,
        request.as_ptr() as u64,
        request.len() as u64,
    );
    assert_ne!(
        result,
        sys::SYSCALL_ERROR,
        "valid owned image startup must be implemented"
    );
    let pid = u32::try_from(result).unwrap();
    assert!(pid > 1);
    supervision::terminate(ipc::ProcessKey { pid, generation: 1 }).unwrap();
    loop {
        if let Some(supervision::ChildEvent::Completed {
            process,
            cause,
            status,
        }) = supervision::next_child().unwrap()
        {
            assert_eq!(process.pid, pid);
            assert_eq!(cause, supervision::Cause::Killed);
            assert_eq!(status, 9);
            break;
        }
        sys::wait_events(sys::Events::CHILD, 100).unwrap();
    }
    for _ in 0..100 {
        if raw(107, 0, 0, 0) == before {
            break;
        }
        sys::sleep_ticks(1).unwrap();
    }
    assert_eq!(raw(107, 0, 0, 0), before, "image child frames must recover");
    for (pointer, packet, size) in [
        (0, request.as_ptr() as u64, 72),
        (u64::MAX - 1, request.as_ptr() as u64, 72),
        (image.as_ptr() as u64, 0, 72),
        (image.as_ptr() as u64, request.as_ptr() as u64, 71),
    ] {
        assert_eq!(
            raw(abi::SYS_SPAWN_IMAGE, pointer, packet, size),
            sys::SYSCALL_ERROR
        );
    }
    for pointer in [0xffff800000000000, 0x3ffc00000 - 1, 0x400000000 - 1] {
        assert_eq!(
            raw(abi::SYS_SPAWN_IMAGE, pointer, request.as_ptr() as u64, 72),
            sys::SYSCALL_ERROR
        );
    }
    let [server, source] = ipc::Endpoint::pair().unwrap();
    let grant = abi::Binding {
        source: source.raw().unwrap(),
        tag: 42,
        rights: ipc::Rights::SEND,
    }
    .encode()
    .unwrap();
    let malformed = [0x7f, b'E', b'L', b'F', 0, 0, 0, 0];
    let bad = abi::ImageSpawn {
        launch: abi::Spawn {
            argv: 0,
            argc: 0,
            envp: 0,
            envc: 0,
            bindings: grant.as_ptr() as u64,
            binding_count: 1,
        },
        image_length: malformed.len() as u64,
    }
    .encode()
    .unwrap();
    assert_eq!(
        raw(
            abi::SYS_SPAWN_IMAGE,
            malformed.as_ptr() as u64,
            bad.as_ptr() as u64,
            72
        ),
        sys::SYSCALL_ERROR
    );
    source
        .send(b"kept")
        .expect("failed image preserves move source");
    let event = ipc::next().unwrap().unwrap();
    assert_eq!(Some(event.endpoint), server.raw());
    assert_eq!(&event.payload[..event.length], b"kept");
    assert!(supervision::next_child().unwrap().is_none());
    drop(source);
    drop(server);
    recordln!("[init-probe] image startup PASS");
}

fn early_publication() {
    use initator::{
        launcher::{Launcher, Spec},
        ports::Ports,
    };
    use rustos_service_core::{
        graph::Catalog,
        manager::{Action, Manager},
        publication::{EarlyExit, EarlyExits},
        transaction::Outcome,
        Completion, ProcessKey,
    };
    let definition = b"Version=1\nKind=service\nName=fault\nExec=/BIN/FLICKER.SVC\nArg=--fault\n";
    let mut launcher = Launcher::new().unwrap();
    for _ in 0..4 {
        let mut manager =
            Manager::new(Catalog::load(&[("FAULT.UNIT", definition)]).unwrap()).unwrap();
        manager.start("fault").unwrap();
        let Some(Action::Start(start)) = manager.next_action(sys::ticks()).unwrap() else {
            panic!("reserved early launch")
        };
        launcher
            .submit(
                start,
                Spec::new(manager.catalog().unit(0).unwrap()).unwrap(),
            )
            .unwrap();
        let mut early = EarlyExits::<2>::new();
        let mut woke = false;
        let record = loop {
            // Deliberately consume the durable exit BEFORE taking the launch
            // result, even if the worker has already posted it. This fixes the
            // race ordering rather than depending on scheduler luck.
            if let Some(supervision::ChildEvent::Completed {
                process,
                cause,
                status,
            }) = supervision::next_child().unwrap()
            {
                assert_eq!(cause, supervision::Cause::Fault);
                assert_eq!(status, 6);
                let record = EarlyExit {
                    process: ProcessKey {
                        pid: process.pid,
                        generation: process.generation,
                    },
                    cause: Completion::Fault(6),
                    tick: sys::ticks(),
                };
                early.insert(record).unwrap();
                break record;
            }
            while let Some(event) = ipc::next().unwrap() {
                if event.endpoint == launcher.wake_handle() {
                    woke = true;
                }
            }
            sys::wait_events(sys::Events::CHILD | sys::Events::PORT, 100).unwrap();
        };
        // Consume the wake without consuming the result. The actor's final
        // pre-wait predicate must still notice this durable publication.
        while !woke {
            while let Some(event) = ipc::next().unwrap() {
                if event.endpoint == launcher.wake_handle() {
                    woke = true;
                }
            }
            if !woke {
                sys::wait_events(sys::Events::PORT, 100).unwrap();
            }
        }
        assert!(
            launcher.has_result(),
            "a consumed wake cannot hide a durable launch result"
        );
        let (token, result) = loop {
            if let Some(result) = launcher.take() {
                break result;
            }
            while ipc::next().unwrap().is_some() {}
            sys::wait_events(sys::Events::PORT, 100).unwrap();
        };
        assert_eq!(token, start);
        let run = result.unwrap();
        assert_eq!(run.key(), record.process);
        manager.spawned(start, run.key(), sys::ticks()).unwrap();
        let mut ports = Ports::new();
        ports.install(start.unit, run).unwrap();
        let cached = early.take(record.process).unwrap();
        assert_eq!(
            manager.completed(cached.process, cached.cause, cached.tick),
            Some(start.unit)
        );
        ports.mark_exit(ipc::ProcessKey {
            pid: record.process.pid,
            generation: record.process.generation,
        });
        ports.finish_exits();
        manager.next_action(sys::ticks()).unwrap();
        assert_eq!(manager.outcome(), Outcome::Failed);
    }
    // Cancellation is scoped to the outstanding reservation, including a
    // result already posted by the worker. A stale token cannot cancel its successor.
    for run_id in 10..14 {
        let token = rustos_service_core::Start {
            unit: 0,
            run: run_id,
        };
        let unit = rustos_service_core::config::parse(definition).unwrap();
        assert_eq!(launcher.current(), None);
        launcher.submit(token, Spec::new(&unit).unwrap()).unwrap();
        assert_eq!(
            launcher.current(),
            Some(token),
            "outstanding launch identity"
        );
        assert!(!launcher.cancel(rustos_service_core::Start {
            run: run_id - 1,
            ..token
        }));
        assert!(launcher.cancel(token));
        let (returned, result) = loop {
            if let Some(result) = launcher.take() {
                break result;
            }
            while ipc::next().unwrap().is_some() {}
            if !launcher.has_result() {
                sys::wait_events(sys::Events::PORT, 100).unwrap();
            }
        };
        assert_eq!(returned, token);
        assert_eq!(launcher.current(), None);
        assert!(!launcher.cancel(token));
        // Creation may have committed before cancellation; reap that exact child.
        if let Ok(run) = result {
            let key = run.key();
            let _ = supervision::terminate(ipc::ProcessKey {
                pid: key.pid,
                generation: key.generation,
            });
            let _ = sys::waitpid(key.pid);
            drop(run);
        }
        while ipc::next().unwrap().is_some() {}
        assert!(supervision::next_child().unwrap().is_none());
    }
    drop(launcher);
    assert!(
        ipc::next().unwrap().is_none(),
        "dropping launch worker must close both private endpoints"
    );
    assert!(supervision::next_child().unwrap().is_none());
    // Discard a queued/in-progress launch without taking its result. Cleanup
    // must cancel/join the worker, reap any unpublished child and close wakes.
    for _ in 0..4 {
        let mut discarded = Launcher::new().unwrap();
        let unit = rustos_service_core::config::parse(definition).unwrap();
        discarded
            .submit(
                rustos_service_core::Start { unit: 0, run: 1 },
                Spec::new(&unit).unwrap(),
            )
            .unwrap();
        drop(discarded);
        assert!(supervision::next_child().unwrap().is_none());
        assert!(ipc::next().unwrap().is_none());
    }
    recordln!("[init-probe] early publication PASS cycles=4");
}

pub fn stop_peer() {
    let mut client = userlib::service::Client::bootstrap().unwrap();
    let mut ready = false;
    let deadline = sys::ticks() + 90;
    loop {
        while let Some(event) = ipc::next().unwrap() {
            client
                .on_event(&event)
                .expect("authenticated manager Stop must reach SDK client");
        }
        if client.run().is_some() && !ready {
            client.ready().unwrap();
            ready = true;
        }
        if client.stop_requested() {
            return;
        }
        assert!(sys::ticks() < deadline, "SDK stop delivery deadline");
        sys::wait_events(sys::Events::PORT, 1).unwrap();
    }
}
pub fn stop_protocol() -> ! {
    use userlib::service::abi::{self, Message};
    let [parent, mut child] = ipc::Endpoint::pair().unwrap();
    let pid = ipc::spawn(
        "/BIN/ZINIT.SVC",
        &["--stop-peer"],
        &[],
        &mut [ipc::Binding {
            endpoint: &mut child,
            tag: abi::NOTIFY_TAG,
            rights: ipc::Rights::from_bits(3).unwrap(),
        }],
    )
    .unwrap();
    let mut bytes = [0; abi::MAX_FRAME];
    let n = Message::Init {
        run: 7,
        notify: true,
        watchdog: None,
    }
    .encode(&mut bytes)
    .unwrap();
    parent.send(&bytes[..n]).unwrap();
    let deadline = sys::ticks() + 90;
    let mut sent = false;
    loop {
        while let Some(event) = ipc::next().unwrap() {
            if event.kind == ipc::Kind::Message {
                assert_eq!(event.sender, Some(ipc::ProcessKey { pid, generation: 1 }));
                assert_eq!(
                    Message::decode(&event.payload[..event.length]),
                    Ok(Message::Ready { run: 7 })
                );
                assert!(!sent);
                let n = Message::Stop { run: 7 }.encode(&mut bytes).unwrap();
                parent.send(&bytes[..n]).unwrap();
                sent = true;
            }
        }
        if let Some(supervision::ChildEvent::Completed {
            process,
            cause,
            status,
        }) = supervision::next_child().unwrap()
        {
            assert!(sent);
            assert_eq!(process.pid, pid);
            assert_eq!(cause, supervision::Cause::Exited);
            assert_eq!(status, 0, "service must exit cleanly after Stop");
            break;
        }
        assert!(
            sys::ticks() < deadline,
            "cooperative stop completion deadline"
        );
        sys::wait_events(sys::Events::PORT | sys::Events::CHILD, 1).unwrap();
    }
    drop(parent);
    while ipc::next().unwrap().is_some() {}
    recordln!("[init-probe] stop protocol PASS");
    loop {
        sys::wait_events(sys::Events::CHILD, 0).unwrap();
    }
}
