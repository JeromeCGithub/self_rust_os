#![cfg(feature = "freestanding")]
use rustos_quickjs::{native_allocations, native_platform_check, Limits, Runtime};
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
