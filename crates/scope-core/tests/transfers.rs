use rustos_scope_core::{Error, ProcessKey, Registry, Usage, SYSTEM};
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn units(n: u64) -> Usage {
    Usage {
        processes: n,
        threads: n,
        pages: n,
        kernel_bytes: n,
        descriptors: n,
        ipc_handles: n,
        sockets: n,
        audio_streams: n,
    }
}
fn tree() -> (Registry, u64, u64, u64) {
    let mut r = Registry::new();
    r.bootstrap(key(1), units(100), units(10)).unwrap();
    let session = r.create_session(key(1), 1, units(80)).unwrap();
    r.appoint_leader(key(1), session, key(2)).unwrap();
    let a = r.create_service(key(2), session, units(60)).unwrap();
    let b = r.create_service(key(2), session, units(60)).unwrap();
    (r, session, a, b)
}
#[test]
fn transfers_at_shared_ancestor_limits_do_not_double_charge() {
    let (mut r, session, a, b) = tree();
    let mut source = r.reserve(a, units(60)).unwrap();
    let mut target = r.reserve(b, units(20)).unwrap();
    let recovery = r.reserve(SYSTEM, units(20)).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, units(100));
    r.can_transfer(&source, &target, units(20)).unwrap();
    r.transfer(&mut source, &mut target, units(20)).unwrap();
    assert_eq!(source.used(), units(40));
    assert_eq!(target.used(), units(40));
    assert_eq!(r.status(key(2), session).unwrap().used, units(80));
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, units(100));
    // Same-scope handoff changes ownership, not aggregate resource use.
    let mut same = r.reserve(a, Usage::ZERO).unwrap();
    r.transfer(&mut source, &mut same, units(40)).unwrap();
    assert_eq!(source.used(), Usage::ZERO);
    r.release(source).unwrap();
    r.release(same).unwrap();
    r.release(target).unwrap();
    r.release(recovery).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}
#[test]
fn denial_preserves_both_owners_and_all_ancestor_counters() {
    let (mut r, session, a, b) = tree();
    let mut source = r.reserve(a, units(60)).unwrap();
    let mut target = r.reserve(b, units(20)).unwrap();
    for delta in [units(61), units(u64::MAX), units(41)] {
        assert!(r.can_transfer(&source, &target, delta).is_err());
        assert!(r.transfer(&mut source, &mut target, delta).is_err());
        assert_eq!(source.used(), units(60));
        assert_eq!(target.used(), units(20));
        assert_eq!(r.status(key(2), a).unwrap().used, units(60));
        assert_eq!(r.status(key(2), b).unwrap().used, units(20));
        assert_eq!(r.status(key(2), session).unwrap().used, units(80));
    }
    r.close(key(2), b).unwrap();
    assert_eq!(
        r.transfer(&mut source, &mut target, units(1)),
        Err(Error::Closed)
    );
    r.release(target).unwrap();
    let mut root = r.reserve(SYSTEM, Usage::ZERO).unwrap();
    r.close(key(2), a).unwrap();
    assert_eq!(
        r.transfer(&mut source, &mut root, units(1)),
        Err(Error::Closed)
    );
    r.release(source).unwrap();
    r.release(root).unwrap();
}
#[test]
fn moving_from_system_to_session_checks_recovery_and_session_limits() {
    let (mut r, session, a, b) = tree();
    let mut source = r.reserve(SYSTEM, units(100)).unwrap();
    let mut target = r.reserve(a, Usage::ZERO).unwrap();
    assert_eq!(
        r.transfer(&mut source, &mut target, units(1)),
        Err(Error::Limit)
    );
    r.release_part(&mut source, units(10)).unwrap();
    r.transfer(&mut source, &mut target, units(60)).unwrap();
    let mut other = r.reserve(b, Usage::ZERO).unwrap();
    assert_eq!(
        r.transfer(&mut source, &mut other, units(21)),
        Err(Error::Limit)
    );
    r.transfer(&mut source, &mut other, units(20)).unwrap();
    assert_eq!(r.status(key(2), session).unwrap().used, units(80));
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, units(90));
    r.transfer(&mut other, &mut source, units(20)).unwrap();
    r.release(source).unwrap();
    r.release(target).unwrap();
    r.release(other).unwrap();
}
#[test]
fn unpublished_receiver_rolls_back_before_transfer_and_commits_once_after_validation() {
    let (mut r, session, a, b) = tree();
    let mut source = r.reserve(a, units(20)).unwrap();
    {
        let mut pending = r.transaction(b, units(10)).unwrap();
        pending.appoint_leader(key(2), key(3)).unwrap();
        pending.can_receive(&source, units(20)).unwrap();
        // A failed native object move returns without committing the guard.
    }
    assert_eq!(source.used(), units(20));
    assert_eq!(r.status(key(2), b).unwrap().used, Usage::ZERO);
    assert_eq!(r.status(key(2), b).unwrap().leader, None);
    let pending = r.transaction(b, units(10)).unwrap();
    pending.can_receive(&source, units(20)).unwrap();
    let target = pending.commit_receiving(&mut source, units(20)).unwrap();
    assert_eq!(source.used(), Usage::ZERO);
    assert_eq!(target.used(), units(30));
    assert_eq!(r.status(key(2), session).unwrap().used, units(30));
    r.release(source).unwrap();
    r.release(target).unwrap();
}

