use rustos_scope_core::{
    CloseCause, Error, Kind, ProcessKey, Registry, State, Usage, MAX_SCOPES, MAX_SESSION_SERVICES,
    RECOVERY_SCOPES, SYSTEM,
};
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn budget(n: u64) -> Usage {
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
fn kernel() -> Registry {
    let mut registry = Registry::new();
    registry.bootstrap(key(1), budget(100), budget(10)).unwrap();
    registry
}
fn session(registry: &mut Registry) -> u64 {
    let id = registry
        .create_session(key(1), 1, budget(80))
        .expect("create native session");
    registry.appoint_leader(key(1), id, key(2)).unwrap();
    id
}
#[test]
fn explicit_factories_cannot_mint_or_cross_session_authority() {
    let mut r = kernel();
    assert_eq!(r.create_session(key(2), 1, budget(1)), Err(Error::Denied));
    for user in [0, u32::MAX] {
        assert_eq!(
            r.create_session(key(1), user, budget(1)),
            Err(Error::Invalid)
        );
    }
    let a = session(&mut r);
    let b = r.create_session(key(1), 1, budget(80)).unwrap();
    r.appoint_leader(key(1), b, key(3)).unwrap();
    assert_ne!(a, b);
    let av = r.status(key(2), a).unwrap();
    let bv = r.status(key(3), b).unwrap();
    assert_eq!(av.kind, Kind::Session);
    assert_eq!(av.user, 1);
    assert_eq!(bv.user, 1);
    assert_ne!(
        av.session, bv.session,
        "same user still has independent sessions"
    );
    assert_eq!(r.create_service(key(2), b, budget(1)), Err(Error::Denied));
    assert_eq!(r.status(key(3), a), Err(Error::Denied));
    assert_eq!(
        r.close(key(2), a),
        Err(Error::Denied),
        "manager cannot close its parent's session grant"
    );
    let service = r.create_service(key(2), a, budget(40)).unwrap();
    let context = r.status(key(2), service).unwrap();
    assert_eq!(
        (context.user, context.session, context.parent),
        (1, a, Some(a))
    );
    assert_eq!(
        r.create_service(key(2), service, budget(1)),
        Err(Error::Denied),
        "no arbitrary recursive factories"
    );
    assert_eq!(
        r.create_service(key(1), SYSTEM, budget(40)).map(|_| ()),
        Ok(())
    );
    assert_eq!(r.appoint_leader(key(2), a, key(99)), Err(Error::Denied));
}
#[test]
fn reservations_are_atomic_across_every_resource_and_ancestor() {
    let mut r = kernel();
    let root = session(&mut r);
    let a = r.create_service(key(2), root, budget(60)).unwrap();
    let b = r.create_service(key(2), root, budget(60)).unwrap();
    let mut owned = r.reserve(a, budget(40)).unwrap();
    let other = r.reserve(b, budget(40)).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, budget(80));
    assert_eq!(
        r.grow(&mut owned, budget(1)),
        Err(Error::Limit),
        "session budget is shared"
    );
    assert_eq!(owned.used(), budget(40));
    assert_eq!(r.status(key(2), a).unwrap().used, budget(40));
    r.release(other).unwrap();
    r.grow(&mut owned, budget(20)).unwrap();
    assert_eq!(
        r.grow(&mut owned, budget(1)),
        Err(Error::Limit),
        "leaf budget is checked too"
    );
    assert_eq!(r.release_part(&mut owned, budget(61)), Err(Error::Invalid));
    r.release_part(&mut owned, budget(20)).unwrap();
    r.release(owned).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}
