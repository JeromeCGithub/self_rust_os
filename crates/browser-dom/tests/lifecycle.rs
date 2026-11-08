#![cfg(feature = "freestanding")]
use rustos_browser::page_context::PageRequest;

#[no_mangle]
extern "C" fn rustos_qjs_epoch_micros() -> u64 {
    100_000_000
}
#[no_mangle]
extern "C" fn rustos_qjs_abort() -> ! {
    std::process::abort()
}

#[test]
fn native_page_contexts_release_engine_allocations_after_navigation_and_job_failure() {
    // This integration executable has one test, so the adapter's process-wide
    // allocation counter cannot race with other realms from the same harness.
    let baseline = rustos_quickjs::native_allocations();
    assert!(baseline.is_some());
    for _ in 0..20 {
        for (html, failures, navigation) in [
            (
                "<script>document.cookie='proof=42; Path=/';const expiry=new Date();Promise.resolve().then(()=>location.replace('/next'));</script>",
                0,
                Some("https://example.test/next"),
            ),
            (
                "<p>Readable</p><script>document.cookie='proof=42; Path=/';function again(){Promise.resolve().then(again)}again()</script>",
                1,
                None,
            ),
        ] {
            let reply = rustos_browser_dom::run_page(PageRequest {
                html, url: "https://example.test/", cookie_snapshot: "", epoch_seconds: 100,
            }, || 0).unwrap();
            assert_eq!(reply.dom.failures, failures);
            assert_eq!(reply.navigation.as_deref(), navigation);
            assert_eq!(reply.cookies.len(), 1);
            drop(reply);
            assert_eq!(rustos_quickjs::native_allocations(), baseline);
        }
    }
}
