extern crate alloc;
use alloc::vec::Vec;
use rustos_browser_worker::{Header, Kind, HEADER};
use rustos_quickjs::{Error, Limits, Runtime};
use userlib::{process, sys};

fn read_exact(bytes: &mut [u8]) {
    let mut used = 0;
    while used < bytes.len() {
        match process::read_packet() {
            Ok(Some((packet, n))) if n <= bytes.len() - used => {
                bytes[used..used + n].copy_from_slice(&packet[..n]);
                used += n;
            }
            Ok(None) => {
                let _ = sys::wait_events(sys::Events::STDIO_READ, 0);
            }
            _ => sys::exit(74),
        }
    }
}
fn write(bytes: &[u8]) {
    if sys::console_write(bytes) != bytes.len() {
        sys::exit(74)
    }
}
fn send(header: Header) {
    match header.bytes() {
        Ok(bytes) => write(&bytes),
        Err(_) => sys::exit(64),
    }
}
fn main() {
    // Trusted bootstrap drops ambient rights before reading a single page byte.
    if process::restrict_worker(16 * 1024 * 1024).is_err() {
        sys::exit(77)
    }
    let mut runtime = match Runtime::new(Limits::default()) {
        Ok(runtime) => runtime,
        Err(_) => sys::exit(12),
    };
    send(Header {
        kind: Kind::Ready,
        generation: 0,
        length: 0,
        status: 0,
    });
    let mut sequence = 0u64;
    loop {
        let mut bytes = [0; HEADER];
        read_exact(&mut bytes);
        let header = match Header::parse(bytes) {
            Ok(header) => header,
            Err(_) => sys::exit(64),
        };
        sequence = match sequence.checked_add(1) {
            Some(value) => value,
            None => sys::exit(64),
        };
        if !matches!(header.kind, Kind::Eval | Kind::Document) || header.generation != sequence {
            sys::exit(64)
        }
        let mut source = Vec::new();
        if source.try_reserve_exact(header.length).is_err() {
            sys::exit(12)
        }
        source.resize(header.length, 0);
        read_exact(&mut source);
        let result = match core::str::from_utf8(&source) {
            Ok(source) if header.kind == Kind::Eval => {
                runtime.eval(source).map(|text| text.into_bytes())
            }
            Ok(html) => rustos_browser_dom::run(html)
                .and_then(|changes| changes.encode().map_err(|_| Error::OutputLimit)),
            Err(_) => Err(Error::InvalidInput),
        };
        match result {
            Ok(result) => {
                send(Header {
                    kind: Kind::Result,
                    generation: sequence,
                    length: result.len(),
                    status: 0,
                });
                write(&result);
            }
            Err(error) => {
                let status = match error {
                    Error::Exception => 1,
                    Error::Interrupted => 2,
                    Error::Memory => 3,
                    Error::OutputLimit => 4,
                    Error::InvalidInput => 5,
                    Error::Unavailable => 6,
                };
                send(Header {
                    kind: Kind::Failure,
                    generation: sequence,
                    length: 0,
                    status,
                });
            }
        }
    }
}
#[no_mangle]
extern "C" fn rustos_qjs_epoch_micros() -> u64 {
    // This is only a non-cryptographic Math.random seed, never certificate time.
    match userlib::system::unix_time() {
        Ok(seconds) => seconds.saturating_mul(1_000_000),
        Err(_) => sys::ticks(),
    }
}
#[no_mangle]
extern "C" fn rustos_qjs_abort() -> ! {
    sys::exit(73)
}
