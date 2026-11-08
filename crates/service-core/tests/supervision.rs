use rustos_service_core::*;
fn policy(restart: Restart) -> Policy {
    Policy {
        restart,
        delay: 2,
        maximum_delay: 8,
        burst: 3,
        window: 100,
        healthy_reset: 50,
    }
}
fn key(pid: u32) -> ProcessKey {
    ProcessKey { pid, generation: 1 }
}
fn running<const N: usize>(s: &mut Supervisor<N>, now: u64, pid: u32) -> Start {
    let start = s
        .next_start(now)
        .expect("eligible service must produce a launch");
    assert!(!s.spawned(start, key(pid), now).unwrap());
    start
}
#[test]
fn two_children_are_independent_and_second_failure_does_not_wait_for_first() {
    let mut s = Supervisor::new([policy(Restart::OnFailure); 2]).unwrap();
    let a = running(&mut s, 0, 10);
    let b = running(&mut s, 0, 11);
    assert_ne!(a.unit, b.unit);
    assert_eq!(s.next_start(0), None);
    assert!(s.completed(key(11), Completion::Fault(14), 1));
    assert_eq!(s.state(a.unit), Some(State::Running(key(10))));
    assert_eq!(s.next_deadline(), Some(3));
    assert_eq!(s.next_start(2), None);
    let again = running(&mut s, 3, 12);
    assert_eq!(again.unit, b.unit);
    assert_ne!(again.run, b.run);
    assert!(!s.completed(key(11), Completion::Exit(1), 4));
    assert_eq!(s.state(b.unit), Some(State::Running(key(12))));
}
#[test]
fn restart_storm_has_exponential_delay_and_a_terminal_limit() {
    let mut s = Supervisor::new([policy(Restart::Always)]).unwrap();
    for (now, pid, due) in [(0, 10, 2), (2, 11, 6), (6, 12, 14)] {
        running(&mut s, now, pid);
        assert!(s.completed(key(pid), Completion::Exit(0), now));
        assert_eq!(s.next_deadline(), Some(due));
        assert_eq!(s.next_start(due - 1), None);
    }
    assert_eq!(s.next_start(14), None);
    assert_eq!(s.state(0), Some(State::Failed));
    assert_eq!(s.next_deadline(), None);
}
#[test]
fn stop_suppresses_always_restart_and_cancels_backoff() {
    let mut s = Supervisor::new([policy(Restart::Always); 2]).unwrap();
    running(&mut s, 0, 10);
    running(&mut s, 0, 11);
    assert_eq!(s.stop(0), Ok(Some(key(10))));
    assert!(s.completed(key(10), Completion::Killed, 1));
    assert_eq!(s.state(0), Some(State::Inactive));
    s.completed(key(11), Completion::Exit(1), 1);
    assert_eq!(s.stop(1), Ok(None));
    assert_eq!(s.next_deadline(), None);
    assert_eq!(s.next_start(999), None);
}
#[test]
fn stop_during_spawn_requires_cleanup_of_late_success() {
    let mut s = Supervisor::new([policy(Restart::Always)]).unwrap();
    let launch = s.next_start(0).unwrap();
    assert_eq!(s.stop(0), Ok(None));
    assert_eq!(s.spawned(launch, key(10), 1), Ok(true));
    assert_eq!(s.state(0), Some(State::Stopping(key(10))));
    assert!(s.completed(key(10), Completion::Killed, 1));
    assert_eq!(s.state(0), Some(State::Inactive));
    assert!(s.spawned(launch, key(11), 2).is_err());
}
#[test]
fn reused_pid_does_not_match_old_generation() {
    let mut s = Supervisor::new([policy(Restart::Always)]).unwrap();
    let launch = s.next_start(0).unwrap();
    let newer = ProcessKey {
        pid: 10,
        generation: 2,
    };
    s.spawned(launch, newer, 0).unwrap();
    assert!(!s.completed(key(10), Completion::Killed, 1));
    assert_eq!(s.state(0), Some(State::Running(newer)));
}
#[test]
fn success_failure_and_spawn_failure_follow_policy() {
    for (restart, cause, expected) in [
        (Restart::Never, Completion::Exit(0), State::Inactive),
        (Restart::Never, Completion::Fault(13), State::Failed),
        (Restart::OnFailure, Completion::Exit(0), State::Inactive),
        (Restart::OnFailure, Completion::Exit(9), State::Backoff(3)),
        (Restart::OnFailure, Completion::Killed, State::Backoff(3)),
    ] {
        let mut s = Supervisor::new([policy(restart)]).unwrap();
        running(&mut s, 0, 10);
        s.completed(key(10), cause, 1);
        assert_eq!(s.state(0), Some(expected));
    }
    let mut s = Supervisor::new([policy(Restart::OnFailure)]).unwrap();
    let launch = s.next_start(0).unwrap();
    s.spawn_failed(launch, 0).unwrap();
    assert_eq!(s.next_deadline(), Some(2));
    assert!(s.spawn_failed(launch, 0).is_err());
}
#[test]
fn unhealthy_short_runs_do_not_reset_budget_but_healthy_run_does() {
    let mut p = policy(Restart::Always);
    p.burst = 1;
    let mut s = Supervisor::new([p]).unwrap();
    running(&mut s, 0, 10);
    s.completed(key(10), Completion::Exit(1), 50);
    assert!(s.next_start(52).is_some());
}
#[test]
fn validates_limits_and_never_wraps_deadlines() {
    for edit in 0..5 {
        let mut p = policy(Restart::Always);
        match edit {
            0 => p.delay = 0,
            1 => p.maximum_delay = 1,
            2 => p.burst = 0,
            3 => p.window = 0,
            _ => p.healthy_reset = u64::MAX,
        }
        assert!(Supervisor::new([p]).is_err());
    }
    let mut s = Supervisor::new([policy(Restart::Always)]).unwrap();
    running(&mut s, u64::MAX - 1, 10);
    s.completed(key(10), Completion::Exit(1), u64::MAX);
    assert_eq!(s.state(0), Some(State::Failed));
    assert_eq!(s.next_deadline(), None);
    assert!(s.stop(1).is_err());
}

