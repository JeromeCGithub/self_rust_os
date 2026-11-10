use rustos_service_core::{
    graph::Catalog, manager::*, transaction::Outcome, Completion, ProcessKey, State,
};
fn manager(edges: &str) -> Manager {
    manager_with_restart(edges, "always")
}
fn manager_with_restart(edges: &str, restart: &str) -> Manager {
    let a=b"Version=1\nKind=service\nName=a\nExec=/BIN/GLOW.SVC\nRestart=always\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\nRestartBurst=3\nRestartWindowTicks=100\nHealthyResetTicks=50\n";
    let b = format!("Version=1\nKind=service\nName=b\nExec=/BIN/FLICKER.SVC\n{edges}");
    let a = core::str::from_utf8(a)
        .unwrap()
        .replace("Restart=always", &format!("Restart={restart}"));
    Manager::new(Catalog::load(&[("A.UNIT", a.as_bytes()), ("B.UNIT", b.as_bytes())]).unwrap())
        .unwrap()
}
fn next(m: &mut Manager, now: u64) -> Option<Action> {
    m.next_action(now).unwrap()
}
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn launch(m: &mut Manager, now: u64, pid: u32) -> usize {
    match next(m, now).unwrap() {
        Action::Start(start) => {
            m.spawned(start, key(pid), now).unwrap();
            start.unit
        }
        other => panic!("expected launch, got {other:?}"),
    }
}
#[test]
fn dependency_activation_and_event_driven_restart_share_one_host_tested_adapter() {
    let mut m = manager("Requires=a\n");
    assert_eq!(next(&mut m, 0), None);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    let b = launch(&mut m, 0, 11);
    assert_ne!(a, b);
    assert_eq!(next(&mut m, 0), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert_eq!(m.completed(key(10), Completion::Fault(6), 1), Some(a));
    assert_eq!(m.next_deadline(), Some(3));
    assert_eq!(next(&mut m, 2), None);
    assert_eq!(launch(&mut m, 3, 12), a);
    assert_eq!(m.completed(key(10), Completion::Killed, 4), None);
    assert_eq!(m.state(b), Some(State::Running(key(11))));
    assert_eq!(next(&mut m, 4), None);
}
#[test]
fn spawn_failure_rolls_back_and_suppresses_retries_for_unowned_branches() {
    let mut m = manager("Requires=a\n");
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    let Action::Start(start) = next(&mut m, 0).unwrap() else {
        panic!("missing b")
    };
    m.spawn_failed(start, 0).unwrap();
    assert_eq!(
        next(&mut m, 0),
        Some(Action::NotifyStop {
            run: m.run_status(a).unwrap().run,
            unit: a,
            process: key(10)
        })
    );
    assert_eq!(next(&mut m, 0), None, "termination cannot be duplicated");
    m.completed(key(10), Completion::Killed, 1);
    assert_eq!(next(&mut m, 1), None);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert_eq!(m.next_deadline(), None);
    assert_eq!(next(&mut m, 999), None);
}
#[test]
fn observed_loss_before_dependent_start_prevents_activation_and_restart() {
    let mut m = manager_with_restart("Requires=a\n", "never");
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    m.completed(key(10), Completion::Exit(1), 1);
    assert_eq!(next(&mut m, 1), None);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert_eq!(m.state(a), Some(State::Inactive));
    assert_eq!(next(&mut m, 999), None);
}
#[test]
fn explicit_stop_cancels_backoff_and_late_results_cannot_change_a_new_run() {
    let mut m = manager("");
    m.start("a").unwrap();
    let a = launch(&mut m, 0, 10);
    next(&mut m, 0);
    m.completed(key(10), Completion::Killed, 1);
    m.stop("a").unwrap();
    assert_eq!(next(&mut m, 1), None);
    assert_eq!(m.next_deadline(), None);
    m.start("a").unwrap();
    let Action::Start(start) = next(&mut m, 2).unwrap() else {
        panic!("missing start")
    };
    m.spawned(start, key(11), 2).unwrap();
    next(&mut m, 2);
    assert!(m.spawned(start, key(12), 3).is_err());
    assert_eq!(m.completed(key(10), Completion::Killed, 3), None);
    assert_eq!(m.state(a), Some(State::Running(key(11))));
}

#[test]
fn explicit_activation_at_exhausted_rate_limit_fails_instead_of_stalling() {
    let mut m = manager("");
    for pid in 10..13 {
        m.start("a").unwrap();
        launch(&mut m, 0, pid);
        next(&mut m, 0);
        m.stop("a").unwrap();
        assert!(matches!(next(&mut m, 0), Some(Action::NotifyStop { .. })));
        m.completed(key(pid), Completion::Killed, 0);
        next(&mut m, 0);
    }
    m.start("a").unwrap();
    assert_eq!(next(&mut m, 0), None);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert_eq!(m.next_deadline(), None);
}

#[test]
fn retryable_startup_loss_holds_dependents_until_replacement_or_terminal_limit() {
    let mut m = manager("Requires=a\n");
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    m.completed(key(10), Completion::Fault(6), 1);
    assert_eq!(next(&mut m, 1), None);
    assert_eq!(
        m.outcome(),
        Outcome::Running,
        "retryable prerequisite failure is not yet terminal"
    );
    assert_eq!(m.next_deadline(), Some(3));
    assert_eq!(launch(&mut m, 3, 11), a);
    let b = launch(&mut m, 3, 12);
    assert_ne!(a, b);
    assert_eq!(next(&mut m, 3), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);

    let mut m = manager("Requires=a\n");
    m.start("b").unwrap();
    for (now, pid, due) in [(0, 10, 2), (2, 11, 6), (6, 12, 14)] {
        launch(&mut m, now, pid);
        m.completed(key(pid), Completion::Fault(6), now);
        assert_eq!(next(&mut m, now), None);
        assert_eq!(m.next_deadline(), Some(due));
    }
    assert_eq!(next(&mut m, 14), None);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert_eq!(m.next_deadline(), None);
}

#[test]
fn a_previously_settled_required_target_waits_for_startup_replacement() {
    let a=b"Version=1\nKind=service\nName=a\nExec=/BIN/GLOW.SVC\nRestart=always\nRestartDelayTicks=2\nRestartMaxDelayTicks=2\n";
    let b = b"Version=1\nKind=service\nName=b\nExec=/BIN/GLOW.SVC\nRequires=group\nAfter=c\n";
    let c = b"Version=1\nKind=service\nName=c\nExec=/BIN/GLOW.SVC\n";
    let group = b"Version=1\nKind=target\nName=group\nRequires=a\n";
    let boot = b"Version=1\nKind=target\nName=boot\nRequires=b\nWants=c\n";
    let catalog = Catalog::load(&[
        ("A.UNIT", a),
        ("B.UNIT", b),
        ("C.UNIT", c),
        ("GROUP.UNIT", group),
        ("BOOT.UNIT", boot),
    ])
    .unwrap();
    let mut m = Manager::new(catalog).unwrap();
    m.start("boot").unwrap();
    let a = launch(&mut m, 0, 10);
    let Action::Start(c) = next(&mut m, 0).unwrap() else {
        panic!("expected c")
    };
    m.completed(key(10), Completion::Fault(6), 1);
    m.spawned(c, key(11), 1).unwrap();
    assert_eq!(
        next(&mut m, 1),
        None,
        "b cannot use a stale target activation"
    );
    assert_eq!(m.outcome(), Outcome::Running);
    assert_eq!(launch(&mut m, 3, 12), a);
    assert_eq!(launch(&mut m, 3, 13), m.catalog().lookup("b").unwrap());
    assert_eq!(next(&mut m, 3), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
}
