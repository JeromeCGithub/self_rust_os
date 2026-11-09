use rustos_service_core::{
    config::Readiness, graph::Catalog, manager::*, transaction::Outcome, Completion, ProcessKey,
    Start, State,
};
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn manager(restart: &str, watchdog: bool) -> Manager {
    let a=format!("Version=1\nKind=service\nName=a\nExec=/BIN/A.SVC\nReady=notify\nStartTimeoutTicks=10\nRestart={restart}\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\nRestartBurst=3\nRestartWindowTicks=100\nHealthyResetTicks=50\n{}",if watchdog{"WatchdogTicks=8\n"}else{""});
    let b = b"Version=1\nKind=service\nName=b\nExec=/BIN/B.SVC\nRequires=a\n";
    let catalog = Catalog::load(&[("A.UNIT", a.as_bytes()), ("B.UNIT", b)]).unwrap();
    assert_eq!(
        catalog
            .unit(catalog.lookup("a").unwrap())
            .unwrap()
            .readiness,
        Readiness::Notify
    );
    Manager::new(catalog).unwrap()
}
fn next(m: &mut Manager, now: u64) -> Option<Action> {
    m.next_action(now).unwrap()
}
fn launch(m: &mut Manager, now: u64, pid: u32) -> Start {
    let Action::Start(start) = next(m, now).expect("launch") else {
        panic!("expected start")
    };
    m.spawned(start, key(pid), now).unwrap();
    start
}
#[test]
fn dependency_waits_for_authenticated_ready_and_rejects_early_duplicate_or_stale_notifications() {
    let mut m = manager("on-failure", true);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    assert_eq!(m.state(a.unit), Some(State::Starting(key(10))));
    assert_eq!(m.next_deadline(), Some(10));
    assert_eq!(next(&mut m, 0), None);
    assert_eq!(
        m.notify(a.unit, a.run, key(11), Notification::Ready, 1),
        Err(NotifyError::Stale)
    );
    assert_eq!(
        m.notify(a.unit, a.run + 1, key(10), Notification::Ready, 1),
        Err(NotifyError::Stale)
    );
    assert_eq!(
        m.notify(a.unit, a.run, key(10), Notification::Heartbeat, 1),
        Err(NotifyError::Unexpected)
    );
    m.notify(a.unit, a.run, key(10), Notification::Ready, 5)
        .unwrap();
    assert!(m.run_status(a.unit).unwrap().ready);
    assert_eq!(m.next_deadline(), Some(13));
    let b = launch(&mut m, 5, 11);
    assert_ne!(a.unit, b.unit);
    assert_eq!(next(&mut m, 5), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert_eq!(
        m.notify(a.unit, a.run, key(10), Notification::Ready, 6),
        Err(NotifyError::Unexpected)
    );
    assert_eq!(
        m.next_deadline(),
        Some(13),
        "duplicate Ready cannot refresh a watchdog"
    );
    m.notify(a.unit, a.run, key(10), Notification::Heartbeat, 12)
        .unwrap();
    assert_eq!(m.next_deadline(), Some(20));
}
#[test]
fn exact_start_deadline_rejects_late_ready_and_waits_for_real_exit_before_failing_job() {
    let mut m = manager("never", false);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    assert_eq!(
        m.notify(a.unit, a.run, key(10), Notification::Ready, 10),
        Err(NotifyError::Late)
    );
    assert_eq!(
        next(&mut m, 10),
        Some(Action::Terminate {
            unit: a.unit,
            process: key(10)
        })
    );
    assert_eq!(
        m.run_status(a.unit).unwrap().failure,
        Some(Failure::StartTimeout)
    );
    assert_eq!(m.outcome(), Outcome::Running);
    assert_eq!(next(&mut m, 10), None);
    assert_eq!(m.completed(key(11), Completion::Killed, 10), None);
    m.completed(key(10), Completion::Killed, 11);
    assert_eq!(next(&mut m, 11), None);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert_eq!(m.next_deadline(), None);
}
#[test]
fn zero_exit_before_ready_keeps_activation_pending_through_restart_and_rejects_old_generation() {
    let mut m = manager("on-failure", false);
    m.start("b").unwrap();
    let old = launch(&mut m, 0, 10);
    m.completed(key(10), Completion::Exit(0), 1);
    assert_eq!(next(&mut m, 1), None);
    assert_eq!(m.outcome(), Outcome::Running);
    assert_eq!(m.next_deadline(), Some(3));
    let Action::Start(new) = next(&mut m, 3).unwrap() else {
        panic!("replacement")
    };
    let newer = ProcessKey {
        pid: 10,
        generation: 2,
    };
    m.spawned(new, newer, 3).unwrap();
    assert_ne!(old.run, new.run);
    assert_eq!(
        m.notify(new.unit, new.run, key(10), Notification::Ready, 4),
        Err(NotifyError::Stale)
    );
    assert_eq!(
        m.notify(new.unit, old.run, newer, Notification::Ready, 4),
        Err(NotifyError::Stale)
    );
    m.notify(new.unit, new.run, newer, Notification::Ready, 4)
        .unwrap();
    launch(&mut m, 4, 11);
    assert_eq!(next(&mut m, 4), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
}
#[test]
fn watchdog_stops_and_restarts_only_its_run_and_late_heartbeat_cannot_rescue_it() {
    let mut m = manager("on-failure", true);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 0)
        .unwrap();
    let b = launch(&mut m, 0, 11);
    next(&mut m, 0);
    assert_eq!(next(&mut m, 7), None);
    assert_eq!(
        m.notify(a.unit, a.run, key(10), Notification::Heartbeat, 8),
        Err(NotifyError::Late)
    );
    assert_eq!(
        next(&mut m, 8),
        Some(Action::Terminate {
            unit: a.unit,
            process: key(10)
        })
    );
    assert_eq!(
        m.run_status(a.unit).unwrap().failure,
        Some(Failure::Watchdog)
    );
    assert!(m
        .notify(a.unit, a.run, key(10), Notification::Heartbeat, 8)
        .is_err());
    m.completed(key(10), Completion::Killed, 9);
    assert_eq!(next(&mut m, 9), None);
    assert_eq!(
        m.state(b.unit),
        Some(State::Running(key(11))),
        "Requires is not a post-activation liveness binding"
    );
    let again = launch(&mut m, 11, 12);
    assert_eq!(again.unit, a.unit);
    assert_eq!(m.state(a.unit), Some(State::Starting(key(12))));
    m.notify(a.unit, again.run, key(12), Notification::Ready, 12)
        .unwrap();
    assert_eq!(m.next_deadline(), Some(20));
    assert_eq!(next(&mut m, 12), None);
}
#[test]
fn timed_out_pending_launch_cleans_up_late_success_without_publishing_readiness() {
    let mut m = manager("never", false);
    m.start("b").unwrap();
    let Action::Start(start) = next(&mut m, 0).unwrap() else {
        panic!("launch")
    };
    assert_eq!(next(&mut m, 10), None, "no process exists to terminate yet");
    assert_eq!(
        m.run_status(start.unit).unwrap().failure,
        Some(Failure::StartTimeout)
    );
    m.spawned(start, key(10), 11).unwrap();
    assert_eq!(
        next(&mut m, 11),
        Some(Action::Terminate {
            unit: start.unit,
            process: key(10)
        })
    );
    assert!(m
        .notify(start.unit, start.run, key(10), Notification::Ready, 11)
        .is_err());
    m.completed(key(10), Completion::Killed, 12);
    next(&mut m, 12);
    assert_eq!(m.outcome(), Outcome::Failed);
}
#[test]
fn explicit_stop_disarms_watchdog_and_suppresses_automatic_restart() {
    let mut m = manager("always", true);
    m.start("a").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 1)
        .unwrap();
    next(&mut m, 1);
    m.stop("a").unwrap();
    assert_eq!(
        m.next_deadline(),
        None,
        "stop intent cancels deadlines immediately"
    );
    assert_eq!(
        next(&mut m, 2),
        Some(Action::Terminate {
            unit: a.unit,
            process: key(10)
        })
    );
    m.completed(key(10), Completion::Killed, 3);
    assert_eq!(next(&mut m, 3), None);
    assert_eq!(next(&mut m, 1000), None);
    assert_eq!(m.next_deadline(), None);
}
#[test]
fn start_deadline_overflow_fails_admission_without_a_process_effect_or_wrapped_timer() {
    let mut m = manager("never", false);
    m.start("a").unwrap();
    assert_eq!(next(&mut m, u64::MAX - 5), None);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert_eq!(m.next_deadline(), None);
    let a = m.catalog().lookup("a").unwrap();
    assert_eq!(m.run_status(a).unwrap().failure, Some(Failure::Clock));
}

