use rustls::{
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    ServerConfig, ServerConnection,
};
use rustos_tls::{Error, Io, Session};
use std::{
    cell::Cell,
    collections::VecDeque,
    io::{Cursor, Read, Write},
    sync::Arc,
};
const CA: &[u8] = include_bytes!("fixtures/ca.der");
const VALID: &[u8] = include_bytes!("fixtures/valid.der");
const KEY: &[u8] = include_bytes!("fixtures/valid-key.der");
const NOW: u64 = 1800000000;
struct Peer {
    server: ServerConnection,
    queue: VecDeque<u8>,
    request: Vec<u8>,
    responded: bool,
    checks: Cell<usize>,
    tamper: bool,
    eof: bool,
    alert_received: bool,
}
impl Peer {
    fn new(cert: &[u8], key: &[u8]) -> Self {
        Self::with_suite(cert, key, rustls_rustcrypto::TLS13_CHACHA20_POLY1305_SHA256)
    }
    fn with_suite(cert: &[u8], key: &[u8], suite: rustls::SupportedCipherSuite) -> Self {
        let mut p = rustls_rustcrypto::provider();
        p.cipher_suites = vec![suite];
        p.kx_groups
            .retain(|group| group.name() == rustls::NamedGroup::X25519);
        let c = ServerConfig::builder_with_provider(Arc::new(p))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert.to_vec())],
                PrivatePkcs8KeyDer::from(key.to_vec()).into(),
            )
            .unwrap();
        Self {
            server: ServerConnection::new(Arc::new(c)).unwrap(),
            queue: VecDeque::new(),
            request: vec![],
            responded: false,
            checks: Cell::new(0),
            tamper: false,
            eof: false,
            alert_received: false,
        }
    }
}
impl Io for Peer {
    fn read(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        if self.eof {
            return Ok(0);
        }
        if self.queue.is_empty() {
            let mut bytes = vec![];
            while self.server.wants_write() {
                self.server.write_tls(&mut bytes).unwrap();
            }
            if self.tamper && self.responded && !bytes.is_empty() {
                let last = bytes.len() - 1;
                bytes[last] ^= 1;
            }
            self.queue.extend(bytes);
        }
        let n = out.len().min(17).min(self.queue.len());
        for b in &mut out[..n] {
            *b = self.queue.pop_front().unwrap();
        }
        Ok(n)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), Error> {
        let mut cursor = Cursor::new(data);
        while (cursor.position() as usize) < data.len() {
            self.server.read_tls(&mut cursor).map_err(|_| Error::Io)?;
            match self.server.process_new_packets() {
                Ok(_) => (),
                Err(rustls::Error::AlertReceived(_)) => {
                    self.alert_received = true;
                    return Ok(()); // TCP accepted the alert, even though TLS rejects it.
                }
                Err(_) => return Err(Error::Io),
            }
        }
        let mut b = [0; 1024];
        while let Ok(n) = self.server.reader().read(&mut b) {
            if n == 0 {
                break;
            }
            self.request.extend_from_slice(&b[..n]);
        }
        if self.request.ends_with(b"\r\n\r\n") && !self.responded {
            self.server
                .writer()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
            self.server.send_close_notify();
            self.responded = true;
        }
        Ok(())
    }
    fn check_deadline(&self) -> Result<(), Error> {
        self.checks.set(self.checks.get() + 1);
        if self.checks.get() > 10000 {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
}
#[test]
fn verified_fragmented_handshake_and_data() {
    for (cert, key) in [
        (VALID, KEY),
        (
            include_bytes!("fixtures/rsa.der").as_slice(),
            include_bytes!("fixtures/rsa-key.der").as_slice(),
        ),
    ] {
        let mut peer = Peer::new(cert, key);
        let mut c = Session::new("fixture.test", NOW, Some(CA)).unwrap();
        c.handshake(&mut peer).unwrap();
        c.write(&mut peer, b"GET / HTTP/1.1\r\nHost: fixture.test\r\n\r\n")
            .unwrap();
        let mut result = vec![];
        let mut b = [0; 3];
        loop {
            let n = c.read(&mut peer, &mut b).unwrap();
            if n == 0 {
                break;
            }
            result.extend_from_slice(&b[..n]);
        }
        assert!(result.ends_with(b"\r\n\r\nok"));
        c.close(&mut peer).unwrap();
    }
}
#[test]
fn rejects_untrusted_wrong_name_and_invalid_dates() {
    for (cert, key, host, root) in [
        (VALID, KEY, "fixture.test", None),
        (VALID, KEY, "wrong.test", Some(CA)),
        (
            include_bytes!("fixtures/expired.der").as_slice(),
            include_bytes!("fixtures/expired-key.der").as_slice(),
            "fixture.test",
            Some(CA),
        ),
        (
            include_bytes!("fixtures/future.der").as_slice(),
            include_bytes!("fixtures/future-key.der").as_slice(),
            "fixture.test",
            Some(CA),
        ),
    ] {
        let mut peer = Peer::new(cert, key);
        let mut c = Session::new(host, NOW, root).unwrap();
        assert_eq!(c.handshake(&mut peer), Err(Error::Certificate));
        assert!(peer.request.is_empty());
    }
}
#[test]
fn rejects_invalid_clock_and_ragged_eof() {
    assert!(matches!(
        Session::new("fixture.test", 0, Some(CA)),
        Err(Error::Clock)
    ));
    let mut peer = Peer::new(VALID, KEY);
    let mut c = Session::new("fixture.test", NOW, Some(CA)).unwrap();
    peer.eof = true;
    assert_eq!(c.handshake(&mut peer), Err(Error::Truncated));
}
#[test]
fn damaged_records_are_rejected() {
    let mut peer = Peer::new(VALID, KEY);
    let mut c = Session::new("fixture.test", NOW, Some(CA)).unwrap();
    c.handshake(&mut peer).unwrap();
    peer.tamper = true;
    c.write(&mut peer, b"GET / HTTP/1.1\r\n\r\n").unwrap();
    let mut b = [0; 100];
    let mut failed = false;
    for _ in 0..10 {
        match c.read(&mut peer, &mut b) {
            Err(_) => {
                failed = true;
                break;
            }
            Ok(0) => break,
            _ => (),
        }
    }
    assert!(failed, "altered authenticated record must fail");
}

#[test]
fn interoperates_with_each_tls13_cipher_suite() {
    for suite in [
        rustls_rustcrypto::TLS13_CHACHA20_POLY1305_SHA256,
        rustls_rustcrypto::TLS13_AES_128_GCM_SHA256,
        rustls_rustcrypto::TLS13_AES_256_GCM_SHA384,
    ] {
        let mut peer = Peer::with_suite(VALID, KEY, suite);
        let mut client = Session::new("fixture.test", NOW, Some(CA)).unwrap();
        client
            .handshake(&mut peer)
            .expect("server's sole cipher must negotiate");
        client.write(&mut peer, b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let mut response = vec![];
        let mut buf = [0; 128];
        loop {
            let n = client.read(&mut peer, &mut buf).unwrap();
            if n == 0 {
                break;
            }
            response.extend_from_slice(&buf[..n]);
        }
        assert!(response.ends_with(b"\r\n\r\nok"));
    }
}

#[test]
fn expired_deadline_also_rejects_buffered_plaintext() {
    let mut peer = Peer::new(VALID, KEY);
    let mut client = Session::new("fixture.test", NOW, Some(CA)).unwrap();
    client.handshake(&mut peer).unwrap();
    client.write(&mut peer, b"GET / HTTP/1.1\r\n\r\n").unwrap();
    let mut byte = [0; 1];
    assert_eq!(client.read(&mut peer, &mut byte), Ok(1));
    peer.checks.set(10000);
    assert_eq!(client.read(&mut peer, &mut byte), Err(Error::Timeout));
}

#[test]
fn certificate_rejection_flushes_fatal_alert_without_waiting_for_peer() {
    let mut peer = Peer::new(VALID, KEY);
    let mut client = Session::new("wrong.test", NOW, Some(CA)).unwrap();
    assert_eq!(client.handshake(&mut peer), Err(Error::Certificate));
    let checks = peer.checks.get();
    let _ = client.close(&mut peer);
    assert!(
        peer.alert_received,
        "fatal rejection must reach the TLS peer"
    );
    assert!(
        peer.checks.get() - checks < 16,
        "fatal close must never wait for input"
    );
    assert!(peer.request.is_empty());
    assert_eq!(
        client.write(&mut peer, b"no plaintext"),
        Err(Error::Certificate)
    );
    assert_eq!(client.read(&mut peer, &mut [0; 1]), Err(Error::Certificate));
    let checks = peer.checks.get();
    client.close(&mut peer).unwrap();
    assert_eq!(peer.checks.get(), checks, "fatal close is idempotent");
}
