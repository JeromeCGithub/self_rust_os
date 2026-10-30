//! Repeatable receive-throughput probe. Host provisions a disposable fixture URL.
use userlib::{http, net, println, sys};
pub fn run() -> ! {
    let mut input = [0; 256];
    let length = sys::read_file("/TMP/PERF.URL", &mut input).expect("benchmark URL");
    let url = core::str::from_utf8(&input[..length]).expect("ASCII URL");
    let baseline = net::info().unwrap().open_sockets;
    for round in 0..2 {
        let start = sys::ticks();
        let mut sum = 0u64;
        let response = http::get_with(
            url,
            http::Options {
                timeout_ticks: 600,
                ..http::Options::default()
            },
            |data| {
                for &byte in data {
                    sum = sum.wrapping_add(u64::from(byte));
                }
                Ok(())
            },
        )
        .expect("benchmark HTTP GET");
        let elapsed = sys::ticks().wrapping_sub(start);
        println!(
            "[netperf] round={} bytes={} ticks={} sum={}",
            round, response.bytes, elapsed, sum
        );
        assert_eq!(response.status, 200);
        assert_eq!(response.bytes, 131072);
        assert_eq!(sum, 131072 * 120);
        assert_eq!(net::info().unwrap().open_sockets, baseline);
    }
    println!("[netperf] PASS");
    sys::exit(0)
}
