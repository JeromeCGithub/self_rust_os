use initator_host::{Event, Runtime};
use rustos_service_core::{graph::Catalog, transaction::Outcome};
use std::time::Duration;
fn runtime(mode: &str, extra: &str) -> Runtime {
    let service=format!("Version=1\nKind=service\nName=beacon\nExec={}\nArg={mode}\nReady=notify\nStartTimeoutTicks=100\n{extra}",env!("CARGO_BIN_EXE_service-fixture"));
    Runtime::new(Catalog::load(&[("A.UNIT", service.as_bytes())]).unwrap()).unwrap()
}
#[test]
fn real_child_ready_and_exit_are_delivered_without_polling_process_status() {
    let mut runtime = runtime("exit", "");
    runtime.manager.start("beacon").unwrap();
    runtime
        .drive_until(Duration::from_secs(5), |r| {
            r.events().any(|e| matches!(e, Event::Ready { .. }))
        })
        .unwrap();
    assert_eq!(runtime.manager.outcome(), Outcome::Succeeded);
    runtime.close_input(0).unwrap();
    runtime
        .drive_until(Duration::from_secs(5), |r| {
            r.events().any(|e| matches!(e, Event::Completed { .. }))
        })
        .unwrap();
    assert_eq!(runtime.live_count(), 0);
}
#[test]
fn real_failures_restart_with_distinct_identities_and_stop_at_configured_budget() {
    let mut runtime=runtime("fail","Restart=on-failure\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\nRestartBurst=3\nRestartWindowTicks=1000\nHealthyResetTicks=1000\n");
    runtime.manager.start("beacon").unwrap();
    runtime
        .drive_until(Duration::from_secs(5), |r| {
            r.events()
                .filter(|e| matches!(e, Event::Completed { .. }))
                .count()
                == 3
                && r.live_count() == 0
                && r.manager.state(0) == Some(rustos_service_core::State::Failed)
        })
        .unwrap();
    let keys: Vec<_> = runtime
        .events()
        .filter_map(|e| match e {
            Event::Started { process, .. } => Some(*process),
            _ => None,
        })
        .collect();
    assert_eq!(keys.len(), 3);
    assert!(keys.windows(2).all(|k| k[0] != k[1]));
}

