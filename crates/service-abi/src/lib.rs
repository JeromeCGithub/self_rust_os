//! Application protocol over authenticated native endpoints; payload IDs are not authority.
#![no_std]
pub use rustos_ipc_abi::ProcessKey as PeerKey;
pub const NOTIFY_TAG: u32 = 1;
pub const LOG_TAG: u32 = 2;
pub const CONTROL_TAG: u32 = 3;
pub const MAX_FRAME: usize = rustos_ipc_abi::MAX_PAYLOAD;
pub const HEADER: usize = 24;
pub const MAX_TEXT: usize = MAX_FRAME - HEADER;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Limit,
    Encoding,
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message<'a> {
    Init {
        run: u64,
        notify: bool,
        watchdog: Option<u64>,
    },
    Ready {
        run: u64,
    },
    Heartbeat {
        run: u64,
    },
    Stop {
        run: u64,
    },
    Log {
        run: u64,
        level: Level,
        text: &'a str,
    },
}
impl<'a> Message<'a> {
    pub fn encode(self, output: &mut [u8; MAX_FRAME]) -> Result<usize, Error> {
        let (run, tag, length, detail): (u64, u32, usize, u32) = match self {
            Self::Init {
                run,
                notify,
                watchdog,
            } => {
                if watchdog.is_some_and(|n| n == 0 || n > i64::MAX as u64)
                    || watchdog.is_some() && !notify
                {
                    return Err(Error::Invalid);
                }
                (run, 1, 8, u32::from(notify))
            }
            Self::Ready { run } => (run, 2, 0, 0),
            Self::Heartbeat { run } => (run, 3, 0, 0),
            Self::Stop { run } => (run, 5, 0, 0),
            Self::Log { run, level, text } => {
                if text.len() > MAX_TEXT {
                    return Err(Error::Limit);
                }
                if text.chars().any(char::is_control) {
                    return Err(Error::Encoding);
                }
                (
                    run,
                    4,
                    text.len(),
                    match level {
                        Level::Debug => 1,
                        Level::Info => 2,
                        Level::Warning => 3,
                        Level::Error => 4,
                    },
                )
            }
        };
        if run == 0 {
            return Err(Error::Invalid);
        }
        output.fill(0);
        output[..4].copy_from_slice(&1u32.to_le_bytes());
        output[4..8].copy_from_slice(&tag.to_le_bytes());
        output[8..16].copy_from_slice(&run.to_le_bytes());
        output[16..20].copy_from_slice(&(length as u32).to_le_bytes());
        output[20..24].copy_from_slice(&detail.to_le_bytes());
        match self {
            Self::Init { watchdog, .. } => {
                output[HEADER..HEADER + 8].copy_from_slice(&watchdog.unwrap_or(0).to_le_bytes())
            }
            Self::Log { text, .. } => {
                output[HEADER..HEADER + length].copy_from_slice(text.as_bytes())
            }
            _ => {}
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
        let run = word64(bytes, 8)?;
        let length = word32(bytes, 16)? as usize;
        let detail = word32(bytes, 20)?;
        if run == 0 || length > MAX_TEXT || bytes.len() != HEADER + length {
            return Err(Error::Invalid);
        }
        match word32(bytes, 4)? {
            1 if length == 8 && detail <= 1 => {
                let value = word64(bytes, HEADER)?;
                if value > i64::MAX as u64 || value != 0 && detail == 0 {
                    return Err(Error::Invalid);
                }
                Ok(Self::Init {
                    run,
                    notify: detail == 1,
                    watchdog: if value == 0 { None } else { Some(value) },
                })
            }
            2 if length == 0 && detail == 0 => Ok(Self::Ready { run }),
            3 if length == 0 && detail == 0 => Ok(Self::Heartbeat { run }),
            5 if length == 0 && detail == 0 => Ok(Self::Stop { run }),
            4 => {
                let level = match detail {
                    1 => Level::Debug,
                    2 => Level::Info,
                    3 => Level::Warning,
                    4 => Level::Error,
                    _ => return Err(Error::Invalid),
                };
                let text = core::str::from_utf8(&bytes[HEADER..]).map_err(|_| Error::Encoding)?;
                if text.chars().any(char::is_control) {
                    return Err(Error::Encoding);
                }
                Ok(Self::Log { run, level, text })
            }
            _ => Err(Error::Invalid),
        }
    }
}
fn word32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::Invalid)?
            .try_into()
            .map_err(|_| Error::Invalid)?,
    ))
}
fn word64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(Error::Invalid)?
            .try_into()
            .map_err(|_| Error::Invalid)?,
    ))
}