#[test]
fn each_resource_bound_and_arithmetic_overflow_leave_charges_unchanged() {
    let mut r = kernel();
    let root = session(&mut r);
    let node = r.create_service(key(2), root, budget(10)).unwrap();
    let mut c = r.reserve(node, budget(1)).unwrap();
    for amount in [10, u64::MAX] {
        for extra in [
            Usage {
                processes: amount,
                ..Usage::ZERO
            },
            Usage {
                threads: amount,
                ..Usage::ZERO
            },
            Usage {
                pages: amount,
                ..Usage::ZERO
            },
            Usage {
                kernel_bytes: amount,
                ..Usage::ZERO
            },
            Usage {
                descriptors: amount,
                ..Usage::ZERO
            },
            Usage {
                ipc_handles: amount,
                ..Usage::ZERO
            },
            Usage {
                sockets: amount,
                ..Usage::ZERO
            },
            Usage {
                audio_streams: amount,
                ..Usage::ZERO
            },
            budget(u64::MAX),
        ] {
            assert_eq!(r.grow(&mut c, extra), Err(Error::Limit));
            assert_eq!(r.status(key(2), node).unwrap().used, budget(1));
            assert_eq!(c.used(), budget(1));
        }
    }
    r.release(c).unwrap();
}
#[test]
fn user_pressure_preserves_system_resource_and_scope_recovery_capacity() {
    let mut r = kernel();
    let root = r.create_session(key(1), 1, budget(90)).unwrap();
    r.appoint_leader(key(1), root, key(2)).unwrap();
    let user = r.reserve(root, budget(90)).unwrap();
    assert_eq!(r.reserve(root, budget(1)).err(), Some(Error::Limit));
    let recovery = r
        .reserve(SYSTEM, budget(10))
        .expect("init retains reserved capacity");
    assert_eq!(r.reserve(SYSTEM, budget(1)).err(), Some(Error::Limit));
    r.release(user).unwrap();
    r.release(recovery).unwrap();
    while r.create_service(key(2), root, Usage::ZERO).is_ok() {}
    // Fill the global user allowance across independently admitted sessions;
    // one session can no longer consume every non-recovery record by itself.
    for pid in 3..MAX_SCOPES as u32 {
        let Ok(next) = r.create_session(key(1), 1, budget(90)) else {
            break;
        };
        r.appoint_leader(key(1), next, key(pid)).unwrap();
        while r.create_service(key(pid), next, Usage::ZERO).is_ok() {}
    }
    // System root + session roots + leaves stop below the recovery floor.
    let mut extra = 0;
    while r.create_service(key(1), SYSTEM, Usage::ZERO).is_ok() {
        extra += 1;
    }
    assert_eq!(extra, RECOVERY_SCOPES);
    assert!(MAX_SCOPES > RECOVERY_SCOPES);
}
#[test]
fn close_waits_for_actual_reclamation_and_acknowledges_children_before_parent() {
    let mut r = kernel();
    let root = session(&mut r);
    let node = r.create_service(key(2), root, budget(20)).unwrap();
    let mut charge = r.reserve(node, budget(10)).unwrap();
    r.close(key(1), root).unwrap();
    assert!(r.is_closing(root) && r.is_closing(node));
    assert_eq!(r.reserve(node, budget(1)).err(), Some(Error::Closed));
    assert_eq!(r.grow(&mut charge, budget(1)), Err(Error::Closed));
    assert!(r.next_closed(key(1)).is_none() && r.next_closed(key(2)).is_none());
    // Stopped execution/consumed child status does not free retained frames or I/O.
    r.release_part(
        &mut charge,
        Usage {
            processes: 10,
            threads: 10,
            ..Usage::ZERO
        },
    )
    .unwrap();
    assert!(r.next_closed(key(2)).is_none());
    assert_eq!(r.ack_closed(key(2), node), Err(Error::Closed));
    r.release(charge).unwrap();
    let pending = r.next_closed(key(2)).unwrap();
    assert_eq!(pending.id, node);
    assert_eq!(pending.state, State::Closed);
    assert_eq!(
        r.next_closed(key(2)),
        Some(pending),
        "peek is durable across failed copy"
    );
    assert_eq!(r.ack_closed(key(3), node), Err(Error::Denied));
    assert!(
        r.next_closed(key(1)).is_none(),
        "parent waits for descendant record cleanup"
    );
    r.ack_closed(key(2), node).unwrap();
    assert_eq!(r.next_closed(key(1)).unwrap().id, root);
    r.ack_closed(key(1), root).unwrap();
    assert_eq!(r.status(key(1), root), Err(Error::Missing));
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}
#[test]
fn manager_death_closes_descendants_and_adopts_durable_cleanup_to_init() {
    let mut r = kernel();
    let root = session(&mut r);
    let node = r.create_service(key(2), root, budget(20)).unwrap();
    r.appoint_leader(key(2), node, key(4)).unwrap();
    let charge = r.reserve(node, budget(10)).unwrap();
    let sibling = r.create_service(key(1), SYSTEM, budget(20)).unwrap();
    let keep = r.reserve(sibling, budget(1)).unwrap();
    r.owner_died(key(2)).unwrap();
    assert_eq!(
        r.status(key(1), root).unwrap().cause,
        Some(CloseCause::LeaderDeath)
    );
    let node_view = r.status(key(1), node).unwrap();
    assert_eq!(node_view.owner, key(1));
    assert!(r.is_closing(node));
    assert!(!r.is_closing(sibling));
    assert_eq!(
        r.create_service(key(2), root, budget(1)),
        Err(Error::Closed)
    );
    assert!(r.next_closed(key(1)).is_none());
    r.release(charge).unwrap();
    assert_eq!(r.next_closed(key(1)).unwrap().id, node);
    r.ack_closed(key(1), node).unwrap();
    r.ack_closed(key(1), root).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, budget(1));
    r.release(keep).unwrap();
}
#[test]
fn leader_exit_closes_only_its_service_and_stale_identities_have_no_effect() {
    let mut r = kernel();
    let root = session(&mut r);
    let node = r.create_service(key(2), root, budget(20)).unwrap();
    r.appoint_leader(key(2), node, key(4)).unwrap();
    assert_eq!(
        r.owner_died(ProcessKey {
            pid: 4,
            generation: 2
        }),
        Err(Error::Invalid)
    );
    assert!(!r.is_closing(node));
    r.owner_died(key(99)).unwrap();
    assert!(!r.is_closing(node));
    assert_eq!(r.owner_died(key(1)), Err(Error::InitDeath));
    assert!(!r.is_closing(SYSTEM));
    r.owner_died(key(4)).unwrap();
    assert!(r.is_closing(node));
    assert!(!r.is_closing(root));
    assert_eq!(
        r.next_closed(key(2)).unwrap().cause,
        Some(CloseCause::LeaderDeath)
    );
    r.ack_closed(key(2), node).unwrap();
    let new = r.create_service(key(2), root, budget(20)).unwrap();
    r.appoint_leader(key(2), new, key(5)).unwrap();
    assert_ne!(new, node);
    assert_eq!(r.close(key(2), node), Err(Error::Missing));
    assert!(!r.is_closing(new));
    r.owner_died(key(4)).unwrap();
    assert!(!r.is_closing(new));
}

