use rustos_service_core::{graph::Catalog, transaction::*};
fn engine(files: &[(&str, &str)]) -> Engine {
    let sources: Vec<_> = files
        .iter()
        .map(|(name, text)| (*name, text.as_bytes()))
        .collect();
    Engine::new(Catalog::load(&sources).unwrap())
}
fn service(name: &str, edges: &str) -> String {
    format!("Version=1\nKind=service\nName={name}\nExec=/BIN/GLOW.SVC\n{edges}")
}
fn id(e: &Engine, name: &str) -> usize {
    e.catalog().lookup(name).unwrap()
}
fn complete(e: &mut Engine) -> Vec<Command> {
    let mut commands = Vec::new();
    for _ in 0..256 {
        let Some(command) = e.next_command() else {
            assert_ne!(e.outcome(), Outcome::Running, "transaction stalled");
            return commands;
        };
        commands.push(command);
        match command {
            Command::Start(unit) => e.started(e.job_id().unwrap(), unit, Ok(())).unwrap(),
            Command::Stop(unit) => e.stopped(e.job_id().unwrap(), unit).unwrap(),
        }
    }
    panic!("unbounded transaction");
}
#[test]
fn prerequisites_wait_for_activation_and_independent_starts_can_overlap() {
    let a = service("a", "");
    let b = service("b", "Requires=a\n");
    let c = service("c", "");
    let mut e = engine(&[
        ("A.UNIT", &a),
        ("B.UNIT", &b),
        ("C.UNIT", &c),
        (
            "BOOT.UNIT",
            "Version=1\nKind=target\nName=boot\nRequires=b\nWants=c\n",
        ),
    ]);
    let (a, b, c) = (id(&e, "a"), id(&e, "b"), id(&e, "c"));
    e.begin_start("boot").unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    assert_eq!(
        e.next_command(),
        Some(Command::Start(c)),
        "independent service must not wait for a"
    );
    assert_eq!(e.next_command(), None);
    e.started(e.job_id().unwrap(), a, Ok(())).unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(b)));
    e.started(e.job_id().unwrap(), b, Ok(())).unwrap();
    e.started(e.job_id().unwrap(), c, Ok(())).unwrap();
    assert!(complete(&mut e).is_empty());
    assert_eq!(e.outcome(), Outcome::Succeeded);
}
#[test]
fn optional_failure_degrades_target_but_after_is_only_a_settled_barrier() {
    let a = service("a", "");
    let b = service("b", "After=a\n");
    let mut e = engine(&[
        ("A.UNIT", &a),
        ("B.UNIT", &b),
        (
            "BOOT.UNIT",
            "Version=1\nKind=target\nName=boot\nRequires=b\nWants=a\n",
        ),
    ]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    e.begin_start("boot").unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    e.started(e.job_id().unwrap(), a, Err(())).unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(b)));
    e.started(e.job_id().unwrap(), b, Ok(())).unwrap();
    complete(&mut e);
    assert_eq!(e.outcome(), Outcome::Degraded);
    assert!(e.active(b));
    assert!(!e.active(a));
}
#[test]
fn required_failure_never_starts_dependent_and_rolls_back_exclusive_services() {
    let a = service("a", "");
    let b = service("b", "");
    let c = service("c", "Requires=b\n");
    let mut e = engine(&[
        ("A.UNIT", &a),
        ("B.UNIT", &b),
        ("C.UNIT", &c),
        (
            "BOOT.UNIT",
            "Version=1\nKind=target\nName=boot\nRequires=a\nRequires=c\n",
        ),
    ]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    e.begin_start("boot").unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    e.started(e.job_id().unwrap(), a, Ok(())).unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(b)));
    e.started(e.job_id().unwrap(), b, Err(())).unwrap();
    assert_eq!(e.next_command(), Some(Command::Stop(a)));
    assert_eq!(e.outcome(), Outcome::Running);
    assert_eq!(
        e.next_command(),
        None,
        "stop completion must be acknowledged"
    );
    e.stopped(e.job_id().unwrap(), a).unwrap();
    assert!(complete(&mut e).is_empty());
    assert_eq!(e.outcome(), Outcome::Failed);
}
#[test]
fn rollback_and_explicit_stop_preserve_services_owned_by_another_root() {
    let a = service("a", "");
    let b = service("b", "Requires=a\n");
    let mut e = engine(&[("A.UNIT", &a), ("B.UNIT", &b)]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    e.begin_start("a").unwrap();
    assert_eq!(complete(&mut e), [Command::Start(a)]);
    e.begin_start("b").unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(b)));
    e.started(e.job_id().unwrap(), b, Err(())).unwrap();
    assert!(complete(&mut e).is_empty());
    assert!(e.active(a));
    e.begin_start("b").unwrap();
    assert_eq!(complete(&mut e), [Command::Start(b)]);
    e.begin_stop("b").unwrap();
    assert_eq!(complete(&mut e), [Command::Stop(b)]);
    assert!(e.active(a));
    e.begin_stop("a").unwrap();
    assert_eq!(complete(&mut e), [Command::Stop(a)]);
}
#[test]
fn cleanup_uses_reverse_actual_activation_order() {
    let a = service("a", "");
    let b = service("b", "Requires=a\n");
    let mut e = engine(&[("A.UNIT", &a), ("B.UNIT", &b)]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    e.begin_start("b").unwrap();
    assert_eq!(complete(&mut e), [Command::Start(a), Command::Start(b)]);
    e.begin_stop("b").unwrap();
    assert_eq!(complete(&mut e), [Command::Stop(b), Command::Stop(a)]);
}
#[test]
fn duplicate_requests_coalesce_conflicts_and_stale_results_are_rejected() {
    let a = service("a", "");
    let b = service("b", "");
    let mut e = engine(&[("A.UNIT", &a), ("B.UNIT", &b)]);
    let a = id(&e, "a");
    let job = e.begin_start("a").unwrap();
    assert_eq!(e.begin_start("a"), Ok(job));
    assert_eq!(e.begin_stop("a"), Err(Error::Busy));
    assert_eq!(e.begin_start("b"), Err(Error::Busy));
    assert_eq!(
        e.started(e.job_id().unwrap(), a, Ok(())),
        Err(Error::InvalidResult)
    );
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    e.started(e.job_id().unwrap(), a, Ok(())).unwrap();
    assert_eq!(
        e.started(e.job_id().unwrap(), a, Ok(())),
        Err(Error::InvalidResult)
    );
    complete(&mut e);
    assert_eq!(e.begin_start("missing"), Err(Error::Unknown));
}
#[test]
fn failed_optional_group_does_not_keep_its_exclusive_prerequisites_running() {
    let a = service("a", "");
    let b = service("b", "");
    let mut e = engine(&[
        ("A.UNIT", &a),
        ("B.UNIT", &b),
        (
            "OPT.UNIT",
            "Version=1\nKind=target\nName=optional\nRequires=a\nRequires=b\n",
        ),
        (
            "BOOT.UNIT",
            "Version=1\nKind=target\nName=boot\nWants=optional\n",
        ),
    ]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    e.begin_start("boot").unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    e.started(e.job_id().unwrap(), a, Ok(())).unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(b)));
    e.started(e.job_id().unwrap(), b, Err(())).unwrap();
    assert_eq!(complete(&mut e), [Command::Stop(a)]);
    assert_eq!(e.outcome(), Outcome::Degraded);
}

