use rustos_service_core::{
    graph::Catalog,
    manager::{Action, Manager},
    transaction::{Error, Outcome},
    Completion, ProcessKey, Start, State,
};
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn manager() -> Manager {
    Manager::new(Catalog::load(&[
        ("A.UNIT",b"Version=1\nKind=service\nName=a\nExec=/BIN/A.SVC\nRestart=always\nRestartDelayTicks=2\nStopTimeoutTicks=50\n"),
        ("B.UNIT",b"Version=1\nKind=service\nName=b\nExec=/BIN/B.SVC\nRequires=a\nStopTimeoutTicks=50\n")]).unwrap()).unwrap()
}
fn reserve(m: &mut Manager, now: u64) -> Start {
    let Some(Action::Start(start)) = m.next_action(now).unwrap() else {
        panic!("reserved launch")
    };
    start
}
fn running() -> (Manager, Start, Start) {
    let mut m = manager();
    m.start("b").unwrap();
    let a = reserve(&mut m, 0);
    m.spawned(a, key(10), 0).unwrap();
    let b = reserve(&mut m, 0);
    m.spawned(b, key(11), 0).unwrap();
    m.next_action(0).unwrap();
    (m, a, b)
}
#[test]
fn shutdown_stops_in_reverse_order_waits_for_exit_and_never_accepts_new_work() {
    let (mut m, a, b) = running();
    let job = m.shutdown(20).unwrap();
    assert_eq!(
        m.shutdown(50),
        Ok(job),
        "coalescing must not replace global deadline"
    );
    for result in [m.start("a"), m.stop("a"), m.restart("a")] {
        assert_eq!(result, Err(Error::Busy));
    }
    assert_eq!(
        m.next_action(1).unwrap(),
        Some(Action::NotifyStop {
            unit: b.unit,
            run: b.run,
            process: key(11)
        })
    );
    assert_eq!(m.state(a.unit), Some(State::Running(key(10))));
    assert_eq!(m.next_deadline(), Some(21));
    assert!(!m.shutdown_ready());
    assert_eq!(m.next_action(2).unwrap(), None);
    m.completed(key(11), Completion::Exit(0), 2);
    assert_eq!(
        m.next_action(2).unwrap(),
        Some(Action::NotifyStop {
            unit: a.unit,
            run: a.run,
            process: key(10)
        })
    );
    m.completed(key(10), Completion::Exit(0), 3);
    assert_eq!(m.next_action(3).unwrap(), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert!(m.shutdown_ready());
    assert_eq!(m.next_deadline(), None);
    assert_eq!(m.next_action(999).unwrap(), None);
    assert_eq!(m.start("a"), Err(Error::Busy));
}
#[test]
fn global_deadline_forces_every_live_service_without_waiting_for_ordered_grace() {
    let (mut m, a, b) = running();
    m.shutdown(5).unwrap();
    m.next_action(1).unwrap();
    assert_eq!(m.next_action(5).unwrap(), None);
    let effects = [
        m.next_action(6).unwrap().unwrap(),
        m.next_action(6).unwrap().unwrap(),
    ];
    assert!(effects.contains(&Action::Terminate {
        unit: a.unit,
        process: key(10)
    }));
    assert!(effects.contains(&Action::Terminate {
        unit: b.unit,
        process: key(11)
    }));
    assert_eq!(m.next_action(7).unwrap(), None);
    assert_eq!(m.next_deadline(), None);
    assert!(!m.shutdown_ready());
    m.completed(key(10), Completion::Killed, 8);
    assert!(!m.shutdown_ready());
    m.completed(key(11), Completion::Killed, 8);
    m.next_action(8).unwrap();
    assert!(m.shutdown_ready());
}
#[test]
fn shutdown_cancels_unpublished_launch_and_accounts_for_a_raced_child() {
    let mut m = manager();
    m.start("b").unwrap();
    let a = reserve(&mut m, 0);
    m.shutdown(20).unwrap();
    assert!(m.launch_cancelled(a));
    assert_eq!(m.next_action(1).unwrap(), None);
    assert!(!m.shutdown_ready());
    m.spawned(a, key(10), 2).unwrap();
    assert_eq!(
        m.next_action(2).unwrap(),
        Some(Action::NotifyStop {
            unit: a.unit,
            run: a.run,
            process: key(10)
        })
    );
    m.completed(key(10), Completion::Exit(0), 3);
    assert_eq!(m.next_action(3).unwrap(), None);
    assert!(m.shutdown_ready());
    assert_eq!(m.state(1), Some(State::Inactive));
}
#[test]
fn shutdown_cancels_restart_backoff_and_is_valid_without_any_process() {
    let mut m = manager();
    m.start("a").unwrap();
    let a = reserve(&mut m, 0);
    m.spawned(a, key(10), 0).unwrap();
    m.next_action(0).unwrap();
    m.completed(key(10), Completion::Fault(6), 1);
    assert_eq!(m.state(a.unit), Some(State::Backoff(3)));
    m.shutdown(20).unwrap();
    assert_eq!(m.next_action(2).unwrap(), None);
    assert!(m.shutdown_ready());
    assert_eq!(m.next_action(1000).unwrap(), None);
}
#[test]
fn invalid_global_timeout_has_no_effect_and_overflow_forces_instead_of_wrapping() {
    let (mut m, _, _) = running();
    assert!(m.shutdown(0).is_err());
    assert!(m.shutdown(u64::MAX).is_err());
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert_eq!(m.next_action(1).unwrap(), None);
    m.shutdown(5).unwrap();
    assert!(matches!(
        m.next_action(u64::MAX - 2).unwrap(),
        Some(Action::Terminate { .. })
    ));
    assert!(matches!(
        m.next_action(u64::MAX - 2).unwrap(),
        Some(Action::Terminate { .. })
    ));
    assert_eq!(m.next_deadline(), None);
    assert!(!m.shutdown_ready());
}

#[test]
fn shutdown_stops_a_live_dependent_before_its_restarted_prerequisite() {
    let (mut m, a, b) = running();
    m.completed(key(10), Completion::Fault(6), 1);
    let replacement = reserve(&mut m, 3);
    assert_eq!(replacement.unit, a.unit);
    m.spawned(replacement, key(12), 3).unwrap();
    m.next_action(3).unwrap();
    m.shutdown(20).unwrap();
    assert_eq!(
        m.next_action(4).unwrap(),
        Some(Action::NotifyStop {
            unit: b.unit,
            process: key(11),
            run: b.run
        }),
        "shutdown must use dependency order, not reverse process-creation order"
    );
    m.completed(key(11), Completion::Exit(0), 5);
    assert_eq!(
        m.next_action(5).unwrap(),
        Some(Action::NotifyStop {
            unit: a.unit,
            process: key(12),
            run: replacement.run
        })
    );
}
#[test]
fn shutdown_preserves_ordering_through_processless_target_dependencies() {
    let a =
        b"Version=1\nKind=service\nName=a\nExec=/BIN/A.SVC\nRestart=always\nRestartDelayTicks=2\n";
    let b = b"Version=1\nKind=service\nName=b\nExec=/BIN/B.SVC\nRequires=middle\n";
    let group = b"Version=1\nKind=target\nName=middle\nRequires=a\n";
    let mut m = Manager::new(
        Catalog::load(&[("A.UNIT", a), ("B.UNIT", b), ("GROUP.UNIT", group)]).unwrap(),
    )
    .unwrap();
    m.start("b").unwrap();
    let a = reserve(&mut m, 0);
    m.spawned(a, key(10), 0).unwrap();
    let b = reserve(&mut m, 0);
    m.spawned(b, key(11), 0).unwrap();
    m.next_action(0).unwrap();
    m.completed(key(10), Completion::Fault(6), 1);
    let replacement = reserve(&mut m, 3);
    m.spawned(replacement, key(12), 3).unwrap();
    m.next_action(3).unwrap();
    m.shutdown(20).unwrap();
    assert_eq!(
        m.next_action(4).unwrap(),
        Some(Action::NotifyStop {
            unit: b.unit,
            process: key(11),
            run: b.run
        })
    );
}
#[test]
fn shutdown_reverses_after_when_separately_started_services_are_now_both_selected() {
    let mut m = Manager::new(
        Catalog::load(&[
            (
                "A.UNIT",
                b"Version=1\nKind=service\nName=a\nExec=/BIN/A.SVC\nAfter=b\n",
            ),
            (
                "B.UNIT",
                b"Version=1\nKind=service\nName=b\nExec=/BIN/B.SVC\n",
            ),
        ])
        .unwrap(),
    )
    .unwrap();
    m.start("a").unwrap();
    let a = reserve(&mut m, 0);
    m.spawned(a, key(10), 0).unwrap();
    m.next_action(0).unwrap();
    m.start("b").unwrap();
    let b = reserve(&mut m, 1);
    m.spawned(b, key(11), 1).unwrap();
    m.next_action(1).unwrap();
    m.shutdown(20).unwrap();
    assert_eq!(
        m.next_action(2).unwrap(),
        Some(Action::NotifyStop {
            unit: a.unit,
            process: key(10),
            run: a.run
        })
    );
}