#[test]
fn cross_session_move_checks_destination_aggregate_and_preserves_global_capacity() {
    let (mut r, session, a, _) = tree();
    let mut source = r.reserve(a, units(60)).unwrap();
    let other = r.create_session(key(1), 1, units(40)).unwrap();
    r.appoint_leader(key(1), other, key(3)).unwrap();
    let b = r.create_service(key(3), other, units(40)).unwrap();
    let c = r.create_service(key(3), other, units(40)).unwrap();
    let mut target = r.reserve(b, Usage::ZERO).unwrap();
    let sibling = r.reserve(c, units(30)).unwrap();
    let recovery = r.reserve(SYSTEM, units(10)).unwrap();
    assert_eq!(
        r.transfer(&mut source, &mut target, units(11)),
        Err(Error::Limit)
    );
    assert_eq!(source.used(), units(60));
    assert_eq!(target.used(), Usage::ZERO);
    assert_eq!(r.status(key(2), session).unwrap().used, units(60));
    assert_eq!(r.status(key(3), other).unwrap().used, units(30));
    r.transfer(&mut source, &mut target, units(10)).unwrap();
    assert_eq!(r.status(key(2), session).unwrap().used, units(50));
    assert_eq!(r.status(key(3), other).unwrap().used, units(40));
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, units(100));
    r.release(source).unwrap();
    r.release(target).unwrap();
    r.release(sibling).unwrap();
    r.release(recovery).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}

#[test]
fn failed_commit_returns_the_reservation_for_resource_rollback_before_quota_release() {
    let (mut r, _, a, b) = tree();
    let mut source = r.reserve(a, units(20)).unwrap();
    let mut pending = r.transaction(b, units(10)).unwrap();
    pending.appoint_leader(key(2), key(3)).unwrap();
    let (error, pending) = pending
        .commit_receiving(&mut source, units(21))
        .unwrap_err();
    assert_eq!(error, Error::Invalid);
    assert_eq!(source.used(), units(20));
    assert_eq!(
        pending.available().unwrap(),
        units(50),
        "destination quota remains held until real construction rollback completes"
    );
    // Native callers destroy mappings/buffers before returning this guard.
    drop(pending);
    assert_eq!(r.status(key(2), b).unwrap().used, Usage::ZERO);
    assert_eq!(r.status(key(2), b).unwrap().leader, None);
    r.release(source).unwrap();
}
