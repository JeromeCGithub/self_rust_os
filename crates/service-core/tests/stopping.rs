use rustos_service_core::{
    graph::Catalog,
    manager::{Action, Failure, Manager},
    transaction::Outcome,
    Completion, ProcessKey,
};
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn started() -> (Manager, usize, u64) {
    let mut m=Manager::new(Catalog::load(&[("A.UNIT",b"Version=1\nKind=service\nName=a\nExec=/BIN/A.SVC\nStopTimeoutTicks=5\nRestart=always\n")]).unwrap()).unwrap();
    m.start("a").unwrap();
    let Some(Action::Start(start)) = m.next_action(0).unwrap() else {
        panic!("launch")
    };
    m.spawned(start, key(10), 0).unwrap();
    m.next_action(0).unwrap();
    (m, start.unit, start.run)
}
#[test]
fn cooperative_stop_waits_for_actual_exit_and_suppresses_restart() {
    let (mut m, unit, run) = started();
    m.stop("a").unwrap();
    assert_eq!(
        m.next_action(1).unwrap(),
        Some(Action::NotifyStop {
            unit,
            process: key(10),
            run
        })
    );
    assert_eq!(m.next_deadline(), Some(6));
    assert_eq!(m.next_action(5).unwrap(), None);
    assert_eq!(m.outcome(), Outcome::Running);
    assert_eq!(m.completed(key(10), Completion::Exit(0), 5), Some(unit));
    assert_eq!(m.next_action(5).unwrap(), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert_eq!(m.next_deadline(), None);
    assert_eq!(m.next_action(999).unwrap(), None);
}
#[test]
fn ignored_stop_escalates_once_at_deadline_and_still_waits_for_completion() {
    let (mut m, unit, run) = started();
    m.stop("a").unwrap();
    assert_eq!(
        m.next_action(1).unwrap(),
        Some(Action::NotifyStop {
            unit,
            process: key(10),
            run
        })
    );
    assert_eq!(m.next_action(5).unwrap(), None);
    assert_eq!(
        m.next_action(6).unwrap(),
        Some(Action::Terminate {
            unit,
            process: key(10)
        })
    );
    assert_eq!(
        m.run_status(unit).unwrap().failure,
        Some(Failure::StopTimeout)
    );
    assert_eq!(
        m.next_deadline(),
        None,
        "expired deadline must not spin after escalation"
    );
    assert_eq!(m.next_action(7).unwrap(), None);
    assert_eq!(m.outcome(), Outcome::Running);
    m.completed(key(10), Completion::Killed, 8);
    m.next_action(8).unwrap();
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert_eq!(m.next_action(999).unwrap(), None);
}
#[test]
fn failed_stop_delivery_escalates_only_the_exact_pending_run() {
    let (mut m, unit, run) = started();
    assert!(m.stop_delivery_failed(unit, run, key(10)).is_err());
    m.stop("a").unwrap();
    m.next_action(1).unwrap();
    for (u, r, k) in [
        (unit, run + 1, key(10)),
        (unit, run, key(11)),
        (64, run, key(10)),
    ] {
        assert!(m.stop_delivery_failed(u, r, k).is_err());
        assert_eq!(m.next_deadline(), Some(6));
    }
    m.stop_delivery_failed(unit, run, key(10)).unwrap();
    assert_eq!(
        m.next_action(2).unwrap(),
        Some(Action::Terminate {
            unit,
            process: key(10)
        })
    );
    assert_eq!(
        m.run_status(unit).unwrap().failure,
        Some(Failure::StopDelivery)
    );
    assert!(m.stop_delivery_failed(unit, run, key(10)).is_err());
}
#[test]
fn overflowing_stop_deadline_forces_cleanup_without_wrapping_or_waiting() {
    let (mut m, unit, _) = started();
    m.stop("a").unwrap();
    assert_eq!(
        m.next_action(u64::MAX - 2).unwrap(),
        Some(Action::Terminate {
            unit,
            process: key(10)
        })
    );
    assert_eq!(m.next_deadline(), None);
    assert_eq!(m.run_status(unit).unwrap().failure, Some(Failure::Clock));
}
