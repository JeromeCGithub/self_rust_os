//! HTTP SDK integration probe; independent host fixture configured by E2E.
use userlib::{http, net, println, sys};
pub fn run(config: [u8; 4]) {
    let port = u16::from_le_bytes([config[0], config[1]]);
    let dns_port = u16::from_le_bytes([config[2], config[3]]);
    let baseline = net::info().unwrap().open_sockets;
    let options = http::Options {
        dns_server: [10, 0, 2, 2],
        dns_port,
        ..http::Options::default()
    };
    for _ in 0..3 {
        for path in [
            "/",
            "/redirect",
            "/chunked",
            "/close",
            "/large",
            "/truncated",
        ] {
            use core::fmt::Write;
            let mut url = Url {
                data: [0; 96],
                len: 0,
            };
            write!(&mut url, "http://fixture.test:{}{}", port, path).unwrap();
            let mut data = [0; 32];
            let mut length = 0;
            let result = http::get_with(
                core::str::from_utf8(&url.data[..url.len]).unwrap(),
                options,
                |bytes| {
                    if path == "/large" {
                        assert!(bytes.iter().all(|b| *b == b'x'));
                    } else {
                        assert!(length + bytes.len() <= data.len());
                        data[length..length + bytes.len()].copy_from_slice(bytes);
                    }
                    length += bytes.len();
                    Ok(())
                },
            );
            if path == "/truncated" {
                assert_eq!(result, Err(http::Error::Truncated));
            } else {
                let response = result.unwrap();
                assert_eq!(response.bytes, length);
                if path == "/close" {
                    assert_eq!(response.status, 404);
                    assert_eq!(&data[..length], b"eof-ok");
                } else if path == "/large" {
                    assert_eq!(length, 20000);
                } else {
                    assert_eq!(&data[..length], b"http-fixture-ok");
                }
            }
            assert_eq!(net::info().unwrap().open_sockets, baseline);
        }
    }
    assert_eq!(
        http::get("ftp://example.com/", |_| Ok(())),
        Err(http::Error::UnsupportedScheme)
    );
    assert_eq!(net::info().unwrap().open_sockets, baseline);
    println!("[httptest] PASS: DNS HTTP framing redirects errors streaming cleanup");
    sys::exit(0);
}
struct Url {
    data: [u8; 96],
    len: usize,
}
impl core::fmt::Write for Url {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.data.len() {
            return Err(core::fmt::Error);
        }
        self.data[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
