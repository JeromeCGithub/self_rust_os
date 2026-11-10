use rustos_service_abi::{
    control::{Access, Operation, Request},
    replies::{Body, Code, JobPhase, Phase},
    Level,
};
use rustos_service_core::{
    control::Session,
    graph::Catalog,
    journal::Journal,
    manager::{Action, Manager},
    Completion, ProcessKey,
};
const PEER: ProcessKey = ProcessKey {
    pid: 9,
    generation: 1,
};
const CHILD: ProcessKey = ProcessKey {
    pid: 10,
    generation: 1,
};
fn manager() -> Manager {
    Manager::new(
        Catalog::load(&[(
            "A.UNIT",
            b"Version=1\nKind=service\nName=a\nExec=/BIN/A.SVC\n",
        )])
        .unwrap(),
    )
    .unwrap()
}
fn request(id: u64, operation: Operation<'_>) -> Request<'_> {
    Request { id, operation }
}
#[test]
fn denied_control_has_no_lookup_job_or_process_effect_and_readonly_can_query() {
    let mut m = manager();
    let j = Journal::<2>::new();
    let mut s = Session::new(PEER, 1, Access::ReadOnly).unwrap();
    for (id, op) in [(1, Operation::Stop("missing")), (2, Operation::Shutdown)] {
        assert_eq!(
            s.dispatch(PEER, 1, request(id, op), &mut m, &j).body,
            Body::Error {
                code: Code::Denied,
                detail: 0
            }
        );
        assert_eq!(m.job_id(), None);
        assert_eq!(m.next_action(0), Ok(None));
    }
    assert!(matches!(
        s.dispatch(
            PEER,
            1,
            request(3, Operation::Status { index: 0 }),
            &mut m,
            &j
        )
        .body,
        Body::Status {
            name: "a",
            phase: Phase::Inactive,
            ..
        }
    ));
    assert_eq!(
        s.dispatch(
            PEER,
            1,
            request(4, Operation::Status { index: 1 }),
            &mut m,
            &j
        )
        .body,
        Body::End
    );
}
#[test]
fn lifecycle_job_reply_never_acknowledges_stop_before_child_completion() {
    let mut m = manager();
    let j = Journal::<2>::new();
    let mut s = Session::new(PEER, 1, Access::Manage).unwrap();
    let Body::Job { job, .. } = s
        .dispatch(PEER, 1, request(1, Operation::Start("a")), &mut m, &j)
        .body
    else {
        panic!("start job")
    };
    let Some(Action::Start(start)) = m.next_action(0).unwrap() else {
        panic!("spawn")
    };
    m.spawned(start, CHILD, 0).unwrap();
    m.next_action(0).unwrap();
    assert_eq!(
        s.dispatch(PEER, 1, request(2, Operation::Job { job }), &mut m, &j)
            .body,
        Body::Job {
            job,
            phase: JobPhase::Succeeded
        }
    );
    let Body::Job { job: stop, .. } = s
        .dispatch(PEER, 1, request(3, Operation::Stop("a")), &mut m, &j)
        .body
    else {
        panic!("stop job")
    };
    assert!(matches!(
        m.next_action(1).unwrap(),
        Some(Action::NotifyStop { process: CHILD, .. })
    ));
    assert_eq!(
        s.dispatch(
            PEER,
            1,
            request(4, Operation::Job { job: stop }),
            &mut m,
            &j
        )
        .body,
        Body::Job {
            job: stop,
            phase: JobPhase::Running
        }
    );
    m.completed(CHILD, Completion::Killed, 2);
    m.next_action(2).unwrap();
    assert_eq!(
        s.dispatch(
            PEER,
            1,
            request(5, Operation::Job { job: stop }),
            &mut m,
            &j
        )
        .body,
        Body::Job {
            job: stop,
            phase: JobPhase::Succeeded
        }
    );
    assert_eq!(
        s.dispatch(PEER, 1, request(6, Operation::Job { job }), &mut m, &j)
            .body,
        Body::Error {
            code: Code::Stale,
            detail: 0
        }
    );
}
#[test]
fn logs_fragment_without_splitting_unicode_and_detect_evicted_continuations() {
    let mut m = manager();
    let mut j = Journal::<1>::new();
    let mut s = Session::new(PEER, 1, Access::ReadOnly).unwrap();
    let text = "é".repeat(100);
    j.push(0, 1, CHILD, 12, Level::Info, &text).unwrap();
    let Body::Log {
        sequence,
        offset,
        total,
        text: first,
        ..
    } = s
        .dispatch(
            PEER,
            1,
            request(
                1,
                Operation::Logs {
                    cursor: 0,
                    offset: 0,
                },
            ),
            &mut m,
            &j,
        )
        .body
    else {
        panic!("first chunk")
    };
    assert_eq!(first.len(), 184);
    assert_eq!((sequence, offset, total), (1, 0, 200));
    let Body::Log { text: second, .. } = s
        .dispatch(
            PEER,
            1,
            request(
                2,
                Operation::Logs {
                    cursor: 1,
                    offset: 184,
                },
            ),
            &mut m,
            &j,
        )
        .body
    else {
        panic!("tail")
    };
    assert_eq!(second.len(), 16);
    assert_eq!(
        s.dispatch(
            PEER,
            1,
            request(
                3,
                Operation::Logs {
                    cursor: 1,
                    offset: 1
                }
            ),
            &mut m,
            &j
        )
        .body,
        Body::Error {
            code: Code::Invalid,
            detail: 0
        }
    );
    assert_eq!(
        s.dispatch(
            PEER,
            1,
            request(
                4,
                Operation::Logs {
                    cursor: 1,
                    offset: 0
                }
            ),
            &mut m,
            &j
        )
        .body,
        Body::End
    );
    j.push(0, 2, CHILD, 13, Level::Warning, "replacement")
        .unwrap();
    assert_eq!(
        s.dispatch(
            PEER,
            1,
            request(
                5,
                Operation::Logs {
                    cursor: 1,
                    offset: 184
                }
            ),
            &mut m,
            &j
        )
        .body,
        Body::Error {
            code: Code::Gap,
            detail: 2
        }
    );
}

#[test]
fn restart_reply_tracks_the_complete_stop_and_replacement_job() {
    let mut m = manager();
    let j = Journal::<2>::new();
    let mut s = Session::new(PEER, 1, Access::Manage).unwrap();
    m.start("a").unwrap();
    let Some(Action::Start(first)) = m.next_action(0).unwrap() else {
        panic!("first")
    };
    m.spawned(first, CHILD, 0).unwrap();
    m.next_action(0).unwrap();
    let Body::Job { job, .. } = s
        .dispatch(PEER, 1, request(1, Operation::Restart("a")), &mut m, &j)
        .body
    else {
        panic!("restart rejected")
    };
    assert!(matches!(
        m.next_action(1).unwrap(),
        Some(Action::NotifyStop { process: CHILD, .. })
    ));
    m.completed(CHILD, Completion::Killed, 2);
    let Some(Action::Start(replacement)) = m.next_action(2).unwrap() else {
        panic!("replacement")
    };
    assert_eq!(m.job_id(), Some(job));
    assert_ne!(first.run, replacement.run);
    m.spawned(
        replacement,
        ProcessKey {
            pid: 11,
            generation: 1,
        },
        2,
    )
    .unwrap();
    m.next_action(2).unwrap();
    assert_eq!(
        s.dispatch(PEER, 1, request(2, Operation::Job { job }), &mut m, &j)
            .body,
        Body::Job {
            job,
            phase: JobPhase::Succeeded
        }
    );
}

#[test]
fn management_shutdown_replaces_pending_activation_and_retains_one_terminal_job() {
    let mut m = manager();
    let journal = Journal::<2>::new();
    m.start("a").unwrap();
    let Some(Action::Start(pending)) = m.next_action(0).unwrap() else {
        panic!("pending launch")
    };
    let mut session = Session::new(PEER, 1, Access::Manage).unwrap();
    let Body::Job { job, phase } = session
        .dispatch(PEER, 1, request(1, Operation::Shutdown), &mut m, &journal)
        .body
    else {
        panic!("authorized shutdown must admit a terminal job")
    };
    assert_eq!(phase, JobPhase::Running);
    assert!(m.launch_cancelled(pending));
    assert_eq!(
        session
            .dispatch(PEER, 1, request(2, Operation::Shutdown), &mut m, &journal)
            .body,
        Body::Job {
            job,
            phase: JobPhase::Running
        }
    );
    assert_eq!(
        m.start("a"),
        Err(rustos_service_core::transaction::Error::Busy)
    );
    m.next_action(1).unwrap();
    assert!(!m.shutdown_ready());
    m.spawn_failed(pending, 1).unwrap();
    m.next_action(1).unwrap();
    assert!(m.shutdown_ready());
    assert_eq!(
        session
            .dispatch(
                PEER,
                1,
                request(3, Operation::Job { job }),
                &mut m,
                &journal
            )
            .body,
        Body::Job {
            job,
            phase: JobPhase::Succeeded
        }
    );
}