#[test]
fn idle_service_blocks_in_readiness_wait_and_stale_identity_cannot_kill_replacement() {
    let mut r = runtime("exit", "");
    r.manager.start("beacon").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events().any(|e| matches!(e, Event::Ready { .. }))
    })
    .unwrap();
    let old = r
        .events()
        .find_map(|e| match e {
            Event::Started { process, .. } => Some(*process),
            _ => None,
        })
        .unwrap();
    let waits = r.waits;
    let error = r
        .drive_until(Duration::from_millis(60), |_| false)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(r.waits - waits <= 3, "idle event loop is spinning");
    r.close_input(0).unwrap();
    r.drive_until(Duration::from_secs(5), |r| r.live_count() == 0)
        .unwrap();
    r.manager.start("beacon").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events()
            .filter(|e| matches!(e, Event::Ready { .. }))
            .count()
            == 2
    })
    .unwrap();
    assert_eq!(
        r.terminate(old).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(r.live_count(), 1);
    assert!(matches!(
        r.manager.state(0),
        Some(rustos_service_core::State::Running(_))
    ));
}
#[test]
fn blocked_launch_worker_does_not_hold_watchdog_or_late_child_cleanup() {
    use std::sync::{Arc, Condvar, Mutex};
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0 .0.lock().unwrap() = false;
            self.0 .1.notify_all();
        }
    }
    let executable = env!("CARGO_BIN_EXE_service-fixture");
    let a=format!("Version=1\nKind=service\nName=a\nExec={executable}\nArg=wedge\nReady=notify\nStartTimeoutTicks=100\nWatchdogTicks=20\n");
    let b=format!("Version=1\nKind=service\nName=b\nExec={executable}\nArg=never-ready\nReady=notify\nStartTimeoutTicks=8\n");
    let gate = Arc::new((Mutex::new(true), Condvar::new()));
    let worker_gate = gate.clone();
    let mut r = Runtime::with_launch_hook(
        Catalog::load(&[("A.UNIT", a.as_bytes()), ("B.UNIT", b.as_bytes())]).unwrap(),
        move |start| {
            if start.unit == 1 {
                let guard = worker_gate.0.lock().unwrap();
                drop(worker_gate.1.wait_while(guard, |blocked| *blocked).unwrap());
            }
        },
    )
    .unwrap();
    let release = Release(gate); // Releases before Runtime cleanup even during panic.
    r.manager.start("a").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events().any(|e| matches!(e, Event::Ready { unit: 0 }))
    })
    .unwrap();
    r.manager.start("b").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events()
            .any(|e| matches!(e, Event::Completed { unit: 0, .. }))
    })
    .unwrap();
    assert_eq!(r.pending_count(), 1);
    assert_eq!(r.manager.outcome(), Outcome::Running);
    assert!(r.manager.run_status(1).unwrap().failure.is_some());
    drop(release);
    r.drive_until(Duration::from_secs(5), |r| {
        r.events()
            .any(|e| matches!(e, Event::Completed { unit: 1, .. }))
            && r.live_count() == 0
    })
    .unwrap();
    assert_eq!(r.manager.outcome(), Outcome::Failed);
    assert_eq!(r.pending_count(), 0);
}
#[test]
fn real_log_flood_is_bounded_while_another_child_watchdog_completes() {
    let executable = env!("CARGO_BIN_EXE_service-fixture");
    let a=format!("Version=1\nKind=service\nName=a\nExec={executable}\nArg=flood\nReady=notify\nStartTimeoutTicks=100\nWatchdogTicks=40\n");
    let b=format!("Version=1\nKind=service\nName=b\nExec={executable}\nArg=wedge\nReady=notify\nStartTimeoutTicks=100\nWatchdogTicks=20\n");
    let boot = b"Version=1\nKind=target\nName=boot\nRequires=a\nRequires=b\n";
    let mut r = Runtime::new(
        Catalog::load(&[
            ("A.UNIT", a.as_bytes()),
            ("B.UNIT", b.as_bytes()),
            ("BOOT.UNIT", boot),
        ])
        .unwrap(),
    )
    .unwrap();
    r.manager.start("boot").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events()
            .any(|e| matches!(e, Event::Completed { unit: 1, .. }))
    })
    .unwrap();
    assert!(matches!(
        r.manager.state(0),
        Some(rustos_service_core::State::Running(_))
    ));
    assert!(!r.journal.is_empty());
    assert!(r.journal.len() <= 256);
    assert!(r.events().any(|e| matches!(
        e,
        Event::Completed {
            unit: 1,
            cause: rustos_service_core::Completion::Killed,
            ..
        }
    )));
}
#[test]
fn invalid_stream_length_terminates_and_collects_the_owned_child() {
    let mut r = runtime("malformed", "");
    r.manager.start("beacon").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events().any(|e| matches!(e, Event::Completed { .. }))
    })
    .unwrap();
    assert!(r.events().any(|e| matches!(e, Event::ProtocolError { .. })));
    assert_eq!(r.live_count(), 0);
}

#[test]
fn launch_worker_backpressure_queues_admitted_units_instead_of_failing_the_boot_job() {
    use std::sync::{Arc, Condvar, Mutex};
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0 .0.lock().unwrap() = false;
            self.0 .1.notify_all();
        }
    }
    let executable = env!("CARGO_BIN_EXE_service-fixture");
    let mut sources:Vec<(String,String)>=(0..8).map(|n|(format!("U{n}.UNIT"),format!("Version=1\nKind=service\nName=u{n}\nExec={executable}\nArg=notify\nReady=notify\nStartTimeoutTicks=1000\n"))).collect();
    sources.push((
        "BOOT.UNIT".into(),
        format!(
            "Version=1\nKind=target\nName=boot\n{}",
            (0..8)
                .map(|n| format!("Requires=u{n}\n"))
                .collect::<String>()
        ),
    ));
    let views: Vec<_> = sources
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_bytes()))
        .collect();
    let gate = Arc::new((Mutex::new(true), Condvar::new()));
    let worker_gate = gate.clone();
    let mut r = Runtime::with_launch_hook(Catalog::load(&views).unwrap(), move |_| {
        let locked = worker_gate.0.lock().unwrap();
        drop(
            worker_gate
                .1
                .wait_while(locked, |blocked| *blocked)
                .unwrap(),
        );
    })
    .unwrap();
    let release = Release(gate);
    r.manager.start("boot").unwrap();
    r.drive_until(Duration::from_millis(100), |r| r.pending_count() == 8)
        .unwrap();
    assert!(!r.events().any(|e| matches!(e, Event::SpawnFailed { .. })));
    drop(release);
    r.drive_until(Duration::from_secs(5), |r| {
        r.manager.outcome() == Outcome::Succeeded
    })
    .unwrap();
    assert_eq!(r.live_count(), 8);
    assert_eq!(r.pending_count(), 0);
}

