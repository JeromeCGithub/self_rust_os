use rustos_browser_worker::{Endpoint, Error, Header, Kind, Session, HEADER, MAX_SOURCE};
use std::{cell::Cell, collections::VecDeque, rc::Rc};
fn header(kind: Kind, generation: u64, length: usize, status: u8) -> Header {
    Header {
        kind,
        generation,
        length,
        status,
    }
}
#[test]
fn framing_rejects_ambient_handles_lengths_reserved_bytes_and_wrong_versions() {
    let valid = header(Kind::Eval, 42, 7, 0);
    let bytes = valid.bytes().expect("valid frame");
    assert_eq!(Header::parse(bytes), Ok(valid));
    for (at, value) in [(0, b'X'), (3, 2), (4, 255), (6, 1), (20, 1)] {
        let mut broken = bytes;
        broken[at] = value;
        assert_eq!(Header::parse(broken), Err(Error::Protocol));
    }
    assert!(header(Kind::Eval, 0, 1, 0).bytes().is_err());
    assert!(header(Kind::Eval, 1, MAX_SOURCE + 1, 0).bytes().is_err());
    assert!(header(Kind::Failure, 1, 1, 3).bytes().is_err());
    assert!(header(Kind::Ready, 1, 0, 0).bytes().is_err());
    assert!(header(Kind::Result, 1, 8193, 0).bytes().is_err());
    assert_eq!(
        Header::parse(header(Kind::Ready, 0, 0, 0).bytes().unwrap())
            .unwrap()
            .kind,
        Kind::Ready
    );
}
struct Fake {
    incoming: VecDeque<u8>,
    sent: Vec<u8>,
    clock: u64,
    drops: Rc<Cell<u32>>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1)
    }
}
impl Endpoint for Fake {
    fn send(&mut self, b: u8) -> Result<bool, Error> {
        self.sent.push(b);
        Ok(true)
    }
    fn receive(&mut self) -> Result<Option<([u8; 3], usize)>, Error> {
        Ok(self.incoming.pop_front().map(|b| ([b, 0, 0], 1)))
    }
    fn ticks(&self) -> u64 {
        self.clock
    }
    fn idle(&mut self) {
        self.clock += 1
    }
}
fn fake(reply: Header, body: &[u8], drops: Rc<Cell<u32>>) -> Fake {
    let mut bytes = header(Kind::Ready, 0, 0, 0).bytes().unwrap().to_vec();
    bytes.extend_from_slice(&reply.bytes().unwrap());
    bytes.extend_from_slice(body);
    Fake {
        incoming: bytes.into(),
        sent: Vec::new(),
        clock: 0,
        drops,
    }
}
#[test]
fn copied_results_cancel_and_stale_replies_obey_session_ownership() {
    let drops = Rc::new(Cell::new(0));
    let mut session = Session::connect(
        fake(header(Kind::Result, 1, 2, 0), b"42", drops.clone()),
        || false,
    )
    .unwrap();
    assert_eq!(session.eval("6*7", || false), Ok("42".into()));
    assert_eq!(drops.get(), 0);
    drop(session);
    assert_eq!(drops.get(), 1);
    let mut stale = Session::connect(
        fake(header(Kind::Result, 2, 2, 0), b"42", drops.clone()),
        || false,
    )
    .unwrap();
    assert_eq!(stale.eval("6*7", || false), Err(Error::Protocol));
    assert_eq!(drops.get(), 2);
    let mut cancel = Session::connect(
        fake(header(Kind::Result, 1, 2, 0), b"42", drops.clone()),
        || false,
    )
    .unwrap();
    assert_eq!(cancel.eval("6*7", || true), Err(Error::Cancelled));
    assert_eq!(drops.get(), 3);
}
#[test]
fn missing_worker_response_is_bounded_and_freed() {
    let drops = Rc::new(Cell::new(0));
    let endpoint = Fake {
        incoming: VecDeque::new(),
        sent: Vec::new(),
        clock: 0,
        drops: drops.clone(),
    };
    assert!(matches!(
        Session::connect(endpoint, || false),
        Err(Error::Deadline)
    ));
    assert_eq!(drops.get(), 1);
    assert_eq!(HEADER, 24);
}
