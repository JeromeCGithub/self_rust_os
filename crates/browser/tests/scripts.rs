#![cfg(feature = "javascript")]
use rustos_browser::{Document, ScriptError};

#[test]
fn inline_scripts_change_dom_text_title_and_style_before_layout() {
    let mut doc=Document::parse("<p id='result'>old</p><script>let n = 6 * 7; document.getElementById('result').textContent = 'Answer: ' + n; document.getElementById('result').style.color = '#123456'; document.body.style.backgroundColor = '#d4edda'; document.title = 'Scripted';</script>").unwrap();
    assert!(
        doc.layout(640).items.iter().any(|i| i.text == "old"),
        "parse alone is inert"
    );
    let report = doc.run_scripts();
    assert_eq!(report.executed, 1);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(doc.title, "Scripted");
    let page = doc.layout(640);
    assert_eq!(page.background, 0xd4edda);
    let item = page.items.iter().find(|i| i.text == "Answer: 42").unwrap();
    assert_eq!(item.color, 0x123456);
    assert!(!page.items.iter().any(|i| i.text.contains("old")));
}
#[test]
fn variables_persist_between_classic_scripts_but_navigation_is_fresh() {
    let mut doc=Document::parse("<script>const base=10;</script><script>let x=(base+2)*3; document.title='Result '+x;</script><script type='application/ld+json'>{bad json}</script>").unwrap();
    let report = doc.run_scripts();
    assert_eq!(report.executed, 2);
    assert!(report.failures.is_empty());
    assert_eq!(doc.title, "Result 36");
    let mut next = Document::parse("<script>document.title = base;</script>").unwrap();
    assert_eq!(next.run_scripts().failures, [ScriptError::UnknownName]);
}
#[test]
fn unsupported_code_missing_elements_and_resource_limits_are_explicit() {
    for (code, error) in [
        ("while(true) {}", ScriptError::Unsupported),
        (
            "document.getElementById('missing').textContent='x';",
            ScriptError::MissingElement,
        ),
        ("const x=1; x=2;", ScriptError::ReadOnly),
        ("fetch('https://example.com');", ScriptError::Unsupported),
    ] {
        let mut doc =
            Document::parse(&format!("<script>{code}</script><p>Still readable</p>")).unwrap();
        assert_eq!(doc.run_scripts().failures, [error], "{code}");
        assert!(doc
            .layout(640)
            .items
            .iter()
            .any(|i| i.text == "Still readable"));
    }
    let mut doc = Document::parse(&format!("<script>{}</script>", " ".repeat(32769))).unwrap();
    assert_eq!(doc.run_scripts().failures, [ScriptError::Budget]);
}
#[test]
fn scripts_cannot_inject_markup_or_declarations_through_text_and_style() {
    let mut doc=Document::parse("<div id='a'><p id='child'>removed</p></div><script>document.getElementById('a').textContent='<script>literal'; document.getElementById('a').style.color='red;display:none';</script>").unwrap();
    assert_eq!(doc.run_scripts().failures, [ScriptError::Type]);
    assert!(doc
        .layout(640)
        .items
        .iter()
        .any(|i| i.text == "<script>literal"));
}