#[test]
fn cooperative_host_child_receives_stop_and_exits_without_a_kill() {
    let mut r = runtime("cooperative", "StopTimeoutTicks=20\nRestart=always\n");
    r.manager.start("beacon").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events().any(|e| matches!(e, Event::Ready { .. }))
    })
    .unwrap();
    r.manager.stop("beacon").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.live_count() == 0 && r.manager.outcome() == Outcome::Succeeded
    })
    .unwrap();
    let completed = r
        .events()
        .find_map(|e| {
            if let Event::Completed { cause, .. } = e {
                Some(*cause)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        completed,
        rustos_service_core::Completion::Exit(0),
        "cooperative stop must deliver a frame, not kill"
    );
    assert_eq!(r.manager.next_deadline(), None);
    assert_eq!(r.pending_count(), 0);
}
#[test]
fn ignored_host_stop_waits_until_policy_deadline_before_forcing_termination() {
    let mut r = runtime("wedge", "StopTimeoutTicks=5\nRestart=always\n");
    r.manager.start("beacon").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events().any(|e| matches!(e, Event::Ready { .. }))
    })
    .unwrap();
    let stop_tick = r.tick();
    r.manager.stop("beacon").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.live_count() == 0 && r.manager.outcome() == Outcome::Succeeded
    })
    .unwrap();
    let (cause, tick) = r
        .events()
        .find_map(|e| {
            if let Event::Completed { cause, tick, .. } = e {
                Some((*cause, *tick))
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(cause, rustos_service_core::Completion::Killed);
    assert!(
        tick >= stop_tick + 5,
        "ignored stop killed before its configured grace period"
    );
    assert_eq!(
        r.manager.run_status(0).unwrap().failure,
        Some(rustos_service_core::manager::Failure::StopTimeout)
    );
}

#[test]
fn host_shutdown_collects_cooperative_dependents_before_their_prerequisites() {
    let executable = env!("CARGO_BIN_EXE_service-fixture");
    let a=format!("Version=1\nKind=service\nName=a\nExec={executable}\nArg=cooperative\nReady=notify\nRestart=always\nStopTimeoutTicks=50\n");
    let b=format!("Version=1\nKind=service\nName=b\nExec={executable}\nArg=cooperative\nReady=notify\nRequires=a\nStopTimeoutTicks=50\n");
    let mut r =
        Runtime::new(Catalog::load(&[("A.UNIT", a.as_bytes()), ("B.UNIT", b.as_bytes())]).unwrap())
            .unwrap();
    r.manager.start("b").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events()
            .filter(|e| matches!(e, Event::Ready { .. }))
            .count()
            == 2
    })
    .unwrap();
    r.manager.shutdown(100).unwrap();
    r.drive_until(Duration::from_secs(5), |r| r.manager.shutdown_ready())
        .unwrap();
    let exits: Vec<_> = r
        .events()
        .filter_map(|e| {
            if let Event::Completed { unit, cause, .. } = e {
                Some((*unit, *cause))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        exits,
        [
            (1, rustos_service_core::Completion::Exit(0)),
            (0, rustos_service_core::Completion::Exit(0))
        ]
    );
    assert_eq!(r.live_count(), 0);
    assert_eq!(r.pending_count(), 0);
    assert_eq!(r.manager.next_deadline(), None);
}
#[test]
fn host_shutdown_global_deadline_forces_hung_dependents_and_all_remaining_children() {
    let executable = env!("CARGO_BIN_EXE_service-fixture");
    let a=format!("Version=1\nKind=service\nName=a\nExec={executable}\nArg=wedge\nReady=notify\nRestart=always\nStopTimeoutTicks=50\n");
    let b = a.replace("Name=a", "Name=b") + "Requires=a\n";
    let mut r =
        Runtime::new(Catalog::load(&[("A.UNIT", a.as_bytes()), ("B.UNIT", b.as_bytes())]).unwrap())
            .unwrap();
    r.manager.start("b").unwrap();
    r.drive_until(Duration::from_secs(5), |r| {
        r.events()
            .filter(|e| matches!(e, Event::Ready { .. }))
            .count()
            == 2
    })
    .unwrap();
    let before = r.tick();
    r.manager.shutdown(5).unwrap();
    r.drive_until(Duration::from_secs(5), |r| r.manager.shutdown_ready())
        .unwrap();
    let exits: Vec<_> = r
        .events()
        .filter_map(|e| {
            if let Event::Completed { cause, tick, .. } = e {
                Some((*cause, *tick))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(exits.len(), 2);
    assert!(exits.iter().all(
        |(cause, tick)| *cause == rustos_service_core::Completion::Killed
            && *tick >= before + 5
            && *tick < before + 50
    ));
    assert_eq!(r.live_count(), 0);
    assert_eq!(r.pending_count(), 0);
}
