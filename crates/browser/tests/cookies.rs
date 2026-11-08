use rustos_browser::cookies::CookieJar;

#[test]
fn host_path_secure_expiry_and_script_visibility_are_enforced() {
    let mut jar = CookieJar::default();
    assert!(jar.set_document(
        "https://a.test/search?q=x",
        "proof=abc; Path=/; Secure; Max-Age=300",
        100
    ));
    assert!(jar.set_document("https://a.test/search?q=x", "local=yes; Path=/search", 100));
    assert_eq!(
        jar.header("https://a.test/search?q=x", 101),
        "local=yes; proof=abc"
    );
    assert_eq!(jar.header("https://b.test/search", 101), "");
    assert_eq!(jar.header("https://sub.a.test/search", 101), "");
    assert_eq!(jar.header("http://a.test/searching", 101), "");
    assert_eq!(jar.header("https://a.test/else", 101), "proof=abc");
    assert_eq!(jar.header("https://a.test/else", 400), "");
    for assignment in [
        "evil=x; Domain=test",
        "hidden=x; HttpOnly",
        "bad=x\r\nHost: evil",
        "__Host-bad=x; Secure; Path=/search",
    ] {
        assert!(!jar.set_document("https://a.test/", assignment, 401));
    }
    assert!(!jar.set_document("http://a.test/", "secret=x; Secure", 401));
    assert_eq!(jar.header("https://a.test/", 401), "");
}

#[test]
fn replacement_deletion_default_path_and_resource_failure_are_atomic() {
    let mut jar = CookieJar::default();
    assert!(jar.set_document("https://a.test/dir/page", "key=first", 0));
    assert_eq!(jar.header("https://a.test/dir/other", 0), "key=first");
    assert_eq!(jar.header("https://a.test/", 0), "");
    assert!(jar.set_document("https://a.test/dir/page", "key=second", 0));
    assert_eq!(jar.header("https://a.test/dir/page", 0), "key=second");
    assert!(!jar.set_document(
        "https://a.test/dir/page",
        &format!("key={}", "x".repeat(4096)),
        0
    ));
    assert_eq!(jar.header("https://a.test/dir/page", 0), "key=second");
    assert!(jar.set_document("https://a.test/dir/page", "key=deleted; Max-Age=0", 1));
    assert_eq!(jar.header("https://a.test/dir/page", 1), "");
    assert!(jar.set_document(
        "https://a.test/",
        "key=x; Expires=Fri, 06 Nov 2026 12:05:00 GMT",
        0
    ));
    assert_eq!(jar.header("https://a.test/", 1793966700), "");
    for n in 0..32 {
        assert!(jar.set_document("https://a.test/", &format!("k{n}=v"), 1793966701));
    }
    let before = jar.header("https://a.test/", 1793966701);
    assert!(!jar.set_document("https://a.test/", "overflow=v", 1793966701));
    assert_eq!(jar.header("https://a.test/", 1793966701), before);
}

#[test]
fn insecure_documents_cannot_delete_secure_values_and_snapshots_keep_duplicate_paths() {
    let mut jar = CookieJar::default();
    assert!(jar.set_document("https://a.test/", "proof=secret; Secure; Path=/", 100));
    assert!(!jar.set_document("http://a.test/", "proof=gone; Path=/; Max-Age=0", 101));
    assert_eq!(jar.header("https://a.test/", 101), "proof=secret");
    assert!(jar.set_document(
        "https://a.test/search",
        "proof=scoped; Secure; Path=/search; Max-Age=100",
        100
    ));
    let snapshot = jar.snapshot("https://a.test/search", 101);
    let mut copy = CookieJar::default();
    for entry in snapshot.lines() {
        assert!(copy.set_document("https://a.test/search", entry, 101));
    }
    assert_eq!(
        copy.header("https://a.test/search", 101),
        "proof=scoped; proof=secret"
    );
    assert_eq!(copy.header("https://a.test/search", 200), "proof=secret");
}