/// Transport-independent client handshake state, shared by native and host clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientState {
    run: Option<u64>,
    manager: Option<rustos_ipc_abi::ProcessKey>,
    notify: bool,
    watchdog: Option<u64>,
    ready: bool,
    stopping: bool,
}
impl Default for ClientState {
    fn default() -> Self {
        Self::new()
    }
}
impl ClientState {
    pub const fn new() -> Self {
        Self {
            run: None,
            manager: None,
            notify: false,
            watchdog: None,
            ready: false,
            stopping: false,
        }
    }
    /// The transport supplies its authenticated peer identity, never a payload claim.
    pub fn accept_init(
        &mut self,
        sender: rustos_ipc_abi::ProcessKey,
        message: Message<'_>,
    ) -> Result<(), Error> {
        if self.run.is_some() || rustos_ipc_abi::ProcessKey::decode(sender.encode()) != Some(sender)
        {
            return Err(Error::Invalid);
        }
        let Message::Init {
            run,
            notify,
            watchdog,
        } = message
        else {
            return Err(Error::Invalid);
        };
        if run == 0
            || watchdog.is_some_and(|n| n == 0 || n > i64::MAX as u64)
            || watchdog.is_some() && !notify
        {
            return Err(Error::Invalid);
        }
        self.run = Some(run);
        self.manager = Some(sender);
        self.notify = notify;
        self.watchdog = watchdog;
        Ok(())
    }
    /// Accept stop only from the Init-authenticated manager for this exact run.
    /// Repetition is idempotent; cleanup logging remains available afterward.
    pub fn accept_stop(&mut self, sender: PeerKey, message: Message<'_>) -> Result<(), Error> {
        let Message::Stop { run } = message else {
            return Err(Error::Invalid);
        };
        if self.manager != Some(sender) || self.run != Some(run) {
            return Err(Error::Invalid);
        }
        self.stopping = true;
        Ok(())
    }
    pub fn stop_requested(&self) -> bool {
        self.stopping
    }
    pub fn run(&self) -> Option<u64> {
        self.run
    }
    pub fn manager(&self) -> Option<rustos_ipc_abi::ProcessKey> {
        self.manager
    }
    pub fn watchdog(&self) -> Option<u64> {
        self.watchdog
    }
    /// Prepare without changing state; failed sends must remain retryable.
    pub fn ready_frame(&self) -> Result<Option<Message<'static>>, Error> {
        let run = self.run.ok_or(Error::Invalid)?;
        if self.ready || self.stopping {
            return Err(Error::Invalid);
        }
        Ok(if self.notify {
            Some(Message::Ready { run })
        } else {
            None
        })
    }
    pub fn ready_sent(&mut self) -> Result<(), Error> {
        self.ready_frame()?;
        self.ready = true;
        Ok(())
    }
    pub fn heartbeat_frame(&self) -> Result<Message<'static>, Error> {
        let run = self.run.ok_or(Error::Invalid)?;
        if !self.ready || self.watchdog.is_none() || self.stopping {
            return Err(Error::Invalid);
        }
        Ok(Message::Heartbeat { run })
    }
    pub fn log_frame<'a>(&self, level: Level, text: &'a str) -> Result<Message<'a>, Error> {
        Ok(Message::Log {
            run: self.run.ok_or(Error::Invalid)?,
            level,
            text,
        })
    }
}

pub mod control;
pub mod replies;
