use rustos_service_core::{journal::*, ProcessKey};
fn key(generation: u32) -> ProcessKey {
    ProcessKey {
        pid: 10,
        generation,
    }
}
#[test]
fn records_preserve_typed_attribution_unicode_and_chronological_cursor() {
    let mut journal = Journal::<4>::new();
    let first = journal
        .push(2, 7, key(1), 10, Level::Info, "hello café")
        .unwrap();
    let second = journal
        .push(2, 8, key(2), 11, Level::Error, "new process")
        .unwrap();
    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(journal.len(), 2);
    let record = journal.after(0).unwrap().unwrap();
    assert_eq!(record.sequence, first);
    assert_eq!(record.text(), "hello café");
    assert_eq!(record.unit, 2);
    assert_eq!(record.run, 7);
    assert_eq!(record.process, key(1));
    assert_eq!(record.tick, 10);
    let record = journal.after(first).unwrap().unwrap();
    assert_eq!(record.process, key(2));
    assert_eq!(record.level, Level::Error);
    assert_eq!(journal.after(second), Ok(None));
}
#[test]
fn bounded_overwrite_reports_cursor_gaps_and_reuses_storage() {
    let mut journal = Journal::<3>::new();
    for n in 0..100 {
        journal
            .push(1, 1, key(1), n, Level::Debug, "record")
            .unwrap();
    }
    assert_eq!(journal.len(), 3);
    assert_eq!(journal.evicted(), 97);
    assert_eq!(journal.after(1), Err(Error::Gap { oldest: 98 }));
    assert_eq!(journal.after(0).unwrap().unwrap().sequence, 98);
    assert_eq!(journal.after(98).unwrap().unwrap().sequence, 99);
    assert_eq!(journal.after(100), Ok(None));
}
#[test]
fn bad_records_do_not_advance_sequences_or_replace_valid_data() {
    let mut journal = Journal::<2>::new();
    journal.push(1, 1, key(1), 0, Level::Info, "valid").unwrap();
    for text in ["bad\nline".to_owned(), "x".repeat(MAX_TEXT + 1)] {
        assert!(journal.push(1, 1, key(1), 0, Level::Info, &text).is_err());
    }
    assert!(journal
        .push(64, 1, key(1), 0, Level::Info, "bad unit")
        .is_err());
    assert!(journal
        .push(1, 0, key(1), 0, Level::Info, "zero run")
        .is_err());
    assert!(journal
        .push(1, 1, key(0), 0, Level::Info, "zero generation")
        .is_err());
    assert_eq!(journal.len(), 1);
    assert_eq!(journal.after(0).unwrap().unwrap().text(), "valid");
    assert_eq!(journal.push(1, 1, key(1), 1, Level::Info, "next"), Ok(2));
    assert!(Journal::<0>::new()
        .push(1, 1, key(1), 0, Level::Info, "no capacity")
        .is_err());
}

#[test]
fn source_budget_pauses_until_a_deadline_and_refills_at_the_exact_boundary() {
    let mut budget = Budget::new(2, 5).unwrap();
    assert_eq!(budget.admit(10), Intake::Accept);
    assert_eq!(budget.admit(10), Intake::Accept);
    assert_eq!(budget.admit(11), Intake::Pause { until: Some(15) });
    assert_eq!(budget.admit(14), Intake::Pause { until: Some(15) });
    assert_eq!(budget.admit(15), Intake::Accept);
    for (burst, window) in [(0, 1), (33, 1), (1, 0), (1, u64::MAX)] {
        assert!(Budget::new(burst, window).is_err());
    }
}
#[test]
fn rate_budget_does_not_reset_forever_when_the_clock_cannot_form_a_new_deadline() {
    let mut budget = Budget::new(1, 2).unwrap();
    assert_eq!(budget.admit(u64::MAX), Intake::Accept);
    assert_eq!(budget.admit(u64::MAX), Intake::Pause { until: None });
}