#[test]
fn stale_transaction_results_cannot_complete_a_new_run_of_the_same_unit() {
    let a = service("a", "");
    let mut e = engine(&[("A.UNIT", &a)]);
    let a = id(&e, "a");
    let old = e.begin_start("a").unwrap();
    complete(&mut e);
    let old_stop = e.begin_stop("a").unwrap();
    complete(&mut e);
    let new = e.begin_start("a").unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    assert_eq!(e.started(old, a, Ok(())), Err(Error::InvalidResult));
    assert!(!e.active(a));
    e.started(new, a, Ok(())).unwrap();
    complete(&mut e);
    let stop = e.begin_stop("a").unwrap();
    assert_eq!(e.next_command(), Some(Command::Stop(a)));
    assert_eq!(e.stopped(old_stop, a), Err(Error::InvalidResult));
    assert!(e.active(a));
    e.stopped(stop, a).unwrap();
    complete(&mut e);
}
#[test]
fn service_headroom_counts_all_retained_roots_and_is_released_on_stop() {
    let files: Vec<_> = (0..57)
        .map(|n| (format!("U{n}.UNIT"), service(&format!("u{n:02}"), "")))
        .collect();
    let mut boot = "Version=1\nKind=target\nName=boot\n".to_owned();
    for n in 0..56 {
        boot.push_str(&format!("Wants=u{n:02}\n"));
    }
    let mut sources: Vec<_> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    sources.push(("BOOT.UNIT", &boot));
    let mut e = engine(&sources);
    e.begin_start("boot").unwrap();
    assert_eq!(complete(&mut e).len(), 56);
    assert_eq!(e.begin_start("u56"), Err(Error::Limit));
    e.begin_stop("boot").unwrap();
    assert_eq!(complete(&mut e).len(), 56);
    e.begin_start("u56").unwrap();
    assert_eq!(complete(&mut e).len(), 1);
}

