//! Real Ring 3 verified HTTPS, platform-boundary and allocation cleanup probe.
use userlib::{http, net, println, sys, system, tls};
pub fn run(config: [u8; 12]) -> ! {
    let port = |index: usize| u16::from_le_bytes([config[index * 2], config[index * 2 + 1]]);
    let mut mode = [0];
    let _ = sys::read_file("/TMP/TLS.MOD", &mut mode);
    if mode[0] == b'O' {
        let _socket = net::open(net::Kind::Udp).unwrap();
        let oversized = alloc::vec![0x55u8;32*1024*1024];
        core::hint::black_box(&oversized);
        sys::exit(1);
    }
    let baseline = net::info().unwrap().open_sockets;
    let options = http::Options {
        dns_server: [10, 0, 2, 2],
        dns_port: port(4),
        tls_ca_file: Some("/ETC/TEST.CA"),
        timeout_ticks: 600,
        ..http::Options::default()
    };
    let mut url = Url {
        bytes: [0; 128],
        len: 0,
    };
    use core::fmt::Write;
    write!(&mut url, "https://10.0.2.2:{}/", port(0)).unwrap();
    if mode[0] == b'R' {
        assert_eq!(
            system::random_bytes(&mut [0; 32]),
            Err(system::Error::Unavailable)
        );
        assert_eq!(
            http::get_with(url.text(), options, |_| Ok(())),
            Err(http::Error::Entropy)
        );
        assert_eq!(net::info().unwrap().open_sockets, baseline);
        println!("[tlstest] PASS: unavailable entropy refused");
        sys::exit(0);
    }
    if mode[0] == b'C' {
        assert_eq!(
            http::get_with(url.text(), options, |_| Ok(())),
            Err(http::Error::Clock)
        );
        assert_eq!(net::info().unwrap().open_sockets, baseline);
        println!("[tlstest] PASS: invalid UTC refused");
        sys::exit(0);
    }
    let mut random = [0; 64];
    system::random_bytes(&mut random).unwrap();
    assert!(random.iter().any(|b| *b != 0));
    let previous = random;
    system::random_bytes(&mut random).unwrap();
    assert_ne!(previous, random);
    let now = system::unix_time().unwrap();
    assert!(now > 1700000000);
    println!("[tlstest] UTC {}", now);
    for (pointer, length) in [
        (0, 32),
        (u64::MAX - 4, 16),
        (0x4000000 - 8, 16),
        (0xffff800000000000, 32),
        (0x1ffffff, 2),   // mapped final stack byte crosses its upper guard
        (0x1bfffff, 2),   // lower stack guard crosses into mapped stack
        (0x1c00000, 257), // request exceeds bounded syscall output
    ] {
        // SAFETY: Deliberately malformed output ranges exercise validation; they must
        // be rejected by the kernel before any memory access or entropy operation.
        let result = unsafe { sys::syscall(27, pointer, length, 0) };
        assert_eq!(result, system::Error::Invalid.encode());
    }
    for (number, first, second, third) in
        [(27, 0, 0, 1), (28, 1, 0, 0), (28, 0, 1, 0), (28, 0, 0, 1)]
    {
        // SAFETY: Invalid scalar/reserved arguments must fail before memory access.
        assert_eq!(
            unsafe { sys::syscall(number, first, second, third) },
            system::Error::Invalid.encode()
        );
    }
    // SAFETY: Zero-length entropy requests accept a null pointer without access.
    assert_eq!(unsafe { sys::syscall(27, 0, 0, 0) }, 0);
    let heap = tls::heap_used();
    for _ in 0..2 {
        let mut allocation = alloc::vec::Vec::<u8>::new();
        allocation
            .try_reserve_exact(8 * 1024 * 1024)
            .expect("expanded userspace heap must allocate 8 MiB");
        allocation.resize(8 * 1024 * 1024, 0x5a);
        assert!(allocation.iter().all(|byte| *byte == 0x5a));
        drop(allocation);
        assert_eq!(
            tls::heap_used(),
            heap,
            "large allocations must be reclaimed"
        );
    }
    for _ in 0..2 {
        for (server, host, path, trusted, expected) in [
            (0, "fixture.test", "/", true, None),
            (3, "fixture.test", "/", true, None),
            (0, "fixture.test", "/large", true, None),
            (0, "fixture.test", "/chunked", true, None),
            (0, "fixture.test", "/headers", true, None),
            (0, "fixture.test", "/long-redirect", true, None),
            (
                0,
                "fixture.test",
                "/overline",
                true,
                Some(http::Error::TooLarge),
            ),
            (
                0,
                "fixture.test",
                "/overheaders",
                true,
                Some(http::Error::TooLarge),
            ),
            (
                0,
                "fixture.test",
                "/overbody",
                true,
                Some(http::Error::TooLarge),
            ),
            (0, "wrong.test", "/", true, Some(http::Error::Certificate)),
            (1, "fixture.test", "/", true, Some(http::Error::Certificate)),
            (2, "fixture.test", "/", true, Some(http::Error::Certificate)),
            (
                0,
                "fixture.test",
                "/",
                false,
                Some(http::Error::Certificate),
            ),
            (
                0,
                "fixture.test",
                "/downgrade",
                true,
                Some(http::Error::InsecureRedirect),
            ),
        ] {
            let mut url = Url {
                bytes: [0; 128],
                len: 0,
            };
            write!(&mut url, "https://{}:{}{}", host, port(server), path).unwrap();
            let mut output = [0; 32];
            let mut length = 0;
            let mut sum = 0u64;
            let result = http::get_with(
                url.text(),
                http::Options {
                    tls_ca_file: if trusted { options.tls_ca_file } else { None },
                    ..options
                },
                |bytes| {
                    if path == "/large" {
                        sum += bytes.iter().map(|b| u64::from(*b)).sum::<u64>();
                    } else {
                        assert!(length + bytes.len() <= output.len());
                        output[length..length + bytes.len()].copy_from_slice(bytes);
                    }
                    length += bytes.len();
                    Ok(())
                },
            );
            if let Some(error) = expected {
                assert_eq!(result, Err(error));
                assert_eq!(length, 0);
            } else {
                assert_eq!(result.unwrap().bytes, length);
                if path == "/large" {
                    assert_eq!(length, 65536);
                    assert_eq!(sum, 65536 * 120);
                } else {
                    assert_eq!(&output[..length], b"tls-fixture-ok");
                }
            }
            assert_eq!(net::info().unwrap().open_sockets, baseline);
            assert_eq!(tls::heap_used(), heap, "TLS allocations must be reclaimed");
        }
    }
    let mut url = Url {
        bytes: [0; 128],
        len: 0,
    };
    write!(&mut url, "https://fixture.test:{}/expanded", port(0)).unwrap();
    let mut count = 0usize;
    let mut sum = 0u64;
    let response = http::get_with(
        url.text(),
        http::Options {
            timeout_ticks: 1200,
            ..options
        },
        |bytes| {
            count += bytes.len();
            sum += bytes.iter().map(|byte| u64::from(*byte)).sum::<u64>();
            Ok(())
        },
    )
    .expect("response beyond the old 16 MiB limit must stream");
    assert_eq!(response.bytes, 17 * 1024 * 1024);
    assert_eq!(count, response.bytes);
    assert_eq!(sum, count as u64 * 120);
    assert_eq!(net::info().unwrap().open_sockets, baseline);
    assert_eq!(tls::heap_used(), heap);
    println!("[tlstest] expanded body: {} bytes", count);
    assert_eq!(sys::write_file("/TMP/TLS.MOD", b"O"), Some(1));
    let child = sys::spawn("/BIN/ZNETTEST.BIN").unwrap();
    assert_eq!(sys::waitpid(child), Ok(12));
    assert_eq!(sys::write_file("/TMP/TLS.MOD", b"V"), Some(1));
    assert_eq!(net::info().unwrap().open_sockets, baseline);
    assert_eq!(tls::heap_used(), heap);
    println!("[tlstest] PASS: verified HTTPS RSA hostname dates trust redirects streaming cleanup");
    sys::exit(0)
}
struct Url {
    bytes: [u8; 128],
    len: usize,
}
impl Url {
    fn text(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap()
    }
}
impl core::fmt::Write for Url {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.bytes.len() {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
