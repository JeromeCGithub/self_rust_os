use rustos_quickjs::{Error, Limits, Runtime};
#[test]
fn real_language_objects_closures_regex_typed_arrays_and_unicode() {
    let mut runtime = Runtime::new(Limits::default()).expect("upstream runtime");
    assert_eq!(runtime.eval("class Counter { #n=40; next(){ return ++this.#n; } }; const c=new Counter(); JSON.stringify([c.next(),c.next(),[1,2,3].map(x=>x*2),/rust/i.test('Rust'),new Uint8Array([7,8])[1],'café'.normalize('NFC')])").unwrap(),"[41,42,[2,4,6],true,8,\"café\"]");
    assert_eq!(
        runtime
            .eval("(()=>{const x=6;return y=>x*y})()(7)")
            .unwrap(),
        "42"
    );
    assert!(runtime.stats().allocated_bytes > 0);
    assert!(runtime.stats().peak_bytes >= runtime.stats().allocated_bytes);
}
#[test]
fn exceptions_and_interruption_leave_a_usable_realm() {
    let mut runtime = Runtime::new(Limits::default()).unwrap();
    assert_eq!(
        runtime.eval("throw new Error('fixture')"),
        Err(Error::Exception)
    );
    assert_eq!(runtime.eval("for(;;) {}"), Err(Error::Interrupted));
    assert!(runtime.stats().interrupt_polls > 0);
    assert_eq!(
        runtime.eval("({toString(){for(;;){}}})"),
        Err(Error::Interrupted)
    );
    assert_eq!(runtime.eval("6*7").unwrap(), "42");
    assert_eq!(
        runtime.eval("function f(){return f()} f()"),
        Err(Error::Exception)
    );
    assert_eq!(runtime.eval("21*2").unwrap(), "42");
}
#[test]
fn budgets_invalid_inputs_and_ambient_capabilities_are_explicit() {
    assert!(matches!(
        Runtime::new(Limits {
            memory_bytes: 0,
            ..Limits::default()
        }),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        Runtime::new(Limits {
            memory_bytes: 1024,
            ..Limits::default()
        }),
        Err(Error::Memory)
    ));
    let mut runtime = Runtime::new(Limits {
        memory_bytes: 1024 * 1024,
        ..Limits::default()
    })
    .unwrap();
    assert_eq!(runtime.eval("'x'.repeat(10*1024*1024)"), Err(Error::Memory));
    assert_eq!(runtime.eval("1\0+2"), Err(Error::InvalidInput));
    assert_eq!(runtime.eval("'x'.repeat(9000)"), Err(Error::OutputLimit));
    assert_eq!(runtime.eval("[typeof std,typeof os,typeof require,typeof fetch,typeof document,typeof Date,typeof Atomics].join(',')").unwrap(),"undefined,undefined,undefined,undefined,undefined,undefined,undefined");
    assert_eq!(runtime.eval("40+2").unwrap(), "42");
}
#[test]
fn repeated_runtime_lifecycle_preserves_independent_globals() {
    for _ in 0..20 {
        let mut first = Runtime::new(Limits::default()).unwrap();
        first.eval("globalThis.fixtureSecret=7").unwrap();
        let mut second = Runtime::new(Limits::default()).unwrap();
        assert_eq!(second.eval("typeof fixtureSecret").unwrap(), "undefined");
    }
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
fn classic_execution_does_not_coerce_unused_results_and_jobs_are_explicit() {
    let mut runtime = Runtime::new(Limits::default()).unwrap();
    runtime
        .execute("globalThis.result=0; ({toString(){throw Error('must not coerce')}})")
        .unwrap();
    runtime
        .execute("Promise.resolve(6).then(x=>{result=x*7})")
        .unwrap();
    assert_eq!(runtime.eval("result").unwrap(), "0");
    assert_eq!(runtime.run_jobs(16), Ok(1));
    assert_eq!(runtime.eval("result").unwrap(), "42");
    assert_eq!(runtime.run_jobs(16), Ok(0));
    assert_eq!(runtime.run_jobs(0), Err(Error::InvalidInput));
}

#[test]
fn promise_checkpoints_bound_both_job_count_and_cpu_work() {
    let mut runtime = Runtime::new(Limits::default()).unwrap();
    runtime
        .execute("function again(){Promise.resolve().then(again)}; again()")
        .unwrap();
    assert_eq!(runtime.run_jobs(8), Err(Error::Interrupted));
    drop(runtime); // Discard the unbounded queue with its realm.
    let mut runtime = Runtime::new(Limits::default()).unwrap();
    runtime
        .execute("Promise.resolve().then(()=>{for(;;){}})")
        .unwrap();
    assert_eq!(runtime.run_jobs(8), Err(Error::Interrupted));
    assert_eq!(runtime.eval("6*7").unwrap(), "42");
}

#[test]
fn native_host_calls_copy_values_and_keep_state_owned_until_engine_drops() {
    struct Host {
        calls: usize,
    }
    impl rustos_quickjs::Host for Host {
        fn call(
            &mut self,
            op: u32,
            node: u32,
            a: &str,
            b: &str,
        ) -> Result<rustos_quickjs::Reply, Error> {
            self.calls += 1;
            Ok(match op {
                0 => rustos_quickjs::Reply::Text(format!("{node}:{a}:{b}")),
                1 => rustos_quickjs::Reply::Node(42),
                2 => rustos_quickjs::Reply::Null,
                3 => rustos_quickjs::Reply::Unit,
                4 => rustos_quickjs::Reply::Text("x".repeat(8193)),
                _ => return Err(Error::InvalidInput),
            })
        }
    }
    let mut runtime = Runtime::new(Limits::default())
        .unwrap()
        .with_host(Host { calls: 0 })
        .unwrap();
    assert_eq!(
        runtime.eval("__rustosHost(0,7,'café','Ω')").unwrap(),
        "7:café:Ω"
    );
    assert_eq!(runtime.eval("JSON.stringify([__rustosHost(1,0,'',''),__rustosHost(2,0,'',''),typeof __rustosHost(3,0,'','')])").unwrap(), "[42,null,\"undefined\"]");
    assert_eq!(
        runtime.eval("__rustosHost(255,0,'','')"),
        Err(Error::Exception)
    );
    assert_eq!(
        runtime.eval("__rustosHost(4,0,'','')"),
        Err(Error::Exception)
    );
    assert_eq!(
        runtime.eval("__rustosHost(0,0,'x'.repeat(8193),'')"),
        Err(Error::Exception)
    );
    assert_eq!(
        runtime.host().calls,
        6,
        "oversized input never reaches the host"
    );
}
