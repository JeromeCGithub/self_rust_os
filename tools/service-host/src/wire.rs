//! Length-delimited application payloads over independent nonblocking streams.
use rustos_service_abi::{Message, MAX_FRAME};
use std::io::{self, Read, Write};
pub struct Frame {
    pub bytes: [u8; MAX_FRAME],
    pub length: usize,
    pub sent: usize,
}
impl Frame {
    pub fn message(message: Message<'_>) -> io::Result<Self> {
        let mut bytes = [0; MAX_FRAME];
        let length = message
            .encode(&mut bytes)
            .map_err(|_| io::Error::other("invalid application frame"))?;
        Ok(Self {
            bytes,
            length,
            sent: 0,
        })
    }
    pub fn write(&mut self, stream: &mut impl Write) -> io::Result<bool> {
        let mut wire = [0; MAX_FRAME + 4];
        wire[..4].copy_from_slice(&(self.length as u32).to_le_bytes());
        wire[4..4 + self.length].copy_from_slice(&self.bytes[..self.length]);
        while self.sent < self.length + 4 {
            match stream.write(&wire[self.sent..self.length + 4]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}
pub struct Reader {
    wire: [u8; MAX_FRAME + 4],
    filled: usize,
    target: usize,
    pub closed: bool,
}
impl Reader {
    pub fn new() -> Self {
        Self {
            wire: [0; MAX_FRAME + 4],
            filled: 0,
            target: 4,
            closed: false,
        }
    }
    pub fn next(&mut self, stream: &mut impl Read) -> io::Result<Option<Frame>> {
        if self.closed {
            return Ok(None);
        }
        loop {
            if self.filled == self.target {
                if self.target == 4 {
                    let length =
                        u32::from_le_bytes(self.wire[..4].try_into().expect("four-byte length"))
                            as usize;
                    if length == 0 || length > MAX_FRAME {
                        return Err(io::Error::other("frame length limit"));
                    }
                    self.target = 4 + length;
                } else {
                    let length = self.target - 4;
                    let mut bytes = [0; MAX_FRAME];
                    bytes[..length].copy_from_slice(&self.wire[4..self.target]);
                    self.filled = 0;
                    self.target = 4;
                    return Ok(Some(Frame {
                        bytes,
                        length,
                        sent: 0,
                    }));
                }
            }
            match stream.read(&mut self.wire[self.filled..self.target]) {
                Ok(0) => {
                    self.closed = true;
                    if self.filled != 0 {
                        return Err(io::Error::other("truncated frame"));
                    }
                    return Ok(None);
                }
                Ok(n) => self.filled += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}
