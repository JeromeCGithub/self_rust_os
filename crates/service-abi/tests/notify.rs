use rustos_service_abi::*;
#[test]
fn init_ready_and_heartbeat_roundtrip_have_exact_versioned_shapes() {
    let messages = [
        Message::Init {
            run: 7,
            notify: false,
            watchdog: None,
        },
        Message::Init {
            run: 7,
            notify: true,
            watchdog: Some(8),
        },
        Message::Ready { run: 7 },
        Message::Heartbeat { run: 7 },
    ];
    for message in messages {
        let mut bytes = [0xaa; MAX_FRAME];
        let n = message.encode(&mut bytes).unwrap();
        assert_eq!(&bytes[..4], &1u32.to_le_bytes());
        assert_eq!(&bytes[8..16], &7u64.to_le_bytes());
        assert_eq!(Message::decode(&bytes[..n]), Ok(message));
        assert!(bytes[n..].iter().all(|&b| b == 0));
        assert_eq!(
            n,
            if matches!(message, Message::Init { .. }) {
                32
            } else {
                24
            }
        );
    }
}
#[test]
fn structured_log_is_bounded_unicode_and_cannot_inject_control_records() {
    for level in [Level::Debug, Level::Info, Level::Warning, Level::Error] {
        let message = Message::Log {
            run: 1,
            level,
            text: "hello café",
        };
        let mut bytes = [0; MAX_FRAME];
        let n = message.encode(&mut bytes).unwrap();
        assert_eq!(Message::decode(&bytes[..n]), Ok(message));
    }
    let exact = "x".repeat(MAX_TEXT);
    let mut bytes = [0; MAX_FRAME];
    assert_eq!(
        Message::Log {
            run: 1,
            level: Level::Info,
            text: &exact
        }
        .encode(&mut bytes),
        Ok(MAX_FRAME)
    );
    let too_large = "x".repeat(MAX_TEXT + 1);
    assert_eq!(
        Message::Log {
            run: 1,
            level: Level::Info,
            text: &too_large
        }
        .encode(&mut bytes),
        Err(Error::Limit)
    );
    for text in ["spoof\n[kernel]", "nul\0tail", "escape\x1b[2J"] {
        assert!(Message::Log {
            run: 1,
            level: Level::Info,
            text
        }
        .encode(&mut bytes)
        .is_err());
    }
}
#[test]
fn forged_runs_unknown_kinds_bad_lengths_and_reserved_flags_are_rejected() {
    let mut bytes = [0; MAX_FRAME];
    let n = Message::Ready { run: 1 }.encode(&mut bytes).unwrap();
    for (offset, value) in [(0, 2), (4, 99), (8, 0), (16, 1), (20, 1)] {
        let mut bad = bytes;
        bad[offset] = value;
        assert!(Message::decode(&bad[..n]).is_err(), "offset {offset}");
    }
    assert!(Message::decode(&bytes[..n - 1]).is_err());
    assert!(Message::decode(&bytes[..n + 1]).is_err());
    let n = Message::Init {
        run: 1,
        notify: true,
        watchdog: Some(8),
    }
    .encode(&mut bytes)
    .unwrap();
    bytes[20] = 0;
    assert!(
        Message::decode(&bytes[..n]).is_err(),
        "watchdog requires explicit readiness"
    );
    assert!(Message::Ready { run: 0 }.encode(&mut bytes).is_err());
    assert!(Message::Init {
        run: 1,
        notify: true,
        watchdog: Some(0)
    }
    .encode(&mut bytes)
    .is_err());
    assert!(Message::Init {
        run: 1,
        notify: true,
        watchdog: Some(u64::MAX)
    }
    .encode(&mut bytes)
    .is_err());
}

#[test]
fn client_never_sends_before_init_and_does_not_commit_ready_until_transport_succeeds() {
    let sender = rustos_ipc_abi::ProcessKey {
        pid: 17,
        generation: 1,
    };
    let mut client = ClientState::new();
    assert!(client.ready_frame().is_err());
    assert!(client.heartbeat_frame().is_err());
    assert!(client.log_frame(Level::Info, "x").is_err());
    client
        .accept_init(
            sender,
            Message::Init {
                run: 8,
                notify: true,
                watchdog: Some(9),
            },
        )
        .unwrap();
    assert_eq!(client.run(), Some(8));
    assert_eq!(client.manager(), Some(sender));
    assert_eq!(client.watchdog(), Some(9));
    assert_eq!(client.ready_frame(), Ok(Some(Message::Ready { run: 8 })));
    assert_eq!(
        client.ready_frame(),
        Ok(Some(Message::Ready { run: 8 })),
        "send failure preserves retryable readiness"
    );
    assert!(client.heartbeat_frame().is_err());
    client.ready_sent().unwrap();
    assert!(client.ready_frame().is_err());
    assert_eq!(client.heartbeat_frame(), Ok(Message::Heartbeat { run: 8 }));
    assert_eq!(
        client.log_frame(Level::Warning, "work"),
        Ok(Message::Log {
            run: 8,
            level: Level::Warning,
            text: "work"
        })
    );
}
#[test]
fn duplicate_or_malformed_init_cannot_replace_the_authenticated_run_context() {
    let sender = rustos_ipc_abi::ProcessKey {
        pid: 17,
        generation: 1,
    };
    let mut client = ClientState::new();
    for message in [
        Message::Ready { run: 1 },
        Message::Init {
            run: 0,
            notify: false,
            watchdog: None,
        },
        Message::Init {
            run: 1,
            notify: false,
            watchdog: Some(8),
        },
    ] {
        assert!(client.accept_init(sender, message).is_err());
        assert_eq!(client.run(), None);
    }
    client
        .accept_init(
            sender,
            Message::Init {
                run: 8,
                notify: true,
                watchdog: None,
            },
        )
        .unwrap();
    assert!(client
        .accept_init(
            sender,
            Message::Init {
                run: 9,
                notify: true,
                watchdog: None
            }
        )
        .is_err());
    assert_eq!(client.run(), Some(8));
    assert_eq!(client.manager(), Some(sender));
}
#[test]
fn spawned_readiness_needs_no_ready_packet_and_watchdog_is_explicitly_absent() {
    let mut client = ClientState::new();
    let sender = rustos_ipc_abi::ProcessKey {
        pid: 1,
        generation: 1,
    };
    client
        .accept_init(
            sender,
            Message::Init {
                run: 1,
                notify: false,
                watchdog: None,
            },
        )
        .unwrap();
    assert_eq!(client.ready_frame(), Ok(None));
    client.ready_sent().unwrap();
    assert!(client.heartbeat_frame().is_err());
}
