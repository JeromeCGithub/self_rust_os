use rustos_scope_core::{Charge, Error, ProcessKey, Registry, Usage, SYSTEM};
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn pages(pages: u64) -> Usage {
    Usage {
        pages,
        ..Usage::ZERO
    }
}
fn kernel() -> Registry {
    let mut r = Registry::new();
    r.bootstrap(key(1), pages(100), pages(10)).unwrap();
    r
}
#[test]
fn failed_publication_rolls_back_usage_and_unpublished_leader_binding() {
    let mut r = kernel();
    let scope = r.create_service(key(1), SYSTEM, pages(20)).unwrap();
    fn fails(r: &mut Registry, scope: u64) -> Result<Charge, Error> {
        let mut pending = r.transaction(scope, pages(3))?;
        assert_eq!(pending.available(), Ok(pages(17)));
        pending.grow(pages(2))?;
        pending.appoint_leader(key(1), key(2))?;
        // Simulate a final fallible grant-move failure, after all allocations.
        Err(Error::Denied)
    }
    for _ in 0..128 {
        assert_eq!(fails(&mut r, scope).err(), Some(Error::Denied));
        let view = r.status(key(1), scope).unwrap();
        assert_eq!(view.used, Usage::ZERO);
        assert_eq!(view.leader, None);
    }
    let mut pending = r.transaction(scope, pages(5)).unwrap();
    pending.appoint_leader(key(1), key(2)).unwrap();
    let charge = pending.commit();
    assert_eq!(r.status(key(1), scope).unwrap().used, pages(5));
    assert_eq!(r.status(key(1), scope).unwrap().leader, Some(key(2)));
    r.close(key(1), scope).unwrap();
    assert!(r.next_closed(key(1)).is_none());
    r.release(charge).unwrap();
    r.ack_closed(key(1), scope).unwrap();
}
#[test]
fn failed_growth_releases_only_its_delta_and_success_retains_it() {
    let mut r = kernel();
    let scope = r.create_service(key(1), SYSTEM, pages(20)).unwrap();
    let mut charge = r.reserve(scope, pages(3)).unwrap();
    for _ in 0..128 {
        let mut pending = r
            .growth(&mut charge, pages(4))
            .expect("reserve growth before allocation");
        pending.grow(pages(2)).unwrap();
        assert_eq!(pending.available(), Ok(pages(11)));
        assert_eq!(pending.grow(pages(20)), Err(Error::Limit));
        drop(pending);
        assert_eq!(charge.used(), pages(3));
        assert_eq!(r.status(key(1), SYSTEM).unwrap().used, pages(3));
    }
    let pending = r.growth(&mut charge, pages(4)).unwrap();
    pending.commit();
    assert_eq!(charge.used(), pages(7));
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, pages(7));
    r.release(charge).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}
#[test]
fn failed_appointment_or_quota_does_not_publish_partial_state() {
    let mut r = kernel();
    let scope = r.create_service(key(1), SYSTEM, pages(5)).unwrap();
    assert_eq!(r.transaction(scope, pages(6)).err(), Some(Error::Limit));
    let mut pending = r.transaction(scope, pages(1)).unwrap();
    assert_eq!(pending.appoint_leader(key(9), key(2)), Err(Error::Denied));
    pending.appoint_leader(key(1), key(2)).unwrap();
    assert_eq!(pending.appoint_leader(key(1), key(3)), Err(Error::Invalid));
    assert_eq!(pending.grow(pages(5)), Err(Error::Limit));
    drop(pending);
    assert_eq!(r.status(key(1), scope).unwrap().used, Usage::ZERO);
    assert_eq!(r.status(key(1), scope).unwrap().leader, None);
    r.close(key(1), scope).unwrap();
    assert_eq!(r.transaction(scope, pages(1)).err(), Some(Error::Closed));
}