#[test]
fn one_process_cannot_be_the_factory_for_two_independent_sessions() {
    let mut r = kernel();
    let a = session(&mut r);
    let b = r.create_session(key(1), 1, budget(80)).unwrap();
    assert_eq!(r.appoint_leader(key(1), b, key(2)), Err(Error::Invalid));
    assert_eq!(r.status(key(1), b).unwrap().leader, None);
    r.appoint_leader(key(1), b, key(3)).unwrap();
    assert_eq!(r.create_service(key(2), b, budget(1)), Err(Error::Denied));
    assert_eq!(r.create_service(key(3), a, budget(1)), Err(Error::Denied));
}

#[test]
fn closed_pending_records_survive_owner_death_and_cannot_alias_new_ids() {
    let mut r = kernel();
    let root = session(&mut r);
    let mut previous = root;
    for _ in 0..128 {
        let id = r.create_service(key(2), root, Usage::ZERO).unwrap();
        assert!(id > previous);
        previous = id;
        r.close(key(2), id).unwrap();
        assert_eq!(r.next_closed(key(2)).unwrap().id, id);
        r.ack_closed(key(2), id).unwrap();
        assert_eq!(r.ack_closed(key(2), id), Err(Error::Missing));
    }
    let id = r.create_service(key(2), root, Usage::ZERO).unwrap();
    r.close(key(2), id).unwrap();
    r.owner_died(key(2)).unwrap();
    assert!(r.next_closed(key(2)).is_none());
    assert_eq!(r.next_closed(key(1)).unwrap().id, id);
    r.ack_closed(key(1), id).unwrap();
    assert_eq!(r.next_closed(key(1)).unwrap().id, root);
    r.ack_closed(key(1), root).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}

