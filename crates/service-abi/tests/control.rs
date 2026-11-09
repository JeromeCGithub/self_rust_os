use rustos_service_abi::{
    control::{Operation, Request},
    MAX_FRAME,
};
#[test]
fn management_requests_roundtrip_without_allocation_or_shell_expansion() {
    for operation in [
        Operation::Status { index: 63 },
        Operation::Logs {
            cursor: u64::MAX,
            offset: 232,
        },
        Operation::Start("graphical.session"),
        Operation::Stop("boot"),
        Operation::Restart("beacon"),
        Operation::Shutdown,
    ] {
        let expected = Request { id: 19, operation };
        let mut bytes = [255; MAX_FRAME];
        let n = expected
            .encode(&mut bytes)
            .expect("valid management request");
        assert_eq!(Request::decode(&bytes[..n]), Ok(expected));
        assert!(bytes[n..].iter().all(|&b| b == 0));
        for end in 0..n {
            assert!(Request::decode(&bytes[..end]).is_err());
        }
        let mut extra = bytes[..n].to_vec();
        extra.push(0);
        assert!(Request::decode(&extra).is_err());
    }
}
#[test]
fn control_frames_reject_bad_ids_names_offsets_reserved_bytes_and_notification_kinds() {
    let mut bytes = [0; MAX_FRAME];
    for name in [
        "",
        "Bad",
        "../boot",
        "a\n",
        "a/b",
        "a b",
        "café",
        &"a".repeat(49),
    ] {
        assert!(Request {
            id: 1,
            operation: Operation::Start(name)
        }
        .encode(&mut bytes)
        .is_err());
    }
    assert!(Request {
        id: 0,
        operation: Operation::Shutdown
    }
    .encode(&mut bytes)
    .is_err());
    assert!(Request {
        id: 1,
        operation: Operation::Status { index: 64 }
    }
    .encode(&mut bytes)
    .is_err());
    assert!(Request {
        id: 1,
        operation: Operation::Logs {
            cursor: 0,
            offset: 233
        }
    }
    .encode(&mut bytes)
    .is_err());
    let n = Request {
        id: 1,
        operation: Operation::Shutdown,
    }
    .encode(&mut bytes)
    .unwrap();
    for position in [0, 4, 8, 16, 20] {
        let mut bad = bytes;
        bad[position] = 255;
        if position != 8 {
            assert!(Request::decode(&bad[..n]).is_err(), "position {position}");
        }
    }
    let mut bad = bytes;
    bad[8..16].fill(0);
    assert!(Request::decode(&bad[..n]).is_err());
    let n = rustos_service_abi::Message::Ready { run: 1 }
        .encode(&mut bytes)
        .unwrap();
    assert!(Request::decode(&bytes[..n]).is_err());
}
