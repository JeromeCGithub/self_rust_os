use rustos_browser::{
    dom::{Change, Changes},
    Document,
};
#[test]
fn bounded_pointer_free_changes_round_trip_and_reject_malformed_input() {
    let changes = Changes {
        failures: 2,
        changes: vec![
            Change::Title("café Ω".into()),
            Change::Text {
                node: 42,
                value: "<b>text</b>".into(),
            },
            Change::Style {
                node: 1,
                property: "color".into(),
                value: "#123456".into(),
            },
        ],
    };
    let bytes = changes.encode().unwrap();
    assert_eq!(Changes::decode(&bytes).unwrap(), changes);
    for end in 0..bytes.len() {
        assert!(Changes::decode(&bytes[..end]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Changes::decode(&trailing).is_err());
    let mut reserved = bytes.clone();
    reserved[10] = 1;
    assert!(Changes::decode(&reserved).is_err());
    assert!(Changes {
        failures: 0,
        changes: vec![Change::Title("x".repeat(8193))]
    }
    .encode()
    .is_err());
    assert!(Changes {
        failures: 0,
        changes: vec![Change::Title("x".into()); 257]
    }
    .encode()
    .is_err());
}
#[test]
fn text_is_literal_and_invalid_worker_effects_cannot_partially_commit() {
    let mut doc = Document::parse("<title>Old</title><p id=x>Before <b>child</b></p>").unwrap();
    let node = doc
        .nodes
        .iter()
        .position(|n| n.attr("id") == Some("x"))
        .unwrap() as u32;
    let bad = Changes {
        failures: 0,
        changes: vec![
            Change::Title("Wrong".into()),
            Change::Text {
                node: u32::MAX,
                value: "bad".into(),
            },
        ],
    };
    assert!(doc.apply_changes(&bad).is_err());
    assert_eq!(doc.title, "Old");
    let changes = Changes {
        failures: 0,
        changes: vec![
            Change::Text {
                node,
                value: "<b>literal</b>".into(),
            },
            Change::Style {
                node,
                property: "color".into(),
                value: "#123456".into(),
            },
        ],
    };
    doc.apply_changes(&changes).unwrap();
    let page = doc.layout(640);
    assert!(page
        .items
        .iter()
        .any(|i| i.text == "<b>literal</b>" && i.color == 0x123456));
    assert!(!page.items.iter().any(|i| i.text.contains("child")));
    let injected = Changes {
        failures: 0,
        changes: vec![Change::Style {
            node,
            property: "color".into(),
            value: "red;display:none".into(),
        }],
    };
    assert!(doc.apply_changes(&injected).is_err());
    assert!(!doc.layout(640).is_empty());
}
