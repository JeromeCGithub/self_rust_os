#![cfg(feature = "freestanding")]
use rustos_quickjs::{native_allocations, native_platform_check, Limits, Runtime};
// These cases observe a process-global allocation counter, so their owned
// fixtures must not overlap. Other runtime tests execute in a separate process.
static LIFECYCLE: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[no_mangle]
extern "C" fn rustos_qjs_epoch_micros() -> u64 {
    1_000_000
}
#[no_mangle]
extern "C" fn rustos_qjs_abort() -> ! {
    std::process::abort()
}
#[test]
fn native_allocator_formatting_and_runtime_cleanup_are_observable() {
    let _guard = LIFECYCLE.lock().expect("lifecycle fixture lock");
    assert_eq!(native_platform_check(), Some(true));
    assert_eq!(native_allocations(), Some(0));
    for _ in 0..20 {
        {
            let mut runtime = Runtime::new(Limits::default()).unwrap();
            assert!(native_allocations().unwrap() > 0);
            assert_eq!(runtime.eval("JSON.stringify([Math.sqrt(144),Math.sin(0),Math.pow(2,10),Math.random()>=0,Math.random()<1])").unwrap(),"[12,0,1024,true,true]");
        }
        assert_eq!(native_allocations(), Some(0));
    }
}

#[test]
fn native_host_and_pending_jobs_release_all_engine_allocations() {
    let _guard = LIFECYCLE.lock().expect("lifecycle fixture lock");
    struct Host;
    impl rustos_quickjs::Host for Host {
        fn call(
            &mut self,
            _: u32,
            _: u32,
            _: &str,
            _: &str,
        ) -> Result<rustos_quickjs::Reply, rustos_quickjs::Error> {
            Ok(rustos_quickjs::Reply::Null)
        }
    }
    assert_eq!(native_allocations(), Some(0));
    for _ in 0..20 {
        {
            let mut runtime = Runtime::new(Limits::default())
                .unwrap()
                .with_host(Host)
                .unwrap();
            runtime
                .execute(
                    "__rustosHost(0,0,'','');Promise.resolve().then(()=>{throw Error('pending')})",
                )
                .unwrap();
        }
        assert_eq!(native_allocations(), Some(0));
    }
}
