//! Typed bounded control replies. Text borrows the caller's current snapshot.
use crate::{
    control::{valid_name, Access},
    word32, word64, Error, Level, HEADER, MAX_FRAME, MAX_TEXT,
};
use rustos_ipc_abi::ProcessKey;
pub const LOG_CHUNK: usize = 184;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Phase {
    Inactive = 1,
    Spawning,
    Starting,
    Running,
    Stopping,
    Backoff,
    Failed,
    GroupActive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum JobPhase {
    Running = 1,
    Succeeded,
    Degraded,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Code {
    Denied = 1,
    Invalid,
    Busy,
    Unknown,
    Gap,
    Stale,
    Unsupported,
    Limit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body<'a> {
    Hello {
        access: Access,
        run: u64,
    },
    Job {
        job: u64,
        phase: JobPhase,
    },
    Status {
        index: u32,
        name: &'a str,
        phase: Phase,
        run: u64,
        process: Option<ProcessKey>,
        deadline: Option<u64>,
    },
    Log {
        sequence: u64,
        unit: u32,
        run: u64,
        process: ProcessKey,
        tick: u64,
        level: Level,
        offset: u16,
        total: u16,
        text: &'a str,
    },
    End,
    Error {
        code: Code,
        detail: u64,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply<'a> {
    pub id: u64,
    pub body: Body<'a>,
}
impl<'a> Reply<'a> {
    pub fn encode(self, out: &mut [u8; MAX_FRAME]) -> Result<usize, Error> {
        self.validate()?;
        out.fill(0);
        let body = &mut out[HEADER..];
        let (tag, length) = match self.body {
            Body::Hello { access, run } => {
                put32(body, 0, if access == Access::ReadOnly { 1 } else { 2 });
                put64(body, 8, run);
                (32, 16)
            }
            Body::Job { job, phase } => {
                put64(body, 0, job);
                put32(body, 8, phase as u32);
                (33, 16)
            }
            Body::Status {
                index,
                name,
                phase,
                run,
                process,
                deadline,
            } => {
                put32(body, 0, index);
                put32(body, 4, phase as u32);
                put64(body, 8, run);
                put64(body, 16, process.map(|p| p.encode()).unwrap_or(0));
                put64(body, 24, deadline.unwrap_or(0));
                body[32..32 + name.len()].copy_from_slice(name.as_bytes());
                (34, 32 + name.len())
            }
            Body::Log {
                sequence,
                unit,
                run,
                process,
                tick,
                level,
                offset,
                total,
                text,
            } => {
                put64(body, 0, sequence);
                put32(body, 8, unit);
                put32(
                    body,
                    12,
                    match level {
                        Level::Debug => 1,
                        Level::Info => 2,
                        Level::Warning => 3,
                        Level::Error => 4,
                    },
                );
                put64(body, 16, run);
                put64(body, 24, process.encode());
                put64(body, 32, tick);
                body[40..42].copy_from_slice(&offset.to_le_bytes());
                body[42..44].copy_from_slice(&total.to_le_bytes());
                body[48..48 + text.len()].copy_from_slice(text.as_bytes());
                (35, 48 + text.len())
            }
            Body::End => (36, 0),
            Body::Error { code, detail } => {
                put32(body, 0, code as u32);
                put64(body, 8, detail);
                (37, 16)
            }
        };
        put32(out, 0, 1);
        put32(out, 4, tag);
        put64(out, 8, self.id);
        put32(out, 16, length as u32);
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
        let b = &bytes[HEADER..];
        let body = match word32(bytes, 4)? {
            32 if length == 16 && word32(b, 4)? == 0 => Body::Hello {
                access: match word32(b, 0)? {
                    1 => Access::ReadOnly,
                    2 => Access::Manage,
                    _ => return Err(Error::Invalid),
                },
                run: word64(b, 8)?,
            },
            33 if length == 16 && word32(b, 12)? == 0 => Body::Job {
                job: word64(b, 0)?,
                phase: JobPhase::try_from(word32(b, 8)?)?,
            },
            34 if length >= 32 => {
                let packed = word64(b, 16)?;
                let process = if packed == 0 {
                    None
                } else {
                    Some(ProcessKey::decode(packed).ok_or(Error::Invalid)?)
                };
                let deadline = match word64(b, 24)? {
                    0 => None,
                    value => Some(value),
                };
                Body::Status {
                    index: word32(b, 0)?,
                    phase: Phase::try_from(word32(b, 4)?)?,
                    run: word64(b, 8)?,
                    process,
                    deadline,
                    name: core::str::from_utf8(&b[32..]).map_err(|_| Error::Encoding)?,
                }
            }
            35 if length >= 48 && word32(b, 44)? == 0 => Body::Log {
                sequence: word64(b, 0)?,
                unit: word32(b, 8)?,
                run: word64(b, 16)?,
                process: ProcessKey::decode(word64(b, 24)?).ok_or(Error::Invalid)?,
                tick: word64(b, 32)?,
                level: match word32(b, 12)? {
                    1 => Level::Debug,
                    2 => Level::Info,
                    3 => Level::Warning,
                    4 => Level::Error,
                    _ => return Err(Error::Invalid),
                },
                offset: u16::from_le_bytes([b[40], b[41]]),
                total: u16::from_le_bytes([b[42], b[43]]),
                text: core::str::from_utf8(&b[48..]).map_err(|_| Error::Encoding)?,
            },
            36 if length == 0 => Body::End,
            37 if length == 16 && word32(b, 4)? == 0 => Body::Error {
                code: Code::try_from(word32(b, 0)?)?,
                detail: word64(b, 8)?,
            },
            _ => return Err(Error::Invalid),
        };
        let reply = Self {
            id: word64(bytes, 8)?,
            body,
        };
        reply.validate()?;
        Ok(reply)
    }
    fn validate(&self) -> Result<(), Error> {
        if matches!(self.body, Body::Hello { .. }) != (self.id == 0) {
            return Err(Error::Invalid);
        }
        match self.body {
            Body::Hello { run, .. } if run == 0 => return Err(Error::Invalid),
            Body::Job { job, .. } if job == 0 => return Err(Error::Invalid),
            Body::Status {
                index,
                name,
                run,
                process,
                deadline,
                ..
            } => {
                if index >= 64
                    || !valid_name(name)
                    || deadline == Some(0)
                    || process.is_some() && run == 0
                    || process.is_some_and(|p| ProcessKey::decode(p.encode()) != Some(p))
                {
                    return Err(Error::Invalid);
                }
            }
            Body::Log {
                sequence,
                unit,
                run,
                process,
                offset,
                total,
                text,
                ..
            } => {
                if sequence == 0
                    || unit >= 64
                    || run == 0
                    || ProcessKey::decode(process.encode()) != Some(process)
                    || total as usize > MAX_TEXT
                    || text.len() > LOG_CHUNK
                    || offset as usize + text.len() > total as usize
                    || text.is_empty() && total != 0
                    || text.chars().any(char::is_control)
                {
                    return Err(Error::Invalid);
                }
            }
            Body::Error { code, detail } if (code == Code::Gap) != (detail != 0) => {
                return Err(Error::Invalid)
            }
            _ => {}
        }
        Ok(())
    }
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
macro_rules! wire_enum {
    ($name:ident,$($variant:ident),+) => {
        impl TryFrom<u32> for $name {
            type Error=Error;
            fn try_from(value:u32)->Result<Self,Error>{
                match value { $(x if x==Self::$variant as u32=>Ok(Self::$variant),)+_=>Err(Error::Invalid) }
            }
        }
    }
}
wire_enum!(
    Phase,
    Inactive,
    Spawning,
    Starting,
    Running,
    Stopping,
    Backoff,
    Failed,
    GroupActive
);
wire_enum!(JobPhase, Running, Succeeded, Degraded, Failed);
wire_enum!(
    Code,
    Denied,
    Invalid,
    Busy,
    Unknown,
    Gap,
    Stale,
    Unsupported,
    Limit
);
