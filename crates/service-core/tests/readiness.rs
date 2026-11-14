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
        Some(Action::NotifyStop {
            run: m.run_status(a.unit).unwrap().run,
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
        Some(Action::NotifyStop {
            run: m.run_status(again.unit).unwrap().run,
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
        Some(Action::NotifyStop {
            run: m.run_status(again.unit).unwrap().run,
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

#[test]
fn explicit_restart_keeps_one_job_until_new_ready_and_ignores_old_completion() {
    let mut m = manager("never", true);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 1)
        .unwrap();
    let b = launch(&mut m, 1, 11);
    next(&mut m, 1);
    let job = m.restart("a").unwrap();
    assert_eq!(
        next(&mut m, 2),
        Some(Action::NotifyStop {
            run: m.run_status(a.unit).unwrap().run,
            unit: a.unit,
            process: key(10)
        })
    );
    assert_eq!(next(&mut m, 2), None);
    assert_eq!(m.outcome(), Outcome::Running);
    m.completed(key(10), Completion::Killed, 3);
    let replacement = launch(&mut m, 3, 12);
    assert_eq!(replacement.unit, a.unit);
    assert_ne!(replacement.run, a.run);
    assert_eq!(m.job_id(), Some(job));
    assert_eq!(m.outcome(), Outcome::Running);
    assert_eq!(m.state(b.unit), Some(State::Running(key(11))));
    assert_eq!(m.completed(key(10), Completion::Killed, 4), None);
    m.notify(a.unit, replacement.run, key(12), Notification::Ready, 4)
        .unwrap();
    next(&mut m, 4);
    assert_eq!(m.outcome(), Outcome::Succeeded);
}
#[test]
fn explicit_restart_does_not_bypass_rolling_start_limit() {
    let mut m = manager("never", false);
    m.start("a").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 0)
        .unwrap();
    next(&mut m, 0);
    for (now, pid) in [(1, 11), (2, 12)] {
        let old = key(pid - 1);
        m.restart("a").unwrap();
        assert_eq!(
            next(&mut m, now),
            Some(Action::NotifyStop {
                run: m.run_status(a.unit).unwrap().run,
                unit: a.unit,
                process: old
            })
        );
        m.completed(old, Completion::Killed, now);
        let new = launch(&mut m, now, pid);
        m.notify(a.unit, new.run, key(pid), Notification::Ready, now)
            .unwrap();
        next(&mut m, now);
    }
    m.restart("a").unwrap();
    assert!(matches!(next(&mut m, 3), Some(Action::NotifyStop { .. })));
    m.completed(key(12), Completion::Killed, 3);
    assert_eq!(next(&mut m, 3), None);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert_eq!(m.state(a.unit), Some(State::Failed));
    m.stop("a").unwrap();
    next(&mut m, 4);
    assert_eq!(m.state(a.unit), Some(State::Inactive));
}

#[test]
fn restart_during_automatic_launch_waits_for_late_publication_and_termination() {
    let mut m = manager("on-failure", false);
    m.start("a").unwrap();
    let first = launch(&mut m, 0, 10);
    m.notify(first.unit, first.run, key(10), Notification::Ready, 0)
        .unwrap();
    next(&mut m, 0);
    m.completed(key(10), Completion::Fault(6), 1);
    let Some(Action::Start(late)) = next(&mut m, 3) else {
        panic!("automatic retry")
    };
    let job = m.restart("a").unwrap();
    assert_eq!(next(&mut m, 3), None);
    assert_eq!(m.job_id(), Some(job));
    assert_eq!(m.outcome(), Outcome::Running);
    m.spawned(late, key(12), 3).unwrap();
    assert_eq!(
        next(&mut m, 3),
        Some(Action::NotifyStop {
            run: m.run_status(first.unit).unwrap().run,
            unit: first.unit,
            process: key(12)
        })
    );
    assert_eq!(next(&mut m, 3), None);
    m.completed(key(12), Completion::Killed, 4);
    let new = launch(&mut m, 4, 13);
    assert_ne!(new.run, late.run);
    m.notify(new.unit, new.run, key(13), Notification::Ready, 5)
        .unwrap();
    next(&mut m, 5);
    assert_eq!(m.job_id(), Some(job));
    assert_eq!(m.outcome(), Outcome::Succeeded);
}

#[test]
fn asynchronous_backend_can_cancel_the_exact_unpublished_start_after_stop_or_timeout() {
    let mut m = manager("on-failure", false);
    m.start("a").unwrap();
    let first = launch(&mut m, 0, 10);
    m.notify(first.unit, first.run, key(10), Notification::Ready, 0)
        .unwrap();
    next(&mut m, 0);
    m.completed(key(10), Completion::Fault(6), 1);
    let Some(Action::Start(start)) = next(&mut m, 3) else {
        panic!("automatic launch")
    };
    assert!(!m.launch_cancelled(start));
    m.stop("a").unwrap();
    next(&mut m, 3);
    assert!(m.launch_cancelled(start));
    assert!(!m.launch_cancelled(Start {
        run: start.run + 1,
        ..start
    }));
    m.spawn_failed(start, 3).unwrap();
    next(&mut m, 3);
    assert!(!m.launch_cancelled(start));
    m.start("a").unwrap();
    let Some(Action::Start(start)) = next(&mut m, 4) else {
        panic!("next launch")
    };
    next(&mut m, 14);
    assert!(m.launch_cancelled(start));
}

#[test]
fn prerequisite_failure_cancels_an_unpublished_dependent_before_its_loader_finishes() {
    let mut m = manager("never", false);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 1)
        .unwrap();
    let Some(Action::Start(b)) = next(&mut m, 1) else {
        panic!("dependent reservation")
    };
    assert!(!m.launch_cancelled(b));
    m.completed(key(10), Completion::Fault(6), 2);
    next(&mut m, 2);
    assert!(
        m.launch_cancelled(b),
        "failed prerequisite must interrupt pending loading"
    );
    assert_eq!(
        m.outcome(),
        Outcome::Running,
        "cancellation still needs acknowledgement"
    );
    m.spawn_failed(b, 2).unwrap();
    next(&mut m, 2);
    assert_eq!(m.outcome(), Outcome::Failed);
    assert!(!m.launch_cancelled(b));
}

#[test]
fn stopping_a_graph_waits_for_dependents_before_an_unready_prerequisite_replacement() {
    let mut m = manager("on-failure", false);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 0)
        .unwrap();
    let b = launch(&mut m, 0, 11);
    next(&mut m, 0);
    m.completed(key(10), Completion::Fault(6), 1);
    let replacement = launch(&mut m, 3, 12);
    assert_eq!(m.state(a.unit), Some(State::Starting(key(12))));
    m.stop("b").unwrap();
    assert_eq!(
        next(&mut m, 4),
        Some(Action::NotifyStop {
            unit: b.unit,
            run: b.run,
            process: key(11)
        }),
        "unready resource ownership must preserve stop ordering"
    );
    assert_eq!(m.state(a.unit), Some(State::Starting(key(12))));
    assert_eq!(next(&mut m, 4), None);
    m.completed(key(11), Completion::Exit(0), 5);
    assert_eq!(
        next(&mut m, 5),
        Some(Action::NotifyStop {
            unit: replacement.unit,
            run: replacement.run,
            process: key(12)
        })
    );
    m.completed(key(12), Completion::Exit(0), 6);
    next(&mut m, 6);
    assert_eq!(m.outcome(), Outcome::Succeeded);
}

#[test]
fn stopping_a_graph_cannot_rearm_prerequisite_watchdogs_while_waiting_for_dependents() {
    let mut m = manager("on-failure", true);
    m.start("b").unwrap();
    let a = launch(&mut m, 0, 10);
    m.notify(a.unit, a.run, key(10), Notification::Ready, 0)
        .unwrap();
    launch(&mut m, 0, 11);
    next(&mut m, 0);
    m.stop("b").unwrap();
    next(&mut m, 1);
    assert_eq!(
        m.notify(a.unit, a.run, key(10), Notification::Heartbeat, 2),
        Err(NotifyError::Unexpected)
    );
    assert_eq!(
        m.next_deadline(),
        Some(91),
        "withdrawn health cannot shorten ordered stop grace"
    );
}

#[test]
fn scoped_restart_waits_for_cleanup_without_expired_deadline_spin() {
    let mut m = manager("on-failure", false);
    m.start("a").unwrap();
    let old = launch(&mut m, 0, 10);
    m.notify(old.unit, old.run, key(10), Notification::Ready, 0)
        .unwrap();
    assert_eq!(next(&mut m, 0), None);
    m.bind_scope(old.unit, old.run, key(10), 700).unwrap();
    assert_eq!(m.completed(key(10), Completion::Exit(1), 1), Some(old.unit));
    assert_eq!(
        m.next_deadline(),
        None,
        "draining scopes must not leave an expired restart deadline"
    );
    assert_eq!(next(&mut m, 50), None, "no restart before cleanup");
    assert!(m.scope_drained(old.unit, 701).is_err());
    assert_eq!(next(&mut m, 50), None);
    m.scope_drained(old.unit, 700).unwrap();
    assert!(m.scope_drained(old.unit, 700).is_err());
    assert_eq!(m.next_deadline(), Some(3));
    let Some(Action::Start(new)) = next(&mut m, 50) else {
        panic!("retry after scope closure")
    };
    assert_eq!(
        new.run,
        old.run + 1,
        "waiting must not consume launch attempts"
    );
}

#[test]
fn draining_scope_does_not_block_unrelated_service_and_rejects_stale_binding() {
    let a = b"Version=1\nKind=service\nName=a\nExec=/BIN/A.SVC\nRestart=on-failure\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\nRestartBurst=3\nRestartWindowTicks=100\nHealthyResetTicks=50\n";
    let b = b"Version=1\nKind=service\nName=b\nExec=/BIN/B.SVC\n";
    let mut m = Manager::new(Catalog::load(&[("A.UNIT", a), ("B.UNIT", b)]).unwrap()).unwrap();
    m.start("a").unwrap();
    let old = launch(&mut m, 0, 10);
    next(&mut m, 0);
    assert!(m.bind_scope(old.unit, old.run + 1, key(10), 800).is_err());
    assert!(m.bind_scope(old.unit, old.run, key(11), 800).is_err());
    m.bind_scope(old.unit, old.run, key(10), 800).unwrap();
    assert_eq!(m.completed(key(10), Completion::Exit(1), 1), Some(old.unit));
    m.start("b").unwrap();
    let Action::Start(other) = next(&mut m, 4).unwrap() else {
        panic!("unrelated service")
    };
    assert_ne!(other.unit, old.unit);
    m.spawned(other, key(20), 4).unwrap();
    assert_eq!(next(&mut m, 4), None);
    assert!(m.scope_drained(other.unit, 800).is_err());
    m.scope_drained(old.unit, 800).unwrap();
    assert!(matches!(next(&mut m, 4), Some(Action::Start(start)) if start.unit == old.unit));
}

#[test]
fn scoped_stop_job_waits_for_process_and_scope_in_either_order() {
    for scope_first in [false, true] {
        let mut m = manager("never", false);
        m.start("a").unwrap();
        let run = launch(&mut m, 0, 10);
        m.notify(run.unit, run.run, key(10), Notification::Ready, 0)
            .unwrap();
        assert_eq!(next(&mut m, 0), None);
        m.bind_scope(run.unit, run.run, key(10), 900).unwrap();
        m.stop("a").unwrap();
        assert!(matches!(next(&mut m, 1), Some(Action::NotifyStop { .. })));
        if scope_first {
            m.scope_drained(run.unit, 900).unwrap();
        }
        assert_eq!(m.outcome(), Outcome::Running);
        assert_eq!(m.completed(key(10), Completion::Exit(0), 2), Some(run.unit));
        if !scope_first {
            assert_eq!(next(&mut m, 2), None);
            assert_eq!(m.outcome(), Outcome::Running);
            m.scope_drained(run.unit, 900).unwrap();
        }
        assert_eq!(next(&mut m, 2), None);
        assert_eq!(m.outcome(), Outcome::Succeeded);
    }
}

#[test]
fn failed_scoped_launch_waits_for_scope_cleanup_in_either_event_order() {
    for scope_first in [false, true] {
        let mut m = manager("never", false);
        m.start("a").unwrap();
        let Some(Action::Start(start)) = next(&mut m, 0) else {
            panic!("launch")
        };
        assert!(m
            .bind_launch_scope(
                Start {
                    unit: start.unit,
                    run: start.run + 1
                },
                1000
            )
            .is_err());
        m.bind_launch_scope(start, 1000).unwrap();
        assert!(m.bind_launch_scope(start, 1001).is_err());
        if scope_first {
            m.scope_drained(start.unit, 1000).unwrap();
        }
        m.spawn_failed(start, 1).unwrap();
        if !scope_first {
            assert_eq!(next(&mut m, 1), None);
            assert_eq!(m.outcome(), Outcome::Running);
            assert!(m.scope_drained(start.unit, 999).is_err());
            m.scope_drained(start.unit, 1000).unwrap();
        }
        assert_eq!(next(&mut m, 1), None);
        assert_eq!(m.outcome(), Outcome::Failed);
        assert!(m.bind_launch_scope(start, 1002).is_err());
    }
}

#[test]
fn cancelled_scoped_launch_does_not_finish_shutdown_before_cleanup() {
    let mut m = manager("never", false);
    m.start("a").unwrap();
    let Some(Action::Start(start)) = next(&mut m, 0) else {
        panic!("launch")
    };
    m.bind_launch_scope(start, 1100).unwrap();
    m.shutdown(20).unwrap();
    assert_eq!(next(&mut m, 1), None);
    assert!(m.launch_cancelled(start));
    m.spawn_failed(start, 2).unwrap();
    assert_eq!(next(&mut m, 2), None);
    assert!(!m.shutdown_ready());
    assert_eq!(m.outcome(), Outcome::Running);
    m.scope_drained(start.unit, 1100).unwrap();
    assert_eq!(next(&mut m, 2), None);
    assert!(m.shutdown_ready());
}

#[test]
fn scope_bound_before_publication_survives_spawn_and_process_exit() {
    let mut m = manager("on-failure", false);
    m.start("a").unwrap();
    let Some(Action::Start(start)) = next(&mut m, 0) else {
        panic!("launch")
    };
    m.bind_launch_scope(start, 1200).unwrap();
    m.spawned(start, key(10), 0).unwrap();
    assert!(m.bind_launch_scope(start, 1201).is_err());
    m.notify(start.unit, start.run, key(10), Notification::Ready, 0)
        .unwrap();
    assert_eq!(next(&mut m, 0), None);
    assert_eq!(
        m.completed(key(10), Completion::Exit(1), 1),
        Some(start.unit)
    );
    assert_eq!(m.next_deadline(), None);
    assert_eq!(next(&mut m, 10), None);
    m.scope_drained(start.unit, 1200).unwrap();
    assert!(matches!(next(&mut m, 10), Some(Action::Start(next)) if next.unit == start.unit));
}

#[test]
fn late_scoped_child_published_during_shutdown_is_stopped_and_drained() {
    let mut m = manager("never", false);
    m.start("a").unwrap();
    let Some(Action::Start(start)) = next(&mut m, 0) else {
        panic!("launch")
    };
    m.bind_launch_scope(start, 1300).unwrap();
    m.shutdown(20).unwrap();
    assert_eq!(next(&mut m, 1), None);
    assert!(m.launch_cancelled(start));
    m.spawned(start, key(77), 2).unwrap();
    assert!(
        matches!(next(&mut m, 2), Some(Action::NotifyStop { process, .. }) if process == key(77))
    );
    assert_eq!(
        m.completed(key(77), Completion::Exit(0), 3),
        Some(start.unit)
    );
    assert_eq!(next(&mut m, 3), None);
    assert!(!m.shutdown_ready());
    m.scope_drained(start.unit, 1300).unwrap();
    assert_eq!(next(&mut m, 3), None);
    assert!(m.shutdown_ready());
}