#[test]
fn bootstrap_and_failed_creation_leave_the_previous_registry_intact() {
    let mut r = Registry::new();
    assert_eq!(
        r.bootstrap(key(1), budget(1), budget(2)),
        Err(Error::Invalid)
    );
    assert_eq!(
        r.bootstrap(
            ProcessKey {
                pid: 1,
                generation: 2
            },
            budget(1),
            Usage::ZERO
        ),
        Err(Error::Invalid)
    );
    r.bootstrap(key(1), budget(100), budget(10)).unwrap();
    assert_eq!(
        r.bootstrap(key(2), budget(200), budget(1)),
        Err(Error::Invalid)
    );
    assert_eq!(r.status(key(1), SYSTEM).unwrap().owner, key(1));
    assert_eq!(r.create_session(key(1), 1, budget(91)), Err(Error::Limit));
    let id = r.create_session(key(1), 1, budget(90)).unwrap();
    assert_eq!(id, 2);
    assert_eq!(
        r.create_service(key(2), id, Usage::ZERO),
        Err(Error::Denied),
        "no factory until the leader is appointed"
    );
    for leader in [
        key(1),
        ProcessKey {
            pid: u32::MAX,
            generation: 1,
        },
        ProcessKey {
            pid: 2,
            generation: 2,
        },
    ] {
        assert_eq!(r.appoint_leader(key(1), id, leader), Err(Error::Invalid));
        assert_eq!(r.status(key(1), id).unwrap().leader, None);
    }
    assert_eq!(r.close(key(1), SYSTEM), Err(Error::Denied));
    r.appoint_leader(key(1), id, key(2)).unwrap();
    r.close(key(1), id).unwrap();
    r.close(key(1), id).unwrap();
    assert_eq!(
        r.create_service(key(2), id, Usage::ZERO),
        Err(Error::Closed)
    );
}

#[test]
fn allocator_room_accounts_for_ancestor_pressure_and_recovery_consumption() {
    let mut r = kernel();
    assert_eq!(r.available(SYSTEM), Ok(budget(100)));
    let root = session(&mut r);
    let a = r.create_service(key(2), root, budget(60)).unwrap();
    let b = r.create_service(key(2), root, budget(60)).unwrap();
    let first = r.reserve(a, budget(40)).unwrap();
    let second = r.reserve(b, budget(40)).unwrap();
    assert_eq!(
        r.available(a),
        Ok(Usage::ZERO),
        "parent pressure bounds a leaf allocator"
    );
    assert_eq!(r.available(SYSTEM), Ok(budget(20)));
    r.release(second).unwrap();
    assert_eq!(r.available(a), Ok(budget(20)));
    let trusted = r.reserve(SYSTEM, budget(60)).unwrap();
    assert_eq!(
        r.available(a),
        Ok(Usage::ZERO),
        "recovery allocations temporarily consume user headroom"
    );
    r.release(trusted).unwrap();
    r.release(first).unwrap();
    assert_eq!(r.available(a), Ok(budget(60)));
    r.close(key(2), a).unwrap();
    assert_eq!(r.available(a), Err(Error::Closed));
    r.ack_closed(key(2), a).unwrap();
    assert_eq!(r.available(a), Err(Error::Missing));
}

