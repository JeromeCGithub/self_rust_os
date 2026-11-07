use rustos_http::{get, Error, Options, Transport};
use rustos_net_abi::{Error as SE, Kind, Packet, CONNECTED};
use std::collections::VecDeque;
#[derive(Default)]
struct Mock {
    time: u64,
    cancel_at: Option<u64>,
    cancel_after_send: Option<usize>,
    fail_after_send: Option<usize>,
    opened: usize,
    closed: usize,
    sent: Vec<u8>,
    incoming: VecDeque<Vec<u8>>,
    no_data: bool,
    repeated: usize,
    short: usize,
    tls: bool,
    tls_calls: Vec<String>,
    ports: Vec<u16>,
}
impl Transport for Mock {
    fn cancelled(&self) -> bool {
        self.cancel_at.is_some_and(|at| self.time >= at)
            || self
                .cancel_after_send
                .is_some_and(|at| self.sent.len() >= at)
    }
    fn start_tls(&mut self, _: u64, host: &str, _: u64, _: Option<&str>) -> Result<(), Error> {
        self.tls_calls.push(host.to_owned());
        if self.tls {
            Ok(())
        } else {
            Err(Error::UnsupportedScheme)
        }
    }
    fn open(&mut self, _: Kind) -> Result<u64, SE> {
        self.opened += 1;
        Ok(self.opened as u64)
    }
    fn connect(&mut self, _: u64, _: [u8; 4], port: u16) -> Result<(), SE> {
        self.ports.push(port);
        Ok(())
    }
    fn send(&mut self, _: u64, data: &[u8]) -> Result<usize, SE> {
        if self.fail_after_send.is_some_and(|at| self.sent.len() >= at) {
            return Err(SE::Closed);
        }
        let n = data.len().min(if self.short == 0 {
            usize::MAX
        } else {
            self.short
        });
        self.sent.extend_from_slice(&data[..n]);
        Ok(n)
    }
    fn recv(&mut self, _: u64, p: &mut Packet) -> Result<(), SE> {
        if self.no_data {
            return Err(SE::WouldBlock);
        }
        *p = Packet::empty();
        if let Some(b) = self.incoming.pop_front() {
            p.len = b.len() as u16;
            p.data[..b.len()].copy_from_slice(&b)
        } else if self.repeated > 0 {
            let n = self.repeated.min(p.data.len());
            p.data[..n].fill(b'x');
            p.len = n as u16;
            self.repeated -= n;
        }
        Ok(())
    }
    fn poll(&mut self, _: u64) -> Result<u64, SE> {
        Ok(CONNECTED)
    }
    fn close(&mut self, _: u64) {
        self.closed += 1
    }
    fn ticks(&self) -> u64 {
        self.time
    }
    fn sleep(&mut self) {
        self.time += 1
    }
}
fn fetch(raw: &[u8], fragment: usize) -> (Result<rustos_http::Response, Error>, Vec<u8>, Mock) {
    let mut m = Mock {
        short: 3,
        ..Mock::default()
    };
    m.incoming = raw.chunks(fragment).map(|b| b.to_vec()).collect();
    let mut body = vec![];
    let result = get(
        &mut m,
        "http://10.0.2.2:8080/a?q=x#ignored",
        Options::default(),
        |b| {
            body.extend_from_slice(b);
            Ok(())
        },
    );
    (result, body, m)
}
#[test]
fn content_length_and_fragmented_headers_short_writes() {
    let (r, b, m) = fetch(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", 1);
    assert_eq!(r.unwrap().bytes, 5);
    assert_eq!(b, b"hello");
    assert!(String::from_utf8(m.sent)
        .unwrap()
        .starts_with("GET /a?q=x HTTP/1.1\r\nHost: 10.0.2.2:8080\r\n"));
    assert_eq!((m.opened, m.closed), (1, 1));
}

#[test]
fn cookies_are_selected_for_each_redirect_url_and_injection_fails_closed() {
    use rustos_http::{request_with_cookies, Request, RequestCookies};
    struct Cookies {
        seen: Vec<String>,
        bad: bool,
    }
    impl RequestCookies for Cookies {
        fn header(&mut self, url: &str) -> Result<&str, Error> {
            self.seen.push(url.into());
            Ok(if self.bad {
                "x=y\r\nHost: evil"
            } else if url.starts_with("https://10.0.2.2/") {
                "proof=abc"
            } else {
                ""
            })
        }
    }
    let mut m = Mock {
        tls: true,
        short: 3,
        ..Mock::default()
    };
    m.incoming.push_back(
        b"HTTP/1.1 302 Found\r\nLocation: https://10.0.2.3/other\r\nContent-Length: 0\r\n\r\n"
            .to_vec(),
    );
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    let mut cookies = Cookies {
        seen: vec![],
        bad: false,
    };
    request_with_cookies(
        &mut m,
        "https://10.0.2.2/search",
        Request::Get,
        Options::default(),
        |_| Ok(()),
        |_| {},
        &mut cookies,
    )
    .unwrap();
    let sent = String::from_utf8(m.sent.clone()).unwrap();
    assert_eq!(sent.matches("Cookie: proof=abc\r\n").count(), 1);
    assert_eq!(
        cookies.seen,
        vec!["https://10.0.2.2/search", "https://10.0.2.3/other"]
    );
    assert!(!sent.split("GET /other").nth(1).unwrap().contains("Cookie:"));
    assert_eq!((m.opened, m.closed), (2, 2));
    let mut bad = Mock {
        tls: true,
        ..Mock::default()
    };
    cookies.bad = true;
    assert_eq!(
        request_with_cookies(
            &mut bad,
            "https://10.0.2.2/",
            Request::Get,
            Options::default(),
            |_| Ok(()),
            |_| {},
            &mut cookies
        ),
        Err(Error::Protocol)
    );
    assert!(bad.sent.is_empty());
    assert_eq!((bad.opened, bad.closed), (1, 1));
}

#[test]
fn ranged_get_validates_coordinates_and_exact_body_before_accepting_a_seek() {
    use rustos_http::{request_with_url, ByteRange, ContentRange, Request};
    for (last, total) in [(7, 100), (7, 8)] {
        let mut m = Mock {
            short: 3,
            ..Mock::default()
        };
        m.incoming.push_back(format!("HTTP/1.1 206 Partial\r\nContent-Range: bytes 4-{last}/{total}\r\nContent-Length: 4\r\n\r\nefgh").into_bytes());
        let mut data = vec![];
        let result = request_with_url(
            &mut m,
            "http://10.0.2.2/media",
            Request::GetRange(ByteRange { first: 4, last: 10 }),
            Options::default(),
            |part| {
                data.extend_from_slice(part);
                Ok(())
            },
            |_| {},
        );
        if total == 8 {
            assert_eq!(
                result.unwrap().range,
                Some(ContentRange::Satisfied {
                    first: 4,
                    last: 7,
                    total: 8
                })
            );
            assert_eq!(data, b"efgh");
        } else {
            assert_eq!(result, Err(Error::Protocol));
            assert!(data.is_empty());
        }
        assert!(String::from_utf8(m.sent)
            .unwrap()
            .contains("\r\nRange: bytes=4-10\r\n"));
        assert_eq!(m.closed, 1);
    }
}

#[test]
fn media_range_rejects_ignored_unsatisfiable_and_malformed_responses_without_body_delivery() {
    use rustos_http::{request_with_url, ByteRange, Request};
    for (head,expected) in [
        ("HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd",Error::RangeIgnored),
        ("HTTP/1.1 416 Range\r\nContent-Range: bytes */4\r\nContent-Length: 0\r\n\r\n",Error::RangeUnsatisfiable),
        ("HTTP/1.1 206 Partial\r\nContent-Range: bytes 3-7/8\r\nContent-Length: 5\r\n\r\ndefgh",Error::Protocol),
        ("HTTP/1.1 206 Partial\r\nContent-Range: bytes 4-8/8\r\n\r\n",Error::Protocol),
        ("HTTP/1.1 206 Partial\r\nContent-Length: 4\r\n\r\nefgh",Error::Protocol),
        ("HTTP/1.1 206 Partial\r\nContent-Range: bytes 4-7/8\r\nContent-Range: bytes 4-7/8\r\n\r\n",Error::Protocol),
    ] {
        let mut m=Mock::default();m.incoming.push_back(head.as_bytes().to_vec());let mut delivered=0;
        assert_eq!(request_with_url(&mut m,"http://10.0.2.2/media",Request::GetRange(ByteRange{first:4,last:7}),Options::default(),|part|{delivered+=part.len();Ok(())},|_|{}),Err(expected));
        assert_eq!(delivered,0);assert_eq!(m.closed,1);
    }
}

#[test]
fn media_range_bounds_chunked_truncation_and_redirects_preserve_cleanup() {
    use rustos_http::{request_with_url, ByteRange, Request};
    for range in [
        ByteRange { first: 8, last: 7 },
        ByteRange {
            first: 0,
            last: u64::MAX,
        },
        ByteRange {
            first: 0,
            last: 1000,
        },
    ] {
        let mut m = Mock::default();
        assert!(request_with_url(
            &mut m,
            "http://10.0.2.2/media",
            Request::GetRange(range),
            Options {
                max_body: 8,
                ..Default::default()
            },
            |_| Ok(()),
            |_| {}
        )
        .is_err());
        assert_eq!(m.opened, 0);
    }
    let mut m = Mock::default();
    m.incoming
        .push_back(b"HTTP/1.1 302 Redirect\r\nLocation: /next\r\n\r\n".to_vec());
    m.incoming.push_back(b"HTTP/1.1 206 Partial\r\nContent-Range: bytes 4-7/10\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nefg\r\n0\r\n\r\n".to_vec());
    assert_eq!(
        request_with_url(
            &mut m,
            "http://10.0.2.2/media",
            Request::GetRange(ByteRange { first: 4, last: 7 }),
            Options::default(),
            |_| Ok(()),
            |_| {}
        ),
        Err(Error::Truncated)
    );
    assert_eq!(m.closed, 2);
    assert_eq!(
        String::from_utf8(m.sent)
            .unwrap()
            .matches("Range: bytes=4-7")
            .count(),
        2
    );
}

#[test]
fn form_post_short_writes_body_and_content_headers_without_url_leakage() {
    use rustos_http::{request_with_url, Request};
    let mut m = Mock {
        short: 3,
        ..Mock::default()
    };
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    let mut out = vec![];
    let mut effective = String::new();
    let response = request_with_url(
        &mut m,
        "http://10.0.2.2/login?flow=1",
        Request::FormPost(b"user=guest&password=testpass"),
        Options::default(),
        |part| {
            out.extend_from_slice(part);
            Ok(())
        },
        |url| effective = url.into(),
    )
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(out, b"ok");
    let sent = String::from_utf8(m.sent).unwrap();
    assert!(sent.starts_with("POST /login?flow=1 HTTP/1.1\r\n"));
    assert!(sent
        .contains("\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 28\r\n"));
    assert!(sent.ends_with("\r\n\r\nuser=guest&password=testpass"));
    assert_eq!(effective, "http://10.0.2.2/login?flow=1");
    assert_eq!((m.opened, m.closed), (1, 1));
}

#[test]
fn post_redirect_method_rules_and_same_origin_body_replay() {
    use rustos_http::{request_with_url, Request};
    for status in [301, 302, 303, 307, 308] {
        let mut m = Mock::default();
        m.incoming.push_back(
            format!("HTTP/1.1 {status} Redirect\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        );
        m.incoming.push_back(b"HTTP/1.1 204 Empty\r\n\r\n".to_vec());
        request_with_url(
            &mut m,
            "http://10.0.2.2/login",
            Request::FormPost(b"x=1"),
            Options::default(),
            |_| Ok(()),
            |_| {},
        )
        .unwrap();
        let sent = String::from_utf8(m.sent).unwrap();
        assert!(sent.starts_with("POST /login HTTP/1.1"));
        if status >= 307 {
            assert!(sent.contains("POST /next HTTP/1.1"));
            assert_eq!(sent.matches("x=1").count(), 2);
        } else {
            assert!(sent.contains("GET /next HTTP/1.1"));
            assert_eq!(sent.matches("x=1").count(), 1);
            assert_eq!(sent.matches("Content-Type:").count(), 1);
        }
        assert_eq!((m.opened, m.closed), (2, 2));
    }
}

#[test]
fn post_cross_origin_replay_downgrade_and_oversize_are_rejected_and_closed() {
    use rustos_http::{request_with_url, Request};
    for target in [
        "http://10.0.2.3/next",
        "http://10.0.2.2:8080/next",
        "https://10.0.2.2/next",
    ] {
        let mut m = Mock::default();
        m.incoming.push_back(
            format!("HTTP/1.1 307 Redirect\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        );
        assert_eq!(
            request_with_url(
                &mut m,
                "http://10.0.2.2/login",
                Request::FormPost(b"secret=x"),
                Options::default(),
                |_| Ok(()),
                |_| {}
            ),
            Err(Error::CrossOriginRedirect)
        );
        assert_eq!((m.opened, m.closed), (1, 1));
    }
    let mut m = Mock {
        tls: true,
        ..Mock::default()
    };
    m.incoming
        .push_back(b"HTTP/1.1 307 Redirect\r\nLocation: http://10.0.2.2/next\r\n\r\n".to_vec());
    assert_eq!(
        request_with_url(
            &mut m,
            "https://10.0.2.2/login",
            Request::FormPost(b"secret=x"),
            Options::default(),
            |_| Ok(()),
            |_| {}
        ),
        Err(Error::InsecureRedirect)
    );
    assert_eq!(m.closed, 1);
    let mut m = Mock::default();
    assert_eq!(
        request_with_url(
            &mut m,
            "http://10.0.2.2/login",
            Request::FormPost(&vec![b'x'; 16385]),
            Options::default(),
            |_| Ok(()),
            |_| {}
        ),
        Err(Error::TooLarge)
    );
    assert_eq!(m.opened, 0);
}

#[test]
fn post_empty_boundary_bodies_cancelled_upload_and_send_failure_release_sockets() {
    use rustos_http::{request_with_url, Request};
    for size in [0, 16384] {
        let mut m = Mock {
            short: 7,
            ..Mock::default()
        };
        m.incoming.push_back(b"HTTP/1.1 204 Empty\r\n\r\n".to_vec());
        let data = vec![b'x'; size];
        request_with_url(
            &mut m,
            "http://10.0.2.2/",
            Request::FormPost(&data),
            Options::default(),
            |_| Ok(()),
            |_| {},
        )
        .unwrap();
        let end = m.sent.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(&m.sent[end..], data);
        assert_eq!(m.closed, 1);
    }
    for cancel in [true, false] {
        let mut m = Mock {
            short: 7,
            cancel_after_send: if cancel { Some(512) } else { None },
            fail_after_send: if cancel { None } else { Some(512) },
            ..Mock::default()
        };
        let data = vec![b'x'; 16384];
        let result = request_with_url(
            &mut m,
            "http://10.0.2.2/",
            Request::FormPost(&data),
            Options::default(),
            |_| Ok(()),
            |_| {},
        );
        assert_eq!(
            result,
            Err(if cancel {
                Error::Cancelled
            } else {
                Error::Socket(SE::Closed)
            })
        );
        let end = m.sent.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        assert!(
            !m.sent[end..].is_empty() && m.sent[end..].len() < data.len(),
            "stop during upload, not before it"
        );
        assert_eq!((m.opened, m.closed), (1, 1));
    }
}
#[test]
fn chunked_extensions_trailers_and_fragmentation() {
    let(r,b,m)=fetch(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;foo=bar\r\nabc\r\n2\r\nde\r\n0\r\nX-End: yes\r\n\r\n",2);
    assert_eq!(r.unwrap().bytes, 5);
    assert_eq!(b, b"abcde");
    assert_eq!(m.closed, 1);
}
#[test]
fn eof_framing_status_and_informational_response() {
    let (r, b, _) = fetch(
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.0 404 Missing\r\n\r\nnot found",
        7,
    );
    assert_eq!(r.unwrap().status, 404);
    assert_eq!(b, b"not found");
}
#[test]
fn incomplete_and_ambiguous_messages_fail_and_close() {
    for raw in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nshort"[..],
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\na",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 1\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
        b"HTTP/1.1 200 OK\r\n\r\n",
    ]
    .iter()
    .take(4)
    {
        let (r, _, m) = fetch(raw, 13);
        assert!(r.is_err());
        assert_eq!(m.closed, 1);
    }
}
#[test]
fn no_body_204_and_304() {
    for s in [204, 304] {
        let raw = format!("HTTP/1.1 {s} Empty\r\nContent-Length: 500\r\n\r\n");
        let (r, b, _) = fetch(raw.as_bytes(), 11);
        assert_eq!(r.unwrap().bytes, 0);
        assert!(b.is_empty());
    }
}
#[test]
fn timeout_output_failure_and_body_limit_release_socket() {
    let mut m = Mock {
        no_data: true,
        ..Mock::default()
    };
    let opt = Options {
        timeout_ticks: 3,
        ..Options::default()
    };
    assert_eq!(
        get(&mut m, "http://10.0.2.2/", opt, |_| Ok(())),
        Err(Error::Timeout)
    );
    assert_eq!(m.closed, 1);
    let mut m = Mock::default();
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc".to_vec());
    assert_eq!(
        get(&mut m, "http://10.0.2.2/", Options::default(), |_| Err(
            Error::Output
        )),
        Err(Error::Output)
    );
    assert_eq!(m.closed, 1);
    let mut m = Mock::default();
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc".to_vec());
    assert_eq!(
        get(
            &mut m,
            "http://10.0.2.2/",
            Options {
                max_body: 2,
                ..Options::default()
            },
            |_| Ok(())
        ),
        Err(Error::TooLarge)
    );
    assert_eq!(m.closed, 1);
}
#[test]
fn invalid_urls_never_open_sockets() {
    for url in [
        "ftp://example.com/",
        "http://",
        "http://u:p@host/",
        "http://host:0/",
        "http://host/\r\nBad: x",
        "http://[::1]/",
        "http://host:99999/",
        "http://-bad.test/",
    ] {
        let mut m = Mock::default();
        assert!(get(&mut m, url, Options::default(), |_| Ok(())).is_err());
        assert_eq!(m.opened, 0, "{url}");
    }
}
#[test]
fn relative_redirect_and_loop_are_bounded() {
    let mut m = Mock::default();
    m.incoming
        .push_back(b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n".to_vec());
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    let mut out = vec![];
    assert_eq!(
        get(&mut m, "http://10.0.2.2/", Options::default(), |b| {
            out.extend_from_slice(b);
            Ok(())
        })
        .unwrap()
        .status,
        200
    );
    assert_eq!(out, b"ok");
    assert_eq!(m.closed, 2);
    let mut m = Mock::default();
    m.incoming
        .push_back(b"HTTP/1.1 302 Found\r\nLocation: /\r\nContent-Length: 0\r\n\r\n".to_vec());
    assert_eq!(
        get(
            &mut m,
            "http://10.0.2.2/",
            Options {
                redirects: 0,
                ..Options::default()
            },
            |_| Ok(())
        ),
        Err(Error::RedirectLimit)
    );
    assert_eq!(m.closed, 1);
}

#[test]
fn malformed_status_chunk_and_header_limits() {
    for raw in [
        &b"HTTP/1.1 xxx Nope\r\n\r\n"[..],
        b"HTTP/1.1 101 Upgrade\r\n\r\n",
        b"HTTP/1.1 200 OK\r\n Bad: fold\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nz\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\naX\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 999999999999999999999999999999\r\n\r\n",
    ] {
        let (r, _, m) = fetch(raw, 3);
        assert!(r.is_err());
        assert_eq!(m.closed, 1);
    }
    let mut raw = b"HTTP/1.1 200 OK\r\nX-Long: ".to_vec();
    raw.extend_from_slice(&vec![b'a'; 32 * 1024 + 1]);
    raw.extend_from_slice(b"\r\n\r\n");
    let (r, _, m) = fetch(&raw, 100);
    assert_eq!(r, Err(Error::TooLarge));
    assert_eq!(m.closed, 1);
}
#[test]
fn large_body_streams_without_collecting_and_can_cancel() {
    let mut m = Mock::default();
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 20000\r\n\r\n".to_vec());
    for _ in 0..20 {
        m.incoming.push_back(vec![b'x'; 1000]);
    }
    let mut count = 0;
    assert_eq!(
        get(&mut m, "http://10.0.2.2/", Options::default(), |b| {
            assert!(b.iter().all(|v| *v == b'x'));
            count += b.len();
            Ok(())
        })
        .unwrap()
        .bytes,
        20000
    );
    assert_eq!(count, 20000);
    assert_eq!(m.closed, 1);
}
#[test]
fn dns_query_matching_cname_and_udp_cleanup() {
    struct DnsMock {
        base: Mock,
        query: Vec<u8>,
        attempt: usize,
    }
    impl Transport for DnsMock {
        fn open(&mut self, k: Kind) -> Result<u64, SE> {
            self.base.open(k)
        }
        fn connect(&mut self, h: u64, a: [u8; 4], p: u16) -> Result<(), SE> {
            self.base.connect(h, a, p)
        }
        fn send(&mut self, h: u64, b: &[u8]) -> Result<usize, SE> {
            if h == 1 {
                self.query = b.to_vec();
                Ok(b.len())
            } else {
                self.base.send(h, b)
            }
        }
        fn recv(&mut self, h: u64, p: &mut Packet) -> Result<(), SE> {
            if h != 1 {
                return self.base.recv(h, p);
            }
            self.attempt += 1;
            let q = &self.query;
            let mut raw = q.clone();
            raw[2] = 0x81;
            raw[3] = 0x80;
            raw[7] = 2;
            // owner=query; CNAME alias.test; its A record uses a compressed pointer.
            raw.extend_from_slice(&[0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 30, 0, 12]);
            let offset = raw.len();
            raw.extend_from_slice(b"\x05alias\x04test\0");
            raw.extend_from_slice(&[
                0xc0 | ((offset >> 8) as u8),
                offset as u8,
                0,
                1,
                0,
                1,
                0,
                0,
                0,
                30,
                0,
                4,
                10,
                0,
                2,
                2,
            ]);
            if self.attempt == 1 {
                raw[0] ^= 1;
            } // A foreign transaction must be ignored.
            *p = Packet::empty();
            p.address = [10, 0, 2, 3];
            p.port = 53;
            p.len = raw.len() as u16;
            p.data[..raw.len()].copy_from_slice(&raw);
            Ok(())
        }
        fn poll(&mut self, h: u64) -> Result<u64, SE> {
            self.base.poll(h)
        }
        fn close(&mut self, h: u64) {
            self.base.close(h)
        }
        fn ticks(&self) -> u64 {
            self.base.time
        }
        fn sleep(&mut self) {
            self.base.sleep()
        }
    }
    let mut m = DnsMock {
        base: Mock::default(),
        query: vec![],
        attempt: 0,
    };
    m.base
        .incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    assert_eq!(
        get(&mut m, "http://example.test/", Options::default(), |_| Ok(
            ()
        ))
        .unwrap()
        .bytes,
        2
    );
    assert_eq!(m.attempt, 2);
    assert_eq!((m.base.opened, m.base.closed), (2, 2));
}
#[test]
fn dns_timeout_retries_and_closes() {
    let mut m = Mock {
        no_data: true,
        ..Mock::default()
    };
    assert_eq!(
        get(&mut m, "http://missing.test/", Options::default(), |_| Ok(
            ()
        )),
        Err(Error::Timeout)
    );
    assert_eq!((m.opened, m.closed), (1, 1));
    assert_eq!(m.time, 54);
}

#[test]
fn https_uses_tls_and_default_port_before_sending_http() {
    let mut m = Mock {
        tls: true,
        ..Mock::default()
    };
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    assert_eq!(
        get(&mut m, "https://10.0.2.2/", Options::default(), |_| Ok(()))
            .unwrap()
            .bytes,
        2
    );
    assert_eq!(m.ports, vec![443]);
    assert_eq!(m.tls_calls, vec!["10.0.2.2"]);
    assert!(String::from_utf8(m.sent)
        .unwrap()
        .contains("Host: 10.0.2.2\r\n"));
    assert_eq!(m.closed, 1);
}
#[test]
fn unavailable_tls_never_sends_plaintext() {
    let mut m = Mock::default();
    assert_eq!(
        get(&mut m, "https://10.0.2.2/", Options::default(), |_| Ok(())),
        Err(Error::UnsupportedScheme)
    );
    assert!(m.sent.is_empty());
    assert_eq!(m.closed, 1);
}
#[test]
fn rejects_https_downgrade_and_preserves_relative_scheme() {
    let mut m = Mock {
        tls: true,
        ..Mock::default()
    };
    m.incoming.push_back(
        b"HTTP/1.1 302 Found\r\nLocation: http://10.0.2.2/plain\r\nContent-Length: 0\r\n\r\n"
            .to_vec(),
    );
    assert_eq!(
        get(&mut m, "https://10.0.2.2/", Options::default(), |_| Ok(())),
        Err(Error::InsecureRedirect)
    );
    assert_eq!(m.opened, 1);
    assert_eq!(m.closed, 1);
    let mut m = Mock {
        tls: true,
        ..Mock::default()
    };
    m.incoming.push_back(
        b"HTTP/1.1 302 Found\r\nLocation: /secure\r\nContent-Length: 0\r\n\r\n".to_vec(),
    );
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec());
    assert_eq!(
        get(&mut m, "https://10.0.2.2/", Options::default(), |_| Ok(()))
            .unwrap()
            .status,
        200
    );
    assert_eq!(m.ports, vec![443, 443]);
    assert_eq!(m.tls_calls.len(), 2);
    assert_eq!(m.closed, 2);
}

#[test]
fn accepts_modern_security_headers_and_larger_header_blocks() {
    let mut raw = b"HTTP/1.1 200 OK\r\nContent-Security-Policy: ".to_vec();
    raw.extend(std::iter::repeat(b'a').take(20000));
    raw.extend_from_slice(b"\r\n");
    for _ in 0..200 {
        raw.extend_from_slice(b"X-Padding: ");
        raw.extend(std::iter::repeat(b'b').take(800));
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"Content-Length: 2\r\n\r\nok");
    let (result, body, mock) = fetch(&raw, 997);
    assert_eq!(result.unwrap().bytes, 2);
    assert_eq!(body, b"ok");
    assert_eq!(mock.closed, 1);
}

#[test]
fn larger_header_line_and_total_limits_still_close_the_socket() {
    for kind in 0..2 {
        let mut raw = b"HTTP/1.1 200 OK\r\n".to_vec();
        if kind == 0 {
            raw.extend_from_slice(b"X: ");
            raw.extend(std::iter::repeat(b'a').take(32 * 1024 - 3));
            raw.extend_from_slice(b"\r\nContent-Length: 2\r\n\r\nok");
            let (result, body, mock) = fetch(&raw, 997);
            assert_eq!(result.unwrap().bytes, 2);
            assert_eq!(body, b"ok");
            assert_eq!(mock.closed, 1);
            raw.insert(20, b'a'); // one byte over the complete line limit
        } else {
            for _ in 0..9 {
                raw.extend_from_slice(b"X: ");
                raw.extend(std::iter::repeat(b'a').take(30000));
                raw.extend_from_slice(b"\r\n");
            }
            raw.extend_from_slice(b"\r\n");
        }
        let (result, body, mock) = fetch(&raw, 997);
        assert_eq!(result, Err(Error::TooLarge));
        assert!(body.is_empty());
        assert_eq!(mock.closed, 1);
    }
}

#[test]
fn follows_long_redirects_with_partial_request_writes() {
    let path = format!("/{}", "a".repeat(8192));
    let response = format!("HTTP/1.1 302 Found\r\nLocation: {path}\r\nContent-Length: 0\r\n\r\n");
    let mut mock = Mock {
        short: 37,
        ..Mock::default()
    };
    mock.incoming
        .extend(response.as_bytes().chunks(997).map(|b| b.to_vec()));
    mock.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    assert_eq!(
        get(
            &mut mock,
            "http://10.0.2.2/",
            Options::default(),
            |_| Ok(())
        )
        .unwrap()
        .bytes,
        2
    );
    assert!(String::from_utf8(mock.sent)
        .unwrap()
        .contains(&format!("GET {path} HTTP/1.1\r\n")));
    assert_eq!((mock.opened, mock.closed), (2, 2));
}

#[test]
fn accepts_long_chunk_extensions_and_trailers() {
    let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;x=".to_vec();
    raw.extend(std::iter::repeat(b'a').take(4000));
    raw.extend_from_slice(b"\r\nok\r\n0\r\nX-Trailer: ");
    raw.extend(std::iter::repeat(b'b').take(20000));
    raw.extend_from_slice(b"\r\n\r\n");
    let (result, body, mock) = fetch(&raw, 997);
    assert_eq!(result.unwrap().bytes, 2);
    assert_eq!(body, b"ok");
    assert_eq!(mock.closed, 1);
}

#[test]
fn streams_up_to_the_expanded_default_body_limit() {
    let mut mock = Mock {
        repeated: 256 * 1024 * 1024,
        ..Mock::default()
    };
    mock.incoming.push_back(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            mock.repeated
        )
        .into_bytes(),
    );
    let mut count = 0;
    let result = get(&mut mock, "http://10.0.2.2/", Options::default(), |bytes| {
        assert_eq!(bytes[0], b'x');
        count += bytes.len();
        Ok(())
    });
    assert_eq!(result.unwrap().bytes, 256 * 1024 * 1024);
    assert_eq!(count, 256 * 1024 * 1024);
    assert_eq!(mock.closed, 1);
    mock.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 268435457\r\n\r\n".to_vec());
    assert_eq!(
        get(
            &mut mock,
            "http://10.0.2.2/",
            Options::default(),
            |_| panic!("oversized declared body must be rejected before output")
        ),
        Err(Error::TooLarge)
    );
    assert_eq!(mock.closed, 2);
}

#[test]
fn final_url_after_redirect_is_observed_once_for_relative_resources() {
    let mut m = Mock::default();
    m.incoming.push_back(
        b"HTTP/1.1 302 Found\r\nLocation: /directory/page\r\nContent-Length: 0\r\n\r\n".to_vec(),
    );
    m.incoming
        .push_back(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
    let mut urls = vec![];
    let mut bytes = vec![];
    let r = rustos_http::get_with_url(
        &mut m,
        "http://10.0.2.2/start",
        Options::default(),
        |b| {
            bytes.extend_from_slice(b);
            Ok(())
        },
        |u| urls.push(u.to_owned()),
    )
    .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(bytes, b"ok");
    assert_eq!(urls, ["http://10.0.2.2/directory/page"]);
    assert_eq!(m.opened, m.closed);
}

#[test]
fn response_reports_declared_charset_for_browser_decoding() {
    for (label, expected) in [
        ("ISO-8859-1", rustos_http::Charset::Windows1252),
        ("\"utf-8\"", rustos_http::Charset::Utf8),
        ("Shift_JIS", rustos_http::Charset::Other),
    ] {
        let raw=format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset={label}\r\nContent-Length: 1\r\n\r\nx");
        let (response, _, mock) = fetch(raw.as_bytes(), 7);
        assert_eq!(response.unwrap().charset, Some(expected));
        assert_eq!(mock.opened, mock.closed);
    }
}

#[test]
fn cancellation_before_open_and_during_headers_closes_only_owned_sockets() {
    let mut before = Mock {
        cancel_at: Some(0),
        ..Mock::default()
    };
    assert_eq!(
        get(
            &mut before,
            "http://127.0.0.1/",
            Options::default(),
            |_| Ok(())
        ),
        Err(Error::Cancelled)
    );
    assert_eq!((before.opened, before.closed), (0, 0));
    let mut blocked = Mock {
        cancel_at: Some(2),
        no_data: true,
        ..Mock::default()
    };
    assert_eq!(
        get(
            &mut blocked,
            "http://127.0.0.1/",
            Options::default(),
            |_| Ok(())
        ),
        Err(Error::Cancelled)
    );
    assert_eq!((blocked.opened, blocked.closed), (1, 1));
    assert_eq!(
        blocked.time, 2,
        "cancellation observed before ordinary request deadline"
    );
}
