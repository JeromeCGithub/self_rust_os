//! Bounded completions observed before an asynchronous launch publishes its key.
use crate::{Completion, ProcessKey};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EarlyExit {
    pub process: ProcessKey,
    pub cause: Completion,
    pub tick: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Full,
    Invalid,
    Conflict,
}
pub struct EarlyExits<const N: usize> {
    slots: [Option<EarlyExit>; N],
}
impl<const N: usize> EarlyExits<N> {
    pub const fn new() -> Self {
        Self { slots: [None; N] }
    }
    pub fn insert(&mut self, record: EarlyExit) -> Result<(), Error> {
        if record.process.generation == 0 || record.process.pid == u32::MAX {
            return Err(Error::Invalid);
        }
        if let Some(existing) = self
            .slots
            .iter()
            .flatten()
            .find(|e| e.process == record.process)
        {
            return if *existing == record {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        *self
            .slots
            .iter_mut()
            .find(|s| s.is_none())
            .ok_or(Error::Full)? = Some(record);
        Ok(())
    }
    pub fn take(&mut self, process: ProcessKey) -> Option<EarlyExit> {
        self.slots
            .iter_mut()
            .find(|s| s.is_some_and(|e| e.process == process))
            .and_then(Option::take)
    }
    pub fn full(&self) -> bool {
        self.slots.iter().all(Option::is_some)
    }
    pub fn clear(&mut self) {
        self.slots.fill(None);
    }
}

impl<const N: usize> Default for EarlyExits<N> {
    fn default() -> Self {
        Self::new()
    }
}