#[test]
fn observed_prerequisite_loss_blocks_dependents_and_waits_for_late_siblings() {
    let a = service("a", "");
    let b = service("b", "Requires=a\n");
    let c = service("c", "");
    let mut e = engine(&[
        ("A.UNIT", &a),
        ("B.UNIT", &b),
        ("C.UNIT", &c),
        (
            "BOOT.UNIT",
            "Version=1\nKind=target\nName=boot\nRequires=b\nWants=c\n",
        ),
    ]);
    let (a, b, c) = (id(&e, "a"), id(&e, "b"), id(&e, "c"));
    let job = e.begin_start("boot").unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    assert_eq!(e.next_command(), Some(Command::Start(c)));
    e.started(job, a, Ok(())).unwrap();
    e.unavailable(a).unwrap();
    assert_eq!(e.next_command(), None, "late sibling result must drain");
    assert!(!e.active(b));
    e.started(job, c, Ok(())).unwrap();
    assert_eq!(complete(&mut e), [Command::Stop(c)]);
    assert_eq!(e.outcome(), Outcome::Failed);
    assert!(!e.wanted(a));
}

#[test]
fn post_activation_loss_keeps_desire_without_binding_dependents_to_liveness() {
    let a = service("a", "");
    let b = service("b", "Requires=a\n");
    let mut e = engine(&[("A.UNIT", &a), ("B.UNIT", &b)]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    e.begin_start("b").unwrap();
    complete(&mut e);
    e.unavailable(a).unwrap();
    assert!(!e.active(a));
    assert!(e.active(b));
    assert!(e.wanted(a));
    assert_eq!(
        e.outcome(),
        Outcome::Succeeded,
        "historical activation result"
    );
    e.restored(a).unwrap();
    assert!(e.restored(a).is_err(), "duplicate activation order entry");
    e.begin_stop("b").unwrap();
    assert_eq!(
        complete(&mut e),
        [Command::Stop(b), Command::Stop(a)],
        "M4 preserves reverse dependency order even after prerequisite restart"
    );
    assert!(e.restored(a).is_err(), "unowned unit must not restart");
}

#[test]
fn loss_during_cleanup_acknowledges_stop_without_requiring_a_second_event() {
    let a = service("a", "");
    let mut e = engine(&[("A.UNIT", &a)]);
    let a = id(&e, "a");
    e.begin_start("a").unwrap();
    complete(&mut e);
    e.begin_stop("a").unwrap();
    assert_eq!(e.next_command(), Some(Command::Stop(a)));
    e.unavailable(a).unwrap();
    assert!(complete(&mut e).is_empty());
    assert!(!e.wanted(a));
    assert!(!e.active(a));
    assert!(e.unavailable(64).is_err());
}

#[test]
fn admitted_stop_withdraws_desire_before_cleanup_while_preserving_other_root_references() {
    let a = service("a", "");
    let b = service("b", "Requires=a\n");
    let mut e = engine(&[("A.UNIT", &a), ("B.UNIT", &b)]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    e.begin_start("a").unwrap();
    complete(&mut e);
    e.begin_start("b").unwrap();
    complete(&mut e);
    e.begin_stop("b").unwrap();
    assert!(
        !e.wanted(b),
        "stop intent disarms deadlines before native completion"
    );
    assert!(e.wanted(a), "another retained root still owns a");
    assert!(
        e.active(b),
        "withdrawn desire is not a forged process completion"
    );
    complete(&mut e);
    e.begin_stop("a").unwrap();
    assert!(!e.wanted(a));
}

#[test]
fn control_endpoints_count_against_the_shared_native_handle_headroom() {
    let mut files: Vec<(String, String)> = (0..38)
        .map(|n| {
            (
                format!("U{n:02}.UNIT"),
                service(&format!("u{n:02}"), "ControlAccess=status\n"),
            )
        })
        .collect();
    files.push((
        "BOOT.UNIT".into(),
        format!(
            "Version=1\nKind=target\nName=boot\n{}",
            (0..37)
                .map(|n| format!("Requires=u{n:02}\n"))
                .collect::<String>()
        ),
    ));
    let views: Vec<_> = files
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    let mut e = engine(&views);
    e.begin_start("boot").unwrap();
    assert_eq!(complete(&mut e).len(), 37);
    // 37*3=111 parent handles; the protected admission budget is 112.
    assert_eq!(e.begin_start("u37"), Err(Error::Limit));
    assert_eq!(e.next_command(), None);
}

#[test]
fn restart_has_one_job_and_stops_only_its_service_even_when_another_root_retains_it() {
    let a = service("a", "");
    let b = service("b", "Requires=a\n");
    let mut e = engine(&[("A.UNIT", &a), ("B.UNIT", &b)]);
    e.begin_start("b").unwrap();
    complete(&mut e);
    let a = id(&e, "a");
    let b = id(&e, "b");
    let job = e.begin_restart("a").unwrap();
    assert_eq!(e.next_command(), Some(Command::Stop(a)));
    assert_eq!(e.next_command(), None);
    assert_eq!(e.outcome(), Outcome::Running);
    assert!(e.active(b));
    assert_eq!(e.begin_stop("b"), Err(Error::Busy));
    e.stopped(job, a).unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    assert_eq!(e.job_id(), Some(job));
    e.started(job, a, Ok(())).unwrap();
    assert_eq!(e.next_command(), None);
    assert_eq!(e.outcome(), Outcome::Succeeded);
    assert!(e.active(a) && e.active(b));
}

#[test]
fn scoped_stop_waits_for_exact_scope_cleanup_before_restarting() {
    let a = service("a", "");
    let mut e = engine(&[("A.UNIT", &a)]);
    let a = id(&e, "a");
    e.begin_start("a").unwrap();
    assert_eq!(complete(&mut e), [Command::Start(a)]);
    e.bind_scope(a, 101).unwrap();
    let job = e.begin_restart("a").unwrap();
    assert_eq!(e.next_command(), Some(Command::Stop(a)));
    e.stopped(job, a).unwrap();
    assert!(!e.active(a));
    assert_eq!(
        e.next_command(),
        None,
        "process exit must not bypass scope cleanup"
    );
    assert_eq!(e.outcome(), Outcome::Running);
    assert_eq!(e.scope_drained(a, 100), Err(Error::InvalidResult));
    assert_eq!(e.next_command(), None);
    e.scope_drained(a, 101).unwrap();
    assert_eq!(e.scope_drained(a, 101), Err(Error::InvalidResult));
    assert_eq!(e.next_command(), Some(Command::Start(a)));
    e.started(job, a, Ok(())).unwrap();
    assert_eq!(e.next_command(), None);
    assert_eq!(e.outcome(), Outcome::Succeeded);
}

#[test]
fn scoped_unexpected_exit_keeps_resources_until_scope_drains() {
    let a = service("a", "");
    let mut e = engine(&[("A.UNIT", &a)]);
    let a = id(&e, "a");
    e.begin_start("a").unwrap();
    complete(&mut e);
    e.bind_scope(a, 202).unwrap();
    let job = e.begin_stop("a").unwrap();
    assert_eq!(e.next_command(), Some(Command::Stop(a)));
    e.unavailable(a).unwrap();
    assert!(!e.active(a));
    assert_eq!(e.next_command(), None);
    assert_eq!(e.outcome(), Outcome::Running);
    assert_eq!(e.stopped(job, a), Err(Error::InvalidResult));
    e.scope_drained(a, 202).unwrap();
    assert_eq!(e.next_command(), None);
    assert_eq!(e.outcome(), Outcome::Succeeded);
}

#[test]
fn scoped_crash_and_retry_cannot_relaunch_before_old_scope_drains() {
    let a = service("a", "");
    let mut e = engine(&[("A.UNIT", &a)]);
    let a = id(&e, "a");
    e.begin_start("a").unwrap();
    complete(&mut e);
    e.bind_scope(a, 303).unwrap();
    e.retrying(a).unwrap();
    assert!(!e.active(a));
    e.begin_start("a").unwrap();
    assert_eq!(e.next_command(), None, "retry waits for previous scope");
    assert_eq!(e.outcome(), Outcome::Running);
    assert_eq!(e.restored(a), Err(Error::InvalidResult));
    assert_eq!(e.reserve_run(a), Err(Error::InvalidResult));
    e.scope_drained(a, 303).unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
}

#[test]
fn scope_close_observed_before_process_completion_does_not_finish_stop_early() {
    let a = service("a", "");
    let mut e = engine(&[("A.UNIT", &a)]);
    let a = id(&e, "a");
    e.begin_start("a").unwrap();
    complete(&mut e);
    e.bind_scope(a, 404).unwrap();
    let job = e.begin_restart("a").unwrap();
    assert_eq!(e.next_command(), Some(Command::Stop(a)));
    e.scope_drained(a, 404).unwrap();
    assert_eq!(e.scope_drained(a, 404), Err(Error::InvalidResult));
    assert_eq!(e.next_command(), None);
    assert_eq!(e.outcome(), Outcome::Running);
    e.stopped(job, a).unwrap();
    assert_eq!(e.next_command(), Some(Command::Start(a)));
}

#[test]
fn scoped_barrier_rejects_unowned_duplicate_and_invalid_scopes() {
    let a = service("a", "");
    let b = service("b", "");
    let mut e = engine(&[("A.UNIT", &a), ("B.UNIT", &b)]);
    let (a, b) = (id(&e, "a"), id(&e, "b"));
    assert_eq!(e.bind_scope(a, 1), Err(Error::InvalidResult));
    assert_eq!(e.bind_scope(64, 1), Err(Error::InvalidResult));
    e.begin_start("a").unwrap();
    complete(&mut e);
    e.begin_start("b").unwrap();
    complete(&mut e);
    assert_eq!(e.bind_scope(a, 0), Err(Error::InvalidResult));
    e.bind_scope(a, 505).unwrap();
    assert_eq!(e.bind_scope(a, 506), Err(Error::InvalidResult));
    assert_eq!(e.bind_scope(b, 505), Err(Error::InvalidResult));
    assert_eq!(e.scope_drained(b, 505), Err(Error::InvalidResult));
    assert_eq!(e.scope_drained(64, 505), Err(Error::InvalidResult));
    assert!(e.active(a) && e.active(b));
}