#[test]
fn restart_limit_uses_a_rolling_window_across_old_window_boundary() {
    let mut p = policy(Restart::Always);
    p.healthy_reset = 1000;
    let mut s = Supervisor::new([p]).unwrap();
    for (start, exit, pid) in [(0, 88, 10), (90, 90, 11), (94, 98, 12), (106, 106, 13)] {
        running(&mut s, start, pid);
        s.completed(key(pid), Completion::Exit(1), exit);
    }
    assert_eq!(
        s.next_start(114),
        None,
        "starts at 90, 94, 106 still fill the preceding 100-tick window"
    );
    assert_eq!(s.state(0), Some(State::Failed));
}
#[test]
fn rolling_history_reuses_storage_when_old_attempts_expire() {
    let mut p = policy(Restart::Always);
    p.delay = 1;
    p.maximum_delay = 1;
    p.window = 5;
    p.healthy_reset = 1000;
    let mut s = Supervisor::new([p]).unwrap();
    for index in 0..128 {
        running(&mut s, index * 2, index as u32 + 10);
        s.completed(key(index as u32 + 10), Completion::Exit(1), index * 2);
    }
}
#[test]
fn restart_history_bound_is_validated_before_start() {
    let mut p = policy(Restart::Always);
    p.burst = 33;
    assert!(Supervisor::new([p]).is_err());
    p.burst = 32;
    p.delay = 1;
    p.maximum_delay = 1;
    p.window = 1000;
    p.healthy_reset = 1000;
    let mut s = Supervisor::new([p]).unwrap();
    for index in 0..32 {
        running(&mut s, index, index as u32 + 10);
        s.completed(key(index as u32 + 10), Completion::Exit(1), index);
    }
    assert_eq!(s.next_start(32), None);
    assert_eq!(s.state(0), Some(State::Failed));
}

#[test]
fn inactive_catalog_only_starts_explicitly_admitted_services() {
    let mut s = Supervisor::new_stopped([policy(Restart::Always); 3]).unwrap();
    assert_eq!(s.next_start(0), None);
    assert_eq!(s.state(0), Some(State::Inactive));
    assert!(s.request_start(3).is_err());
    s.request_start(1).unwrap();
    let start = running(&mut s, 0, 10);
    assert_eq!(start.unit, 1);
    s.request_start(1).unwrap();
    assert_eq!(s.next_start(0), None);
    s.completed(key(10), Completion::Killed, 1);
    s.request_start(1).unwrap();
    assert_eq!(s.next_start(2), None, "activation cannot bypass backoff");
    assert_eq!(running(&mut s, 3, 11).unit, 1);
    assert_eq!(s.state(0), Some(State::Inactive));
    assert_eq!(s.state(2), Some(State::Inactive));
}

#[test]
fn reactivation_waits_for_cleanup_and_preserves_rate_limit_history() {
    let mut p = policy(Restart::Always);
    p.burst = 1;
    let mut s = Supervisor::new_stopped([p]).unwrap();
    s.request_start(0).unwrap();
    let start = s.next_start(0).unwrap();
    s.stop(0).unwrap();
    assert!(
        s.request_start(0).is_err(),
        "cancelled spawn must drain first"
    );
    assert_eq!(s.spawned(start, key(10), 0), Ok(true));
    assert!(
        s.request_start(0).is_err(),
        "stopping process still owns slot"
    );
    s.completed(key(10), Completion::Killed, 1);
    s.request_start(0).unwrap();
    assert_eq!(
        s.next_start(1),
        None,
        "stop/start cannot reset burst history"
    );
    assert_eq!(s.state(0), Some(State::Failed));
    s.request_start(0).unwrap();
    assert!(
        s.next_start(100).is_some(),
        "old attempts eventually expire"
    );
}
