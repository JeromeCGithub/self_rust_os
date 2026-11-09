use rustos_service_abi::control::{Access, Operation, Request};
use rustos_service_core::{
    control::{Denied, Session},
    ProcessKey,
};
const PEER: ProcessKey = ProcessKey {
    pid: 7,
    generation: 1,
};
fn request(id: u64, operation: Operation<'_>) -> Request<'_> {
    Request { id, operation }
}
#[test]
fn readonly_grant_can_observe_but_cannot_start_stop_restart_or_shutdown() {
    let mut session = Session::new(PEER, 4, Access::ReadOnly).unwrap();
    assert_eq!(
        session.admit(PEER, 4, request(1, Operation::Status { index: 0 })),
        Ok(())
    );
    assert_eq!(
        session.admit(
            PEER,
            4,
            request(
                2,
                Operation::Logs {
                    cursor: 0,
                    offset: 0
                }
            )
        ),
        Ok(())
    );
    for (offset, operation) in [
        Operation::Start("boot"),
        Operation::Stop("boot"),
        Operation::Restart("boot"),
        Operation::Shutdown,
    ]
    .into_iter()
    .enumerate()
    {
        let id = offset as u64 + 3;
        assert_eq!(
            session.admit(PEER, 4, request(id, operation)),
            Err(Denied::Permission)
        );
        assert_eq!(
            session.admit(PEER, 4, request(id, Operation::Status { index: 0 })),
            Err(Denied::Replay)
        );
    }
}
#[test]
fn management_authority_is_bound_to_exact_peer_generation_and_run_before_effects() {
    let mut session = Session::new(PEER, 4, Access::Manage).unwrap();
    let cmd = request(1, Operation::Stop("boot"));
    for peer in [
        ProcessKey {
            pid: 8,
            generation: 1,
        },
        ProcessKey {
            pid: 7,
            generation: 2,
        },
    ] {
        assert_eq!(session.admit(peer, 4, cmd), Err(Denied::Peer));
    }
    assert_eq!(session.admit(PEER, 3, cmd), Err(Denied::Run));
    // Forged attempts cannot consume the real peer's request ID.
    assert_eq!(session.admit(PEER, 4, cmd), Ok(()));
    assert_eq!(session.admit(PEER, 4, cmd), Err(Denied::Replay));
    for (offset, op) in [
        Operation::Start("boot"),
        Operation::Restart("boot"),
        Operation::Shutdown,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            session.admit(PEER, 4, request(offset as u64 + 2, op)),
            Ok(())
        );
    }
}
#[test]
fn malformed_in_memory_requests_cannot_bypass_decoder_or_exhaust_sequence() {
    let mut session = Session::new(PEER, 1, Access::Manage).unwrap();
    for cmd in [
        request(0, Operation::Shutdown),
        request(u64::MAX, Operation::Start("../boot")),
        request(u64::MAX, Operation::Status { index: 64 }),
    ] {
        assert_eq!(session.admit(PEER, 1, cmd), Err(Denied::Malformed));
    }
    assert_eq!(
        session.admit(PEER, 1, request(1, Operation::Start("boot"))),
        Ok(())
    );
    assert_eq!(
        session.admit(PEER, 1, request(u64::MAX, Operation::Stop("boot"))),
        Ok(())
    );
    assert_eq!(
        session.admit(PEER, 1, request(1, Operation::Start("boot"))),
        Err(Denied::Replay)
    );
    assert_eq!(
        session.admit(PEER, 1, request(u64::MAX, Operation::Stop("boot"))),
        Err(Denied::Replay)
    );
}
#[test]
fn replacing_connection_never_inherits_replay_state_or_old_authority() {
    assert!(Session::new(
        ProcessKey {
            pid: 7,
            generation: 0
        },
        1,
        Access::Manage
    )
    .is_err());
    assert!(Session::new(
        ProcessKey {
            pid: u32::MAX,
            generation: 1
        },
        1,
        Access::Manage
    )
    .is_err());
    assert!(Session::new(PEER, 0, Access::Manage).is_err());
    let mut replacement = Session::new(PEER, 5, Access::ReadOnly).unwrap();
    let cmd = request(1, Operation::Stop("boot"));
    assert_eq!(replacement.admit(PEER, 4, cmd), Err(Denied::Run));
    assert_eq!(replacement.admit(PEER, 5, cmd), Err(Denied::Permission));
    assert_eq!(
        replacement.admit(PEER, 5, request(2, Operation::Status { index: 0 })),
        Ok(())
    );
}
