use rustos_ipc_core::{abi::*, *};
fn owner(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn binding(handle: Handle, tag: u32, rights: Rights) -> Binding {
    Binding {
        source: handle,
        tag,
        rights,
    }
}
fn receive(r: &mut Registry, pid: u32) -> Event {
    let pending = r.peek(owner(pid)).unwrap().expect("queued event");
    r.consume(owner(pid), pending).unwrap();
    pending.event
}
#[test]
fn fifo_is_bounded_and_sender_identity_is_not_supplied_in_payload() {
    let mut r = Registry::new();
    let [a, b] = r.pair(owner(1)).unwrap();
    r.bindings(owner(1), owner(2), &[binding(b, 1, Rights::ALL)])
        .unwrap();
    for n in 0..MAX_MESSAGES {
        r.send(owner(2), b, &[n as u8]).unwrap();
    }
    assert_eq!(r.send(owner(2), b, b"extra"), Err(Error::Full));
    for n in 0..MAX_MESSAGES {
        let event = receive(&mut r, 1);
        assert_eq!(event.endpoint, a);
        assert_eq!(event.sender, Some(owner(2)));
        assert_eq!(event.payload[0], n as u8);
        assert_eq!(event.length, 1);
    }
    assert!(!r.ready(owner(1)));
    assert_eq!(r.peek(owner(1)), Ok(None));
    assert_eq!(
        r.send(owner(2), b, &[0; MAX_PAYLOAD + 1]),
        Err(Error::Invalid)
    );
}
#[test]
fn foreign_stale_closed_and_wrong_generation_handles_are_denied() {
    let mut r = Registry::new();
    let [a, b] = r.pair(owner(1)).unwrap();
    assert_eq!(r.send(owner(2), a, b"bad"), Err(Error::Denied));
    assert_eq!(r.close(owner(2), b), Err(Error::Denied));
    assert!(r
        .send(
            ProcessKey {
                pid: 1,
                generation: 2
            },
            a,
            b"stale"
        )
        .is_err());
    r.close(owner(1), a).unwrap();
    r.close(owner(1), b).unwrap();
    let newer = r.pair(owner(1)).unwrap();
    assert!(!newer.contains(&a));
    assert!(r.send(owner(1), a, b"old").is_err());
    assert!(r.close(owner(1), a).is_err());
}
#[test]
fn spawn_bindings_are_atomic_explicit_moves_with_rights_attenuation() {
    let mut r = Registry::new();
    let [a, b] = r.pair(owner(1)).unwrap();
    let original = r.handles(owner(1));
    for bindings in [
        vec![binding(a, 1, Rights::SEND), binding(b, 1, Rights::ALL)],
        vec![binding(a, 1, Rights::SEND), binding(a, 2, Rights::ALL)],
        vec![
            binding(a, 1, Rights::SEND),
            binding(Handle(9999), 2, Rights::ALL),
        ],
    ] {
        assert!(r.bindings(owner(1), owner(2), &bindings).is_err());
        assert_eq!(r.handles(owner(1)), original);
        assert_eq!(r.handles(owner(2)), 0);
        assert!(r.bootstrap(owner(2), 1).is_err());
    }
    r.bindings(owner(1), owner(2), &[binding(b, 7, Rights::SEND)])
        .unwrap();
    assert_eq!(r.bootstrap(owner(2), 7), Ok(b));
    assert!(r.bootstrap(owner(1), 7).is_err());
    assert_eq!(r.send(owner(1), b, b"parent alias"), Err(Error::Denied));
    assert!(r
        .bindings(owner(2), owner(3), &[binding(b, 1, Rights::ALL)])
        .is_err());
    r.send(owner(2), b, b"ready").unwrap();
    assert_eq!(receive(&mut r, 1).payload[..5], *b"ready");
    assert_eq!(r.handles(owner(1)), 1);
    assert_eq!(r.handles(owner(2)), 1);
}
#[test]
fn peek_copy_failure_does_not_consume_and_stale_ack_cannot_remove_new_message() {
    let mut r = Registry::new();
    let [a, b] = r.pair(owner(1)).unwrap();
    r.send(owner(1), a, b"one").unwrap();
    let pending = r.peek(owner(1)).unwrap().unwrap();
    assert_eq!(r.peek(owner(1)), Ok(Some(pending)));
    assert_eq!(r.consume(owner(2), pending), Err(Error::Denied));
    r.consume(owner(1), pending).unwrap();
    r.send(owner(1), a, b"two").unwrap();
    assert!(r.consume(owner(1), pending).is_err());
    let event = receive(&mut r, 1);
    assert_eq!(event.endpoint, b);
    assert_eq!(&event.payload[..3], b"two");
}
#[test]
fn peer_death_drains_messages_then_reports_close_once_and_frees_last_owner() {
    let mut r = Registry::new();
    let [a, b] = r.pair(owner(1)).unwrap();
    r.bindings(owner(1), owner(2), &[binding(b, 1, Rights::ALL)])
        .unwrap();
    r.send(owner(2), b, b"last").unwrap();
    r.close_owner(owner(2));
    assert_eq!(r.handles(owner(2)), 0);
    assert!(r.ready(owner(1)));
    assert_eq!(receive(&mut r, 1).kind, Kind::Message);
    assert_eq!(receive(&mut r, 1).kind, Kind::PeerClosed);
    assert!(
        !r.ready(owner(1)),
        "reported hangup must not create an infinite wake loop"
    );
    assert_eq!(r.send(owner(1), a, b"lost"), Err(Error::Closed));
    r.close_owner(owner(1));
    assert_eq!(r.channels(), 0);
}
#[test]
fn backpressure_has_explicit_one_shot_writable_interest() {
    let mut r = Registry::new();
    let [a, b] = r.pair(owner(1)).unwrap();
    r.bindings(owner(1), owner(2), &[binding(b, 1, Rights::ALL)])
        .unwrap();
    for _ in 0..MAX_MESSAGES {
        r.send(owner(1), a, b"x").unwrap();
    }
    r.interest(owner(1), a, true).unwrap();
    assert!(!r.ready(owner(1)));
    receive(&mut r, 2);
    assert!(r.ready(owner(1)));
    assert_eq!(receive(&mut r, 1).kind, Kind::Writable);
    assert!(!r.ready(owner(1)));
    r.send(owner(1), a, b"again").unwrap();
    assert!(!r.ready(owner(1)));
}
#[test]
fn per_owner_limits_leave_other_owners_capacity_and_repeated_cleanup_reuses_storage() {
    let mut r = Registry::new();
    for _ in 0..MAX_HANDLES / 2 {
        r.pair(owner(1)).unwrap();
    }
    assert_eq!(r.pair(owner(1)), Err(Error::Limit));
    r.pair(owner(2)).unwrap();
    r.close_owner(owner(1));
    r.close_owner(owner(2));
    assert_eq!(r.channels(), 0);
    for _ in 0..512 {
        r.pair(owner(1)).unwrap();
        r.close_owner(owner(1));
        assert_eq!(r.channels(), 0);
    }
}
#[test]
fn endpoint_scan_is_fair_across_a_flooding_channel_and_a_quiet_peer() {
    let mut r = Registry::new();
    let [a, _] = r.pair(owner(1)).unwrap();
    let [c, d] = r.pair(owner(1)).unwrap();
    for _ in 0..MAX_MESSAGES {
        r.send(owner(1), a, b"flood").unwrap();
    }
    r.send(owner(1), c, b"control").unwrap();
    let first = receive(&mut r, 1);
    r.send(owner(1), a, b"refill").unwrap();
    let second = receive(&mut r, 1);
    assert!(
        first.endpoint == d || second.endpoint == d,
        "busy lower index must not starve control"
    );
}

#[test]
fn global_channel_limit_recovers_without_stealing_another_owners_objects() {
    let mut r = Registry::new();
    for pid in [1, 2] {
        for _ in 0..MAX_HANDLES / 2 {
            r.pair(owner(pid)).unwrap();
        }
    }
    assert_eq!(r.channels(), MAX_CHANNELS);
    assert_eq!(r.pair(owner(3)), Err(Error::Limit));
    r.close_owner(owner(1));
    assert_eq!(r.handles(owner(2)), MAX_HANDLES);
    r.pair(owner(3)).unwrap();
    assert_eq!(r.handles(owner(3)), 2);
}
#[test]
fn queued_data_survives_an_explicit_receive_grant_but_old_owner_cannot_ack_it() {
    let mut r = Registry::new();
    let [a, b] = r.pair(owner(1)).unwrap();
    r.send(owner(1), a, b"before binding").unwrap();
    let old = r.peek(owner(1)).unwrap().unwrap();
    r.bindings(owner(1), owner(2), &[binding(b, 1, Rights::RECEIVE)])
        .unwrap();
    assert_eq!(r.consume(owner(1), old), Err(Error::Denied));
    assert_eq!(r.send(owner(2), b, b"forbidden"), Err(Error::Denied));
    let event = receive(&mut r, 2);
    assert_eq!(event.sender, Some(owner(1)));
    assert_eq!(&event.payload[..14], b"before binding");
}
#[test]
fn stale_writable_ack_cannot_clear_a_rearmed_interest() {
    let mut r = Registry::new();
    let [a, _] = r.pair(owner(1)).unwrap();
    r.interest(owner(1), a, true).unwrap();
    let old = r.peek(owner(1)).unwrap().unwrap();
    r.interest(owner(1), a, false).unwrap();
    r.interest(owner(1), a, true).unwrap();
    assert!(r.consume(owner(1), old).is_err());
    assert!(r.ready(owner(1)));
    assert_eq!(receive(&mut r, 1).kind, Kind::Writable);
    assert!(!r.ready(owner(1)));
}

#[test]
fn paused_receive_interest_keeps_data_queued_without_starving_control_or_losing_eof() {
    let mut r = Registry::new();
    let [data, peer] = r.pair(owner(1)).unwrap();
    let [control, command] = r.pair(owner(1)).unwrap();
    r.bindings(
        owner(1),
        owner(2),
        &[
            binding(peer, 7, Rights::ALL),
            binding(command, 8, Rights::RECEIVE),
        ],
    )
    .unwrap();
    r.set_interest(owner(2), peer, false, false).unwrap();
    for _ in 0..MAX_MESSAGES {
        r.send(owner(1), data, b"queued").unwrap();
    }
    assert!(
        !r.ready(owner(2)),
        "paused receive interest cannot busy-wake"
    );
    r.send(owner(1), control, b"go").unwrap();
    assert_eq!(receive(&mut r, 2).endpoint, command);
    r.close(owner(1), data).unwrap();
    assert!(
        !r.ready(owner(2)),
        "EOF follows the retained unread payloads"
    );
    r.set_interest(owner(2), peer, true, false).unwrap();
    for _ in 0..MAX_MESSAGES {
        assert_eq!(receive(&mut r, 2).kind, Kind::Message);
    }
    assert_eq!(receive(&mut r, 2).kind, Kind::PeerClosed);
    assert!(!r.ready(owner(2)));
}
