use rustos_ipc_abi::*;
fn owner() -> ProcessKey {
    ProcessKey {
        pid: 42,
        generation: 1,
    }
}
#[test]
fn message_roundtrip_authenticates_kernel_supplied_identity_and_zeroes_tail() {
    for length in [0, 1, MAX_PAYLOAD] {
        let data = vec![0x87; length];
        let event = Event::message(Handle(7), owner(), &data).unwrap();
        let bytes = event.encode().unwrap();
        assert_eq!(bytes.len(), 288);
        assert_eq!(&bytes[..8], &[1, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&bytes[8..16], &7u64.to_le_bytes());
        assert_eq!(&bytes[16..24], &owner().encode().to_le_bytes());
        assert_eq!(&bytes[24..28], &(length as u32).to_le_bytes());
        assert!(bytes[28..32]
            .iter()
            .chain(&bytes[32 + length..])
            .all(|&b| b == 0));
        assert_eq!(Event::decode(&bytes), Ok(event));
    }
    assert!(Event::message(Handle(7), owner(), &[0; MAX_PAYLOAD + 1]).is_err());
    assert!(Event::message(Handle(0), owner(), b"x").is_err());
}
#[test]
fn peer_close_and_writable_signals_never_fabricate_a_sender_or_payload() {
    for (kind, tag) in [(Kind::PeerClosed, 2), (Kind::Writable, 3)] {
        let event = Event::signal(Handle(6), kind).unwrap();
        let bytes = event.encode().unwrap();
        assert_eq!(bytes[4], tag);
        assert!(bytes[16..].iter().all(|&b| b == 0));
        assert_eq!(Event::decode(&bytes), Ok(event));
    }
    assert!(Event::signal(Handle(7), Kind::Message).is_err());
}
#[test]
fn malformed_wire_never_looks_like_an_authenticated_event() {
    let good = Event::message(Handle(7), owner(), b"x")
        .unwrap()
        .encode()
        .unwrap();
    for size in [0, EVENT_SIZE - 1, EVENT_SIZE + 1] {
        assert!(Event::decode(&vec![0; size]).is_err());
    }
    for (offset, value) in [(0, 2), (4, 9), (8, 0), (20, 0), (25, 255), (28, 1), (33, 1)] {
        let mut bytes = good;
        bytes[offset] = value;
        assert!(Event::decode(&bytes).is_err(), "offset {offset}");
    }
    let mut signal = Event::signal(Handle(7), Kind::PeerClosed)
        .unwrap()
        .encode()
        .unwrap();
    signal[16] = 42;
    assert!(Event::decode(&signal).is_err());
    let mut event = Event::message(Handle(7), owner(), b"x").unwrap();
    event.sender = None;
    assert!(event.encode().is_err());
}
#[test]
fn binding_frames_reject_unknown_rights_reserved_fields_and_zero_tags() {
    let binding = Binding {
        source: Handle(55),
        tag: 1,
        rights: Rights::SEND,
    };
    let bytes = binding.encode().unwrap();
    assert_eq!(&bytes[..8], &55u64.to_le_bytes());
    assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
    assert_eq!(&bytes[12..16], &1u32.to_le_bytes());
    assert_eq!(Binding::decode(&bytes), Ok(binding));
    for (offset, value) in [(0, 0), (8, 0), (12, 0), (12, 8), (16, 1)] {
        let mut corrupt = bytes;
        corrupt[offset] = value;
        assert!(Binding::decode(&corrupt).is_err(), "offset {offset}");
    }
    for bits in 1..=7 {
        assert_eq!(Rights::from_bits(bits).unwrap().bits(), bits);
    }
    for bits in [0, 8, u32::MAX] {
        assert!(Rights::from_bits(bits).is_err());
    }
    assert!(Rights::ALL.contains(Rights::SEND));
    assert!(!Rights::SEND.contains(Rights::TRANSFER));
}

#[test]
fn bound_spawn_v2_has_a_distinct_frame_and_keeps_v1_layout_unchanged() {
    let request = Spawn {
        argv: 0x1000,
        argc: 2,
        envp: 0x2000,
        envc: 1,
        bindings: 0x3000,
        binding_count: 2,
    };
    let bytes = request.encode().unwrap();
    let fields: Vec<u64> = bytes
        .chunks_exact(8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .collect();
    assert_eq!(fields, vec![2, 0, 0x1000, 2, 0x2000, 1, 0x3000, 2]);
    assert_eq!(Spawn::decode(&bytes), Ok(request));
    assert_eq!(core::mem::size_of::<rustos_process_abi::SpawnRequest>(), 48);
    assert_ne!(SYS_SPAWN_BOUND, rustos_process_abi::SYS_SPAWN_ENV);
}
#[test]
fn unsupported_spawn_flags_counts_and_frame_lengths_fail_before_binding_work() {
    let request = Spawn {
        argv: 0,
        argc: 0,
        envp: 0,
        envc: 0,
        bindings: 0,
        binding_count: 0,
    };
    let bytes = request.encode().unwrap();
    for size in [0, SPAWN_SIZE - 1, SPAWN_SIZE + 1] {
        assert!(Spawn::decode(&vec![0; size]).is_err());
    }
    for (offset, value) in [
        (0, 1u64),
        (8, 1),
        (24, 4097),
        (40, 4097),
        (56, MAX_BINDINGS as u64 + 1),
    ] {
        let mut bad = bytes;
        bad[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        assert!(Spawn::decode(&bad).is_err(), "field {offset}");
    }
    let huge = Spawn {
        binding_count: MAX_BINDINGS as u64 + 1,
        ..request
    };
    assert!(huge.encode().is_err());
}
