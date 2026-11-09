//! Bounded management frames, carried only over explicitly granted endpoints.
use crate::{word32, word64, Error, HEADER, MAX_FRAME, MAX_TEXT};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    Manage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation<'a> {
    Status { index: u32 },
    Logs { cursor: u64, offset: u16 },
    Start(&'a str),
    Stop(&'a str),
    Restart(&'a str),
    Shutdown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub id: u64,
    pub operation: Operation<'a>,
}
impl<'a> Request<'a> {
    pub fn encode(self, output: &mut [u8; MAX_FRAME]) -> Result<usize, Error> {
        self.validate()?;
        let (tag, length) = match self.operation {
            Operation::Status { .. } => (16u32, 4),
            Operation::Start(name) => (17, name.len()),
            Operation::Stop(name) => (18, name.len()),
            Operation::Restart(name) => (19, name.len()),
            Operation::Logs { .. } => (20, 12),
            Operation::Shutdown => (21, 0),
        };
        output.fill(0);
        output[..4].copy_from_slice(&1u32.to_le_bytes());
        output[4..8].copy_from_slice(&tag.to_le_bytes());
        output[8..16].copy_from_slice(&self.id.to_le_bytes());
        output[16..20].copy_from_slice(&(length as u32).to_le_bytes());
        match self.operation {
            Operation::Status { index } => {
                output[HEADER..HEADER + 4].copy_from_slice(&index.to_le_bytes())
            }
            Operation::Logs { cursor, offset } => {
                output[HEADER..HEADER + 8].copy_from_slice(&cursor.to_le_bytes());
                output[HEADER + 8..HEADER + 10].copy_from_slice(&offset.to_le_bytes());
            }
            Operation::Start(name) | Operation::Stop(name) | Operation::Restart(name) => {
                output[HEADER..HEADER + length].copy_from_slice(name.as_bytes());
            }
            Operation::Shutdown => {}
        }
        Ok(HEADER + length)
    }
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER || bytes.len() > MAX_FRAME {
            return Err(Error::Invalid);
        }
        if word32(bytes, 0)? != 1 {
            return Err(Error::Unsupported);
        }
        let length = word32(bytes, 16)? as usize;
        if length > MAX_TEXT || bytes.len() != HEADER + length || word32(bytes, 20)? != 0 {
            return Err(Error::Invalid);
        }
        let body = &bytes[HEADER..];
        let operation = match word32(bytes, 4)? {
            16 if length == 4 => Operation::Status {
                index: word32(body, 0)?,
            },
            tag @ (17..=19) => {
                let name = core::str::from_utf8(body).map_err(|_| Error::Encoding)?;
                match tag {
                    17 => Operation::Start(name),
                    18 => Operation::Stop(name),
                    _ => Operation::Restart(name),
                }
            }
            20 if length == 12 && body[10..] == [0, 0] => Operation::Logs {
                cursor: word64(body, 0)?,
                offset: u16::from_le_bytes([body[8], body[9]]),
            },
            21 if length == 0 => Operation::Shutdown,
            _ => return Err(Error::Invalid),
        };
        let request = Self {
            id: word64(bytes, 8)?,
            operation,
        };
        request.validate()?;
        Ok(request)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.id == 0 {
            return Err(Error::Invalid);
        }
        match self.operation {
            Operation::Status { index } if index >= 64 => return Err(Error::Limit),
            Operation::Logs { offset, .. } if offset as usize > MAX_TEXT => {
                return Err(Error::Limit)
            }
            Operation::Start(name) | Operation::Stop(name) | Operation::Restart(name) => {
                if !valid_name(name) {
                    return Err(Error::Invalid);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn valid_name(name: &str) -> bool {
    let initial = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !name.is_empty()
        && name.len() <= 48
        && initial(name.as_bytes()[0])
        && name
            .bytes()
            .all(|b| initial(b) || matches!(b, b'-' | b'_' | b'.'))
}
