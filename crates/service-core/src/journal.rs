//! Bounded volatile records. Callers authenticate endpoint/unit/run attribution.
use crate::ProcessKey;
pub use rustos_service_abi::Level;
pub const MAX_TEXT: usize = rustos_service_abi::MAX_TEXT;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Limit,
    Sequence,
    Gap { oldest: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub sequence: u64,
    pub unit: usize,
    pub run: u64,
    pub process: ProcessKey,
    pub tick: u64,
    pub level: Level,
    bytes: [u8; MAX_TEXT],
    length: usize,
}
impl Record {
    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length])
            .expect("journal copies complete validated UTF-8")
    }
}
pub struct Journal<const N: usize> {
    records: [Option<Record>; N],
    head: usize,
    length: usize,
    next: u64,
    evicted: u64,
}
impl<const N: usize> Default for Journal<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Journal<N> {
    pub const fn new() -> Self {
        Self {
            records: [None; N],
            head: 0,
            length: 0,
            next: 1,
            evicted: 0,
        }
    }
    /// Authenticate attribution at the transport boundary before calling this.
    /// A record copies complete text and performs no allocation or output I/O.
    pub fn push(
        &mut self,
        unit: usize,
        run: u64,
        process: ProcessKey,
        tick: u64,
        level: Level,
        text: &str,
    ) -> Result<u64, Error> {
        if N == 0 || text.len() > MAX_TEXT {
            return Err(Error::Limit);
        }
        if unit >= crate::config::MAX_UNITS
            || run == 0
            || process.generation == 0
            || process.pid == u32::MAX
            || text.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        let next = self.next.checked_add(1).ok_or(Error::Sequence)?;
        let mut bytes = [0; MAX_TEXT];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        let position = if self.length == N {
            let old = self.head;
            self.head = (self.head + 1) % N;
            self.evicted += 1;
            old
        } else {
            let next = (self.head + self.length) % N;
            self.length += 1;
            next
        };
        let sequence = self.next;
        self.records[position] = Some(Record {
            sequence,
            unit,
            run,
            process,
            tick,
            level,
            bytes,
            length: text.len(),
        });
        self.next = next;
        Ok(sequence)
    }
    /// Cursor zero begins at the oldest retained record. A stale nonzero cursor
    /// reports a gap explicitly; callers must not fabricate missing log history.
    pub fn after(&self, cursor: u64) -> Result<Option<&Record>, Error> {
        if self.length == 0 {
            return Ok(None);
        }
        let oldest = self.records[self.head]
            .as_ref()
            .ok_or(Error::Invalid)?
            .sequence;
        if cursor != 0 && cursor < oldest - 1 {
            return Err(Error::Gap { oldest });
        }
        for offset in 0..self.length {
            let record = self.records[(self.head + offset) % N]
                .as_ref()
                .ok_or(Error::Invalid)?;
            if record.sequence > cursor {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }
    pub fn len(&self) -> usize {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn evicted(&self) -> u64 {
        self.evicted
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intake {
    Accept,
    Pause { until: Option<u64> },
}
/// Per-run input budget. Paused endpoints resume on this deadline, never by polling.
pub struct Budget {
    burst: u16,
    window: u64,
    used: u16,
    initialized: bool,
    reset: Option<u64>,
}
impl Budget {
    pub fn new(burst: u16, window: u64) -> Result<Self, Error> {
        if burst == 0
            || burst as usize > crate::MAX_BURST
            || window == 0
            || window > i64::MAX as u64
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            burst,
            window,
            used: 0,
            initialized: false,
            reset: None,
        })
    }
    pub fn admit(&mut self, now: u64) -> Intake {
        if !self.initialized || self.reset.is_some_and(|reset| now >= reset) {
            self.used = 0;
            self.reset = now.checked_add(self.window);
            self.initialized = true;
        }
        if self.used >= self.burst {
            return Intake::Pause { until: self.reset };
        }
        self.used += 1;
        Intake::Accept
    }
}
