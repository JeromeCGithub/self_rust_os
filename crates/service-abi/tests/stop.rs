use rustos_service_abi::*;
fn peer(pid: u32) -> PeerKey {
    PeerKey { pid, generation: 1 }
}
#[test]
fn stop_is_a_canonical_run_scoped_manager_to_service_message() {
    let message = Message::Stop { run: 7 };
    let mut bytes = [0xaa; MAX_FRAME];
    let n = message
        .encode(&mut bytes)
        .expect("cooperative stop encoding");
    assert_eq!(n, HEADER);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 5);
    assert_eq!(Message::decode(&bytes[..n]), Ok(message));
    assert!(bytes[n..].iter().all(|&b| b == 0));
    for offset in [0, 16, 20] {
        let mut bad = bytes;
        bad[offset] = 99;
        assert!(Message::decode(&bad[..n]).is_err());
    }
    assert!(Message::decode(&bytes[..n - 1]).is_err());
    assert!(Message::decode(&bytes[..n + 1]).is_err());
    assert!(Message::Stop { run: 0 }.encode(&mut bytes).is_err());
}
#[test]
fn only_initialized_manager_can_stop_exact_run_and_health_stays_withdrawn() {
    let mut state = ClientState::new();
    assert!(!state.stop_requested());
    assert!(state
        .accept_stop(peer(1), Message::Stop { run: 7 })
        .is_err());
    state
        .accept_init(
            peer(1),
            Message::Init {
                run: 7,
                notify: true,
                watchdog: Some(5),
            },
        )
        .unwrap();
    for (sender, message) in [
        (peer(2), Message::Stop { run: 7 }),
        (peer(1), Message::Stop { run: 8 }),
        (peer(1), Message::Ready { run: 7 }),
    ] {
        assert!(state.accept_stop(sender, message).is_err());
        assert!(!state.stop_requested());
    }
    state
        .accept_stop(peer(1), Message::Stop { run: 7 })
        .unwrap();
    assert!(state.stop_requested());
    assert!(state.ready_frame().is_err());
    assert!(state.heartbeat_frame().is_err());
    state
        .accept_stop(peer(1), Message::Stop { run: 7 })
        .unwrap();
    assert!(state.log_frame(Level::Info, "cleaning up").is_ok());
}
