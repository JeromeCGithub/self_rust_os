use rustos_quickjs::{native_allocations, native_platform_check, Error, Limits, Runtime};
use userlib::{println, sys, thread};

fn main(_: usize, argv: &[&str]) {
    if argv.contains(&"--child") {
        // The runtime is created and destroyed inside the worker: no foreign
        // pointer or JS value crosses threads. Exercise the existing 256 KiB cap.
        thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(exercise_engine)
            .expect("native engine worker")
            .join()
            .expect("engine worker join");
        return;
    }
    let child =
        sys::spawn_with_args("/BIN/ZJSPROBE.BIN", &["--child"]).expect("ordinary probe child");
    assert!(child > 0);
    assert_eq!(
        sys::waitpid(child),
        Ok(0),
        "ordinary child completed assertions"
    );
    println!("[qjsprobe] PASS");
}

fn exercise_engine() {
    let started = sys::ticks();
    assert_eq!(native_allocations(), Some(0));
    assert_eq!(native_platform_check(), Some(true));
    assert_eq!(native_allocations(), Some(0));
    {
        let mut runtime = Runtime::new(Limits::default()).expect("upstream engine in Ring 3");
        assert_eq!(runtime.eval("class Counter { #n=40; next(){return ++this.#n} }; const c=new Counter(); JSON.stringify([c.next(),c.next(),[1,2].map(x=>x*2),/rust/i.test('Rust'),new Uint8Array([7,8])[1],'café'.normalize('NFC')])").unwrap(), "[41,42,[2,4],true,8,\"café\"]");
        assert_eq!(
            runtime
                .eval("(()=>{const x=6;return y=>x*y})()(7)")
                .unwrap(),
            "42"
        );
        println!("[qjsprobe] case language PASS");
        assert_eq!(runtime.eval("JSON.stringify([Math.sqrt(81),Math.sin(0),Math.floor(1.9),Math.pow(2,5),Number.isFinite(Math.random())])").unwrap(), "[9,0,1,32,true]");
        println!("[qjsprobe] case math PASS");
        assert_eq!(
            runtime.eval("throw new Error('fixture')"),
            Err(Error::Exception)
        );
        assert_eq!(runtime.eval("6*7").unwrap(), "42");
        println!("[qjsprobe] case exceptions PASS");
        assert_eq!(runtime.eval("for(;;) {}"), Err(Error::Interrupted));
        let polls = runtime.stats().interrupt_polls;
        assert_eq!(
            runtime.eval("({toString(){for(;;){}}})"),
            Err(Error::Interrupted)
        );
        assert!((1..=100).contains(&polls));
        assert_eq!(runtime.eval("21*2").unwrap(), "42");
        println!("[qjsprobe] case interrupt PASS polls={}", polls);
        assert_eq!(
            runtime.eval("function f(){return f()} f()"),
            Err(Error::Exception)
        );
        assert_eq!(runtime.eval("40+2").unwrap(), "42");
        println!("[qjsprobe] case stack PASS");
        assert_eq!(runtime.eval("'x'.repeat(9000)"), Err(Error::OutputLimit));
        assert_eq!(runtime.eval("1\0+2"), Err(Error::InvalidInput));
        println!("[qjsprobe] case output PASS");
        assert_eq!(runtime.eval("[typeof std,typeof os,typeof require,typeof fetch,typeof document,typeof Date,typeof Atomics].join(',')").unwrap(), "undefined,undefined,undefined,undefined,undefined,undefined,undefined");
        println!("[qjsprobe] case capabilities PASS");
        let stats = runtime.stats();
        assert!(stats.allocated_bytes > 0 && stats.peak_bytes >= stats.allocated_bytes);
        assert!(stats.peak_bytes <= Limits::default().memory_bytes);
        println!(
            "[qjsprobe] engine_peak_bytes={} live_allocations={}",
            stats.peak_bytes, stats.allocations
        );
    }
    assert!(matches!(
        Runtime::new(Limits {
            memory_bytes: 1024,
            ..Limits::default()
        }),
        Err(Error::Memory)
    ));
    {
        let mut runtime = Runtime::new(Limits {
            memory_bytes: 1024 * 1024,
            ..Limits::default()
        })
        .unwrap();
        assert_eq!(runtime.eval("'x'.repeat(10*1024*1024)"), Err(Error::Memory));
        assert_eq!(runtime.eval("6*7").unwrap(), "42");
        println!(
            "[qjsprobe] case quota PASS peak_bytes={}",
            runtime.stats().peak_bytes
        );
    }
    for _ in 0..20 {
        {
            let mut runtime = Runtime::new(Limits::default()).unwrap();
            assert_eq!(runtime.eval("typeof fixtureSecret").unwrap(), "undefined");
            runtime.eval("globalThis.fixtureSecret=7").unwrap();
        }
        assert_eq!(
            native_allocations(),
            Some(0),
            "native blocks reclaimed after every realm"
        );
    }
    println!("[qjsprobe] case cleanup PASS cycles=20");
    println!(
        "[qjsprobe] native_blocks=0 elapsed_ticks={}",
        sys::ticks() - started
    );
}

// Math.random is non-cryptographic. This profile exposes neither Date nor a
// high-resolution wall clock to scripts; UTC is used only for runtime seeding.
#[no_mangle]
extern "C" fn rustos_qjs_epoch_micros() -> u64 {
    userlib::system::unix_time()
        .expect("UTC seed")
        .checked_mul(1_000_000)
        .expect("UTC range")
}
#[no_mangle]
extern "C" fn rustos_qjs_abort() -> ! {
    println!("[qjsprobe] FAIL native invariant");
    sys::exit(1)
}
