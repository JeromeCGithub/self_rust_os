use rustos_service_core::{publication::*, Completion, ProcessKey};
fn record(pid: u32, generation: u32) -> EarlyExit {
    EarlyExit {
        process: ProcessKey { pid, generation },
        cause: Completion::Fault(6),
        tick: 17,
    }
}
#[test]
fn early_completion_survives_publication_and_cannot_match_a_reused_identity() {
    let mut exits = EarlyExits::<2>::new();
    exits.insert(record(3, 1)).unwrap();
    assert_eq!(
        exits.take(ProcessKey {
            pid: 3,
            generation: 2
        }),
        None
    );
    assert_eq!(
        exits.take(ProcessKey {
            pid: 3,
            generation: 1
        }),
        Some(record(3, 1))
    );
    assert_eq!(
        exits.take(ProcessKey {
            pid: 3,
            generation: 1
        }),
        None
    );
}
#[test]
fn full_buffer_retains_all_status_and_requires_child_intake_backpressure() {
    let mut exits = EarlyExits::<2>::new();
    exits.insert(record(3, 1)).unwrap();
    exits.insert(record(4, 1)).unwrap();
    assert!(exits.full());
    assert_eq!(exits.insert(record(5, 1)), Err(Error::Full));
    assert_eq!(exits.take(record(3, 1).process), Some(record(3, 1)));
    assert!(!exits.full());
    exits.insert(record(5, 1)).unwrap();
    exits.clear();
    assert!(!exits.full());
}
#[test]
fn duplicate_status_never_changes_failure_cause_or_restart_time() {
    let mut exits = EarlyExits::<1>::new();
    exits.insert(record(3, 1)).unwrap();
    exits.insert(record(3, 1)).unwrap();
    let mut changed = record(3, 1);
    changed.tick = 99;
    assert_eq!(exits.insert(changed), Err(Error::Conflict));
    assert_eq!(exits.take(record(3, 1).process), Some(record(3, 1)));
    assert_eq!(exits.insert(record(4, 0)), Err(Error::Invalid));
    assert_eq!(exits.insert(record(u32::MAX, 1)), Err(Error::Invalid));
}
