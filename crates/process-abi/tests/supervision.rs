use rustos_process_abi::supervision::*;
#[test]
fn complete_wire_records_round_trip_without_rust_layout_assumptions() {
    let process = ProcessKey {
        pid: 42,
        generation: 1,
    };
    for event in [
        ChildEvent::Adopted(process),
        ChildEvent::Completed {
            process,
            cause: Cause::Exited,
            status: u64::MAX,
        },
        ChildEvent::Completed {
            process,
            cause: Cause::Killed,
            status: 9,
        },
        ChildEvent::Completed {
            process,
            cause: Cause::Fault,
            status: 14,
        },
    ] {
        let bytes = event.encode();
        assert_eq!(bytes.len(), 40);
        assert_eq!(bytes[0..4], 1u32.to_le_bytes());
        assert_eq!(bytes[8..12], 42u32.to_le_bytes());
        assert_eq!(bytes[12..16], 1u32.to_le_bytes());
        assert_eq!(ChildEvent::decode(&bytes), Some(event));
    }
}
#[test]
fn malformed_tags_generations_and_reserved_fields_fail_closed() {
    let valid = ChildEvent::Completed {
        process: ProcessKey {
            pid: 7,
            generation: 1,
        },
        cause: Cause::Exited,
        status: 0,
    }
    .encode();
    for (offset, value) in [
        (0, 2),
        (4, 0),
        (4, 3),
        (12, 2),
        (16, 3),
        (20, 1),
        (32, 1),
        (39, 255),
    ] {
        let mut bytes = valid;
        bytes[offset] = value;
        assert_eq!(ChildEvent::decode(&bytes), None, "offset {}", offset);
    }
    let mut adopted = ChildEvent::Adopted(ProcessKey {
        pid: 7,
        generation: 1,
    })
    .encode();
    adopted[24] = 1;
    assert_eq!(ChildEvent::decode(&adopted), None);
    for value in [
        0,
        7,
        u64::MAX,
        (2u64 << 32) | 7,
        (1u64 << 32) | u32::MAX as u64,
    ] {
        assert_eq!(ProcessKey::decode(value), None);
    }
}