#[test]
fn recovery_use_in_one_resource_does_not_block_unrelated_allocations() {
    let mut r = kernel();
    let root = session(&mut r);
    let scope = r.create_service(key(2), root, budget(60)).unwrap();
    let trusted = r
        .reserve(
            SYSTEM,
            Usage {
                threads: 100,
                ..Usage::ZERO
            },
        )
        .unwrap();
    let room = r.available(scope).unwrap();
    assert_eq!(room.pages, 60);
    assert_eq!(room.threads, 0);
    let mut pages = r
        .reserve(
            scope,
            Usage {
                pages: 60,
                ..Usage::ZERO
            },
        )
        .expect("thread recovery pressure must not deny a page-only reservation");
    assert_eq!(
        r.grow(
            &mut pages,
            Usage {
                threads: 1,
                ..Usage::ZERO
            }
        ),
        Err(Error::Limit)
    );
    assert_eq!(
        r.status(key(1), SYSTEM).unwrap().used,
        Usage {
            pages: 60,
            threads: 100,
            ..Usage::ZERO
        }
    );
    r.release(pages).unwrap();
    r.release(trusted).unwrap();
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}

#[test]
fn session_record_pressure_is_bounded_until_cleanup_acknowledgement() {
    let mut r = kernel();
    let root = session(&mut r);
    let other = r.create_session(key(1), 1, budget(80)).unwrap();
    r.appoint_leader(key(1), other, key(3)).unwrap();
    let mut leaves = [0; MAX_SESSION_SERVICES];
    for id in &mut leaves {
        *id = r.create_service(key(2), root, budget(1)).unwrap();
    }
    for _ in 0..3 {
        assert_eq!(
            r.create_service(key(2), root, Usage::ZERO),
            Err(Error::Limit)
        );
    }
    // Empty and closed-but-unacknowledged records still own their slots.
    r.close(key(2), leaves[0]).unwrap();
    assert_eq!(r.status(key(2), leaves[0]).unwrap().state, State::Closed);
    assert_eq!(
        r.create_service(key(2), root, Usage::ZERO),
        Err(Error::Limit)
    );
    let survivor = r.create_service(key(3), other, Usage::ZERO).unwrap();
    assert_eq!(
        survivor,
        leaves[MAX_SESSION_SERVICES - 1] + 1,
        "failed admission must not consume identities"
    );
    let charge = r.reserve(leaves[1], budget(1)).unwrap();
    r.close(key(2), leaves[1]).unwrap();
    assert_eq!(r.status(key(2), leaves[1]).unwrap().state, State::Closing);
    assert_eq!(
        r.create_service(key(2), root, Usage::ZERO),
        Err(Error::Limit)
    );
    r.ack_closed(key(2), leaves[0]).unwrap();
    let replacement = r.create_service(key(2), root, Usage::ZERO).unwrap();
    assert!(replacement > survivor);
    assert_eq!(
        r.create_service(key(2), root, Usage::ZERO),
        Err(Error::Limit)
    );
    r.release(charge).unwrap();
    assert_eq!(
        r.create_service(key(2), root, Usage::ZERO),
        Err(Error::Limit)
    );
    r.ack_closed(key(2), leaves[1]).unwrap();
    assert!(r.create_service(key(2), root, Usage::ZERO).is_ok());
    assert_eq!(
        r.create_service(key(2), root, Usage::ZERO),
        Err(Error::Limit)
    );
    assert_eq!(r.status(key(3), survivor).unwrap().state, State::Open);
    assert_eq!(r.status(key(1), SYSTEM).unwrap().used, Usage::ZERO);
}
