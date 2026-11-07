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

#[test]
fn page_profile_compiles_nested_generated_expressions_and_recovers_at_its_boundary() {
    let expression = format!("{}42{}", "Math.max(".repeat(48), ",0)".repeat(48));
    let (doc, failures) = render(&format!(
        "<script>document.title=String({expression})</script>"
    ));
    assert_eq!(
        failures, 0,
        "bounded ordinary generated expressions must compile"
    );
    assert_eq!(doc.title, "42");
    let excessive = format!("{}42{}", "Math.max(".repeat(256), ",0)".repeat(256));
    let (doc, failures) = render(&format!(
        "<p>Readable</p><script>{excessive}</script><script>document.title='Recovered'</script>"
    ));
    assert_eq!(failures, 1);
    assert_eq!(doc.title, "Recovered");
}

#[test]
fn page_dates_support_utc_expiry_arithmetic_without_other_ambient_capabilities() {
    let (doc, failures) = render(
        r#"<script>
      const expiry=new Date('2026-11-06T12:00:00Z');
      expiry.setUTCSeconds(expiry.getUTCSeconds()+300);
      if(!Number.isFinite(Date.now()) || typeof os!=='undefined' || typeof Atomics!=='undefined')throw Error('profile');
      document.title=expiry.toUTCString()+'|'+new Date(NaN).toString();
    </script>"#,
    );
    assert_eq!(failures, 0);
    assert_eq!(doc.title, "Fri, 06 Nov 2026 12:05:00 GMT|Invalid Date");
}

#[test]
fn page_context_cookie_round_trip_and_promise_navigation_stop_old_document_scripts() {
    use rustos_browser::page_context::PageRequest;
    let html = r#"<script>
      if(navigator.userAgent!=='RustOS-curl/1' || !navigator.cookieEnabled || navigator.platform!=='RustOS')throw Error('identity');
      if(location.href!=='https://example.test/search?q=rust#top' || document.URL!==location.href || location.search!=='?q=rust' || location.pathname!=='/search' || location.hostname!=='example.test' || location.protocol!=='https:' || location.hash!=='#top')throw Error('location');
      if(document.cookie!=='proof=old')throw Error('snapshot');
      document.cookie='proof=abc; Path=/; Secure; Max-Age=300';
      document.cookie='scoped=no; Path=/other';
      document.cookie='evil=no; Domain=test';
      if(document.cookie!=='proof=abc')throw Error('read your writes');
      const first=performance.now();if(first<0 || performance.now()<first)throw Error('monotonic');
      Promise.resolve().then(()=>location.replace('/search?q=rust&verified=1'));
      document.title='Page context ready';
    </script><script>document.title='must not run after navigation'</script>"#;
    let result = rustos_browser_dom::run_page(
        PageRequest {
            html,
            url: "https://example.test/search?q=rust#top",
            cookie_snapshot: "proof=old; Path=/; Secure",
            epoch_seconds: 100,
        },
        || 500,
    )
    .unwrap();
    assert_eq!(result.dom.failures, 0);
    assert_eq!(
        result.cookies,
        vec![
            "proof=abc; Path=/; Secure; Max-Age=300",
            "scoped=no; Path=/other"
        ]
    );
    assert_eq!(
        result.navigation.as_deref(),
        Some("https://example.test/search?q=rust&verified=1")
    );
    let mut doc = Document::parse(html).unwrap();
    doc.apply_changes(&result.dom).unwrap();
    assert_eq!(doc.title, "Page context ready");
}

#[test]
fn page_rejects_unsafe_navigation_and_recovers_for_a_valid_later_script() {
    use rustos_browser::page_context::PageRequest;
    let html = r#"<script>location.href='javascript:alert(1)'</script><script>location.assign('http://example.test/')</script><script>document.title='Recovered';location.href='/next'</script>"#;
    let result = rustos_browser_dom::run_page(
        PageRequest {
            html,
            url: "https://example.test/",
            cookie_snapshot: "",
            epoch_seconds: 0,
        },
        || 0,
    )
    .unwrap();
    assert_eq!(result.dom.failures, 2);
    assert_eq!(
        result.navigation.as_deref(),
        Some("https://example.test/next")
    );
    assert!(rustos_browser_dom::run_page(
        PageRequest {
            url: "file:///etc/passwd",
            html,
            cookie_snapshot: "",
            epoch_seconds: 0
        },
        || 0
    )
    .is_err());
}

#[test]
fn performance_clock_progresses_and_never_moves_backwards_with_a_coarse_clock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALL: AtomicUsize = AtomicUsize::new(0);
    fn clock() -> u64 {
        [100, 125, 120, 140][CALL.fetch_add(1, Ordering::Relaxed).min(3)]
    }
    CALL.store(0, Ordering::Relaxed);
    let result=rustos_browser_dom::run_page(rustos_browser::page_context::PageRequest{html:"<script>document.title=[performance.now(),performance.now(),performance.now(),performance.timeOrigin].join(',')</script>",url:"https://a.test/",cookie_snapshot:"",epoch_seconds:100},clock).unwrap();
    assert_eq!(result.dom.failures, 0);
    assert_eq!(
        result.dom.changes,
        vec![rustos_browser::dom::Change::Title("25,25,40,100000".into())]
    );
}
