use rustos_ipc_abi::ProcessKey;
use rustos_service_abi::{control::Access, replies::*, Level, MAX_FRAME};
const KEY: ProcessKey = ProcessKey {
    pid: 2,
    generation: 1,
};
#[test]
fn reply_pages_roundtrip_and_reject_all_truncation_and_trailing_data() {
    let long = "é".repeat(LOG_CHUNK / 2);
    for body in [
        Body::Hello {
            access: Access::ReadOnly,
            run: 9,
        },
        Body::Job {
            job: 3,
            phase: JobPhase::Degraded,
        },
        Body::Status {
            index: 63,
            name: "nest.session",
            phase: Phase::Starting,
            run: 9,
            process: Some(KEY),
            deadline: Some(40),
        },
        Body::Status {
            index: 0,
            name: "boot",
            phase: Phase::GroupActive,
            run: 0,
            process: None,
            deadline: None,
        },
        Body::Log {
            sequence: 3,
            unit: 1,
            run: 9,
            process: KEY,
            tick: 17,
            level: Level::Warning,
            offset: 0,
            total: 232,
            text: &long,
        },
        Body::Log {
            sequence: 3,
            unit: 1,
            run: 9,
            process: KEY,
            tick: 17,
            level: Level::Info,
            offset: 184,
            total: 232,
            text: &"x".repeat(48),
        },
        Body::End,
        Body::Error {
            code: Code::Gap,
            detail: 15,
        },
    ] {
        let expected = Reply {
            id: if matches!(body, Body::Hello { .. }) {
                0
            } else {
                7
            },
            body,
        };
        let mut bytes = [255; MAX_FRAME];
        let n = expected.encode(&mut bytes).expect("valid reply");
        assert_eq!(Reply::decode(&bytes[..n]), Ok(expected));
        assert!(bytes[n..].iter().all(|&b| b == 0));
        for end in 0..n {
            assert!(Reply::decode(&bytes[..end]).is_err(), "end={end}");
        }
        let mut extra = bytes[..n].to_vec();
        extra.push(0);
        assert!(Reply::decode(&extra).is_err());
    }
}
#[test]
fn invalid_metadata_never_encodes_as_a_trusted_snapshot() {
    let mut bytes = [0; MAX_FRAME];
    for reply in [
        Reply {
            id: 1,
            body: Body::Hello {
                access: Access::Manage,
                run: 1,
            },
        },
        Reply {
            id: 0,
            body: Body::End,
        },
        Reply {
            id: 1,
            body: Body::Job {
                job: 0,
                phase: JobPhase::Running,
            },
        },
        Reply {
            id: 1,
            body: Body::Status {
                index: 64,
                name: "boot",
                phase: Phase::Inactive,
                run: 0,
                process: None,
                deadline: None,
            },
        },
        Reply {
            id: 1,
            body: Body::Status {
                index: 0,
                name: "../boot",
                phase: Phase::Inactive,
                run: 0,
                process: None,
                deadline: None,
            },
        },
        Reply {
            id: 1,
            body: Body::Log {
                sequence: 0,
                unit: 1,
                run: 1,
                process: KEY,
                tick: 1,
                level: Level::Info,
                offset: 0,
                total: 1,
                text: "x",
            },
        },
        Reply {
            id: 1,
            body: Body::Log {
                sequence: 1,
                unit: 1,
                run: 1,
                process: KEY,
                tick: 1,
                level: Level::Info,
                offset: 0,
                total: 2,
                text: "",
            },
        },
        Reply {
            id: 1,
            body: Body::Log {
                sequence: 1,
                unit: 1,
                run: 1,
                process: KEY,
                tick: 1,
                level: Level::Info,
                offset: 231,
                total: 232,
                text: "xx",
            },
        },
    ] {
        assert!(reply.encode(&mut bytes).is_err());
    }
    let n = Reply {
        id: 1,
        body: Body::End,
    }
    .encode(&mut bytes)
    .unwrap();
    for at in [0, 4, 16, 20] {
        let mut bad = bytes;
        bad[at] = 255;
        assert!(Reply::decode(&bad[..n]).is_err());
    }
}
