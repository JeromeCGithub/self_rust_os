use rustos_browser::{
    dom::Changes,
    page_context::{PageReply, PageRequest},
};
#[test]
fn page_context_and_effects_round_trip_without_ambiguous_or_unbounded_records() {
    let request = PageRequest {
        html: "<p>Page</p>",
        url: "https://example.test/search?q=rust",
        cookie_snapshot: "proof=abc",
        epoch_seconds: 1793966700,
    };
    let encoded = request.encode().unwrap();
    let decoded = PageRequest::decode(&encoded).unwrap();
    assert_eq!(decoded.html, request.html);
    assert_eq!(decoded.url, request.url);
    assert_eq!(decoded.cookie_snapshot, request.cookie_snapshot);
    assert_eq!(decoded.epoch_seconds, request.epoch_seconds);
    for n in 0..encoded.len() {
        assert!(PageRequest::decode(&encoded[..n]).is_err());
    }
    let mut tail = encoded.clone();
    tail.push(0);
    assert!(PageRequest::decode(&tail).is_err());
    let too_large = "x".repeat(128 * 1024);
    assert!(PageRequest {
        html: &too_large,
        ..request
    }
    .encode()
    .is_err());
    assert!(PageRequest {
        cookie_snapshot: "x=y\r\nHost: evil",
        ..request
    }
    .encode()
    .is_err());
    let reply = PageReply {
        dom: Changes::default(),
        cookies: vec!["proof=new; Path=/; Secure".into()],
        navigation: Some("/search?q=rust".into()),
    };
    let bytes = reply.encode().unwrap();
    let parsed = PageReply::decode(&bytes).unwrap();
    assert_eq!(parsed.cookies, reply.cookies);
    assert_eq!(parsed.navigation, reply.navigation);
    for n in 0..bytes.len() {
        assert!(PageReply::decode(&bytes[..n]).is_err());
    }
    let mut oversized = reply;
    oversized.cookies.push("x".repeat(8192));
    assert!(oversized.encode().is_err());
}

#[test]
fn controller_rechecks_untrusted_effects_atomically_and_never_downgrades_tls() {
    use rustos_browser::{cookies::CookieJar, dom::Change, Document};
    let mut doc = Document::parse("<title>Original</title><p>Readable</p>").unwrap();
    let mut jar = CookieJar::default();
    let mut reply = PageReply {
        dom: Changes {
            failures: 0,
            changes: vec![Change::Title("Changed".into())],
        },
        cookies: vec!["proof=valid; Secure; Path=/".into()],
        navigation: Some("/results".into()),
    };
    assert_eq!(
        reply
            .commit(&mut doc, &mut jar, "https://a.test/search", 100)
            .unwrap()
            .as_deref(),
        Some("https://a.test/results")
    );
    assert_eq!(doc.title, "Changed");
    assert_eq!(jar.header("https://a.test/", 100), "proof=valid");
    for navigation in ["javascript:alert(1)", "file:///secret", "http://a.test/"] {
        reply.dom.changes = vec![Change::Title("Uncommitted".into())];
        reply.cookies = vec!["proof=uncommitted; Path=/; Secure".into()];
        reply.navigation = Some(navigation.into());
        assert!(reply
            .commit(&mut doc, &mut jar, "https://a.test/", 100)
            .is_err());
        assert_eq!(doc.title, "Changed");
        assert_eq!(jar.header("https://a.test/", 100), "proof=valid");
    }
    reply.navigation = None;
    reply.cookies = vec!["proof=uncommitted; Domain=a.test".into()];
    assert!(reply
        .commit(&mut doc, &mut jar, "https://a.test/", 100)
        .is_err());
    assert_eq!(doc.title, "Changed");
    assert_eq!(jar.header("https://a.test/", 100), "proof=valid");
}

#[test]
fn complete_envelope_limits_include_metadata_and_round_trip_exact_boundaries() {
    let html = "x".repeat(128 * 1024 - 24 - "https://a.test/".len());
    let request = PageRequest {
        html: &html,
        url: "https://a.test/",
        cookie_snapshot: "",
        epoch_seconds: 0,
    };
    let bytes = request.encode().unwrap();
    assert_eq!(bytes.len(), 128 * 1024);
    assert_eq!(PageRequest::decode(&bytes).unwrap().html.len(), html.len());
    use rustos_browser::dom::Change;
    let mut reply = PageReply {
        dom: Changes {
            failures: 0,
            changes: vec![Change::Title("x".repeat(8192 - 37))],
        },
        cookies: vec![],
        navigation: None,
    };
    let bytes = reply.encode().unwrap();
    assert_eq!(bytes.len(), 8192);
    assert!(PageReply::decode(&bytes).is_ok());
    reply.dom.changes = vec![Change::Title("x".repeat(8192 - 36))];
    assert!(reply.encode().is_err());
}