#[test]
fn stopping_an_unready_replacement_waits_for_its_real_process_completion() {
    let mut m = manager("on-failure", false);
    m.start("a").unwrap();
    let first = launch(&mut m, 0, 10);
    m.notify(first.unit, first.run, key(10), Notification::Ready, 0)
        .unwrap();
    next(&mut m, 0);
    m.completed(key(10), Completion::Fault(6), 1);
    let again = launch(&mut m, 3, 11);
    m.stop("a").unwrap();
    assert_eq!(
        next(&mut m, 4),
        Some(Action::Terminate {
            unit: again.unit,
            process: key(11)
        })
    );
    assert_eq!(
        m.outcome(),
        Outcome::Running,
        "unready is not the same as no owned process"
    );
    m.completed(key(11), Completion::Killed, 5);
    assert_eq!(next(&mut m, 5), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
}
#[test]
fn stop_waits_for_an_inflight_replacement_and_terminates_its_late_publication() {
    let mut m = manager("on-failure", false);
    m.start("a").unwrap();
    let first = launch(&mut m, 0, 10);
    m.notify(first.unit, first.run, key(10), Notification::Ready, 0)
        .unwrap();
    next(&mut m, 0);
    m.completed(key(10), Completion::Fault(6), 1);
    let Action::Start(again) = next(&mut m, 3).unwrap() else {
        panic!("replacement")
    };
    m.stop("a").unwrap();
    assert_eq!(next(&mut m, 4), None);
    assert_eq!(
        m.outcome(),
        Outcome::Running,
        "a launch reservation can still publish a child"
    );
    m.spawned(again, key(11), 5).unwrap();
    assert_eq!(
        next(&mut m, 5),
        Some(Action::Terminate {
            unit: again.unit,
            process: key(11)
        })
    );
    m.completed(key(11), Completion::Killed, 6);
    assert_eq!(next(&mut m, 6), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
}

#[test]
fn stop_after_watchdog_kill_still_waits_for_the_outstanding_child() {
    let mut m = manager("on-failure", true);
    m.start("a").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 0)
        .unwrap();
    next(&mut m, 0);
    assert_eq!(
        next(&mut m, 8),
        Some(Action::Terminate {
            unit: a.unit,
            process: key(10)
        })
    );
    m.stop("a").unwrap();
    assert_eq!(next(&mut m, 9), None, "one kill request per run");
    assert_eq!(m.outcome(), Outcome::Running);
    m.completed(key(10), Completion::Killed, 10);
    assert_eq!(next(&mut m, 10), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert_eq!(m.next_deadline(), None);
}
#[test]
fn failed_inflight_launch_acknowledges_stop_and_rejects_a_duplicate_late_result() {
    let mut m = manager("on-failure", false);
    m.start("a").unwrap();
    let first = launch(&mut m, 0, 10);
    m.notify(first.unit, first.run, key(10), Notification::Ready, 0)
        .unwrap();
    next(&mut m, 0);
    m.completed(key(10), Completion::Fault(6), 1);
    let Action::Start(again) = next(&mut m, 3).unwrap() else {
        panic!("replacement")
    };
    m.stop("a").unwrap();
    assert_eq!(next(&mut m, 4), None);
    assert_eq!(m.outcome(), Outcome::Running);
    m.spawn_failed(again, 5).unwrap();
    assert_eq!(next(&mut m, 5), None);
    assert_eq!(m.outcome(), Outcome::Succeeded);
    assert!(m.spawned(again, key(11), 6).is_err());
    assert_eq!(m.next_deadline(), None);
}
