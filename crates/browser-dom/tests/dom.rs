use rustos_browser::Document;
fn render(html: &str) -> (Document, u32) {
    let result = rustos_browser_dom::run(html).unwrap();
    let mut doc = Document::parse(html).unwrap();
    doc.apply_changes(&result).unwrap();
    (doc, result.failures)
}
#[test]
fn real_language_and_promises_mutate_live_dom_before_layout() {
    let html = r#"<title>Before</title><p id=result>Waiting</p><script>
    class Counter { #n=40; next(){return ++this.#n;} }
    const c=new Counter(); const p=document.getElementById('result');
    if(p!==document.getElementById('result'))throw Error('identity');
    p.textContent=[c.next(),c.next()].map(n=>n*2).join(',');
    p.style.color='#123456';document.body.style.backgroundColor='#e8dcff';
    Promise.resolve().then(()=>{document.title='Native promise ready';p.textContent+=' café Ω';});
    ({toString(){throw Error('unused completion');}});
    </script>"#;
    let (doc, failures) = render(html);
    assert_eq!(failures, 0);
    assert_eq!(doc.title, "Native promise ready");
    let page = doc.layout(640);
    assert_eq!(page.background, 0xe8dcff);
    assert!(page
        .items
        .iter()
        .any(|i| i.text == "82,84 café Ω" && i.color == 0x123456));
}
#[test]
fn script_order_detached_nodes_literal_text_and_errors_preserve_completed_effects() {
    let html = r#"<title>Before</title><div id=parent><p id=child>old</p></div>
    <script>const p=document.getElementById('parent');const child=document.getElementById('child');p.textContent='<b>literal</b>';if(document.getElementById('child')!==null)throw Error('detached');child.textContent='invisible';globalThis.shared=6;</script>
    <script>throw Error('expected');</script><script>document.title='Order '+shared*7;</script>
    <script src='/unsupported.js'></script>"#;
    let (doc, failures) = render(html);
    assert_eq!(failures, 2);
    assert_eq!(doc.title, "Order 42");
    let page = doc.layout(640);
    assert!(page.items.iter().any(|i| i.text == "<b>literal</b>"));
    assert!(!page.items.iter().any(|i| i.text.contains("invisible")));
}
#[test]
fn hostile_loops_values_queues_and_missing_apis_are_bounded_without_ambient_io() {
    for script in [
        "for(;;){}",
        "document.body.textContent='x'.repeat(9000)",
        "function again(){Promise.resolve().then(again)};again()",
        "fetch('/secret')",
    ] {
        let (_, failures) = render(&format!("<p>Still readable</p><script>{script}</script>"));
        assert!(failures > 0);
    }
    let (doc,failures)=render("<script>document.title=[typeof os,typeof std,typeof __rustosHost,typeof fetch].join(',')</script>");
    assert_eq!(failures, 0);
    assert_eq!(doc.title, "undefined,undefined,undefined,undefined");
    let (doc, failures) = render("<script>document.title=typeof shared</script>");
    assert_eq!(failures, 0);
    assert_eq!(doc.title, "undefined");
}
#[cfg(feature = "freestanding")]
#[no_mangle]
extern "C" fn rustos_qjs_epoch_micros() -> u64 {
    1_000_000
}
#[cfg(feature = "freestanding")]
#[no_mangle]
extern "C" fn rustos_qjs_abort() -> ! {
    std::process::abort()
}

#[test]
fn effect_budget_is_enforced_before_a_failing_setter_changes_the_document() {
    let (doc, failures) = render(
        "<title>Before</title><script>for(let i=0;i<300;i++)document.title=String(i)</script>",
    );
    assert_eq!(failures, 1);
    assert_eq!(
        doc.title, "255",
        "first 256 effects commit; the rejected setter has no effect"
    );
    let (_, failures) =
        render("<p>Readable</p><script>document.body.style.color='red;display:none'</script>");
    assert_eq!(failures, 1);
}
