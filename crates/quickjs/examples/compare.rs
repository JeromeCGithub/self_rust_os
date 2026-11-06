//! Fixed public fixtures only; no page/source/exception text enters the report.
use rustos_quickjs::{Limits, Runtime};
struct NoDom;
impl rustos_script::Host for NoDom {
    fn read(
        &mut self,
        _: &rustos_script::Target,
    ) -> Result<rustos_script::Value, rustos_script::Error> {
        Err(rustos_script::Error::MissingElement)
    }
    fn write(
        &mut self,
        _: &rustos_script::Target,
        _: rustos_script::Value,
    ) -> Result<(), rustos_script::Error> {
        Err(rustos_script::Error::MissingElement)
    }
}
fn main() {
    for (name, source) in [
        ("language", include_str!("../fixtures/language.js")),
        ("dom", include_str!("../fixtures/dom.js")),
        ("promises", include_str!("../fixtures/promises.js")),
        ("loop", include_str!("../fixtures/loop.js")),
        ("quota", include_str!("../fixtures/quota.js")),
    ] {
        let prototype = match rustos_script::Runtime::new().execute(source, &mut NoDom) {
            Ok(()) => "accepted".to_string(),
            Err(e) => format!("{e:?}"),
        };
        let start = std::time::Instant::now();
        let mut engine = Runtime::new(Limits::default()).expect("restricted upstream runtime");
        let outcome = match engine.eval(source) {
            Ok(value) if value == "[object Promise]" => "pending_jobs_unserviced".to_string(),
            Ok(_) => "evaluated".to_string(),
            Err(e) => format!("{e:?}"),
        };
        let stats = engine.stats();
        println!("{{\"fixture\":\"{name}\",\"source_bytes\":{},\"prototype\":\"{prototype}\",\"quickjs\":\"{outcome}\",\"engine_peak_bytes\":{},\"engine_live_allocations\":{},\"interrupt_polls\":{},\"host_elapsed_us\":{}}}",source.len(),stats.peak_bytes,stats.allocations,stats.interrupt_polls,start.elapsed().as_micros());
    }
}
#[cfg(feature = "freestanding")]
#[no_mangle]
extern "C" fn rustos_qjs_epoch_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("host clock")
        .as_micros()
        .try_into()
        .expect("clock range")
}
#[cfg(feature = "freestanding")]
#[no_mangle]
extern "C" fn rustos_qjs_abort() -> ! {
    std::process::abort()
}
