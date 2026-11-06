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

#[test]
fn attributes_update_identity_styles_links_and_successful_controls() {
    let (doc, failures) = render(
        r#"<style>.ready {color:#124578}</style><p id=x style='display:none'>Visible result</p>
    <a id=link href='/old'>Destination</a><form><input id=query name=q value=old disabled></form>
    <script>
    const p=document.getElementById('x');
    if(p.getAttribute('missing')!==null || p.hasAttribute('class'))throw Error('absent');
    p.setAttribute('CLASS','ready');p.removeAttribute('STYLE');p.id='result';
    if(document.getElementById('x')!==null || document.getElementById('result')!==p || p.className!=='ready')throw Error('identity');
    document.getElementById('link').setAttribute('href','/actual');
    const q=document.getElementById('query');q.setAttribute('value','new value');q.removeAttribute('disabled');
    document.title='Attribute page ready';
    </script>"#,
    );
    assert_eq!(failures, 0);
    assert_eq!(doc.title, "Attribute page ready");
    let page = doc.layout(640);
    assert!(page
        .items
        .iter()
        .any(|i| i.text == "Visible result" && i.color == 0x124578));
    assert!(page
        .items
        .iter()
        .any(|i| i.link.as_deref() == Some("/actual")));
    assert_eq!(
        doc.forms[0].submit("https://example.test/", None).unwrap(),
        "https://example.test/?q=new+value"
    );
}

#[test]
fn browser_base64_uses_binary_strings_and_forgiving_ascii_whitespace() {
    let (doc, failures) = render(
        r#"<script>
    let binary='';for(let i=0;i<256;i++)binary+=String.fromCharCode(i);
    if(atob(btoa(binary))!==binary || btoa('foo')!=='Zm9v' || atob(' Zm\t9v\n')!=='foo' || atob('Zg')!=='f' || atob('Zg==')!=='f')throw Error('round trip');
    for(const value of ['a','ab=c','Zg===','☃','a-b_']) {
      let rejected=false;try {atob(value)}catch(e){rejected=e.name==='InvalidCharacterError'}
      if(!rejected)throw Error('invalid base64');
    }
    let rejected=false;try{btoa('Ω')}catch(e){rejected=e.name==='InvalidCharacterError'}
    if(!rejected)throw Error('non-Latin1');
    document.title=atob('QmFzZTY0IHJlYWR5');
    </script>"#,
    );
    assert_eq!(failures, 0);
    assert_eq!(doc.title, "Base64 ready");
}

#[test]
fn attribute_failures_leave_previous_value_and_realm_recovery_intact() {
    let (doc, failures) = render(
        r#"<p id=x title=original>Safe</p><script>
    const p=document.getElementById('x');
    for(const [name,value] of [['bad name','x'],['title','x'.repeat(8193)]]) {
      let rejected=false;try{p.setAttribute(name,value)}catch(e){rejected=true}
      if(!rejected || p.getAttribute('title')!=='original')throw Error('atomicity');
    }
    p.removeAttribute('absent');document.title='Recovered';
    </script>"#,
    );
    assert_eq!(failures, 0);
    assert_eq!(doc.title, "Recovered");
}
