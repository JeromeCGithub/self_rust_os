use rustos_browser::{
    dom::{Change, Changes},
    Document,
};
#[test]
fn linked_heading_evidence_requires_rendered_navigation_and_nonempty_text() {
    let doc=Document::parse("<h3>Unlinked</h3><a href='/one'><h3>First title</h3></a><h2><a href='/two'>Second title</a></h2><a href='/empty'><h3></h3></a><div style='display:none'><a href='/hidden'><h3>Hidden title</h3></a></div>").unwrap();
    assert_eq!(doc.linked_heading_count(&doc.layout(640)), 2);
}
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
            Change::Attribute {
                node: 2,
                name: "href".into(),
                value: Some("/result".into()),
            },
            Change::Attribute {
                node: 2,
                name: "style".into(),
                value: None,
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
fn invalid_attributes_and_form_limits_rollback_the_whole_batch() {
    let mut doc =
        Document::parse("<title>Before</title><form id=f><input id=x name=q value=old></form>")
            .unwrap();
    let node = doc
        .nodes
        .iter()
        .position(|n| n.attr("id") == Some("x"))
        .unwrap() as u32;
    for (name, value) in [
        ("bad name", Some("x".into())),
        ("value", Some("x".repeat(2049))),
        ("title", Some("x".repeat(8193))),
    ] {
        let changes = Changes {
            failures: 0,
            changes: vec![
                Change::Title("Wrong".into()),
                Change::Attribute {
                    node,
                    name: name.into(),
                    value,
                },
            ],
        };
        assert!(doc.apply_changes(&changes).is_err());
        assert_eq!(doc.title, "Before");
        assert_eq!(doc.forms[0].fields[0].value, "old");
    }
    for i in 0..61 {
        doc.apply_change(&Change::Attribute {
            node,
            name: format!("data-{i}"),
            value: Some("x".into()),
        })
        .unwrap();
    }
    assert!(doc
        .apply_change(&Change::Attribute {
            node,
            name: "extra".into(),
            value: Some("x".into())
        })
        .is_err());
    doc.apply_change(&Change::Attribute {
        node,
        name: "value".into(),
        value: Some("updated".into()),
    })
    .unwrap();
    assert_eq!(doc.forms[0].fields[0].value, "updated");
    doc.apply_change(&Change::Attribute {
        node,
        name: "data-0".into(),
        value: None,
    })
    .unwrap();
    doc.apply_change(&Change::Attribute {
        node,
        name: "extra".into(),
        value: Some("x".into()),
    })
    .unwrap();
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
