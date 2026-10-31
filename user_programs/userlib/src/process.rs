//! Owned child process with a bounded, private standard-I/O connection.
//! Input packets contain one byte; output packets contain up to three bytes.
use crate::sys::{self, MessageError, SYSCALL_ERROR};

fn operation(op: u64, value: u32) -> u64 {
    // SAFETY: This register-only syscall has no pointers or borrowed buffers.
    unsafe { sys::syscall(sys::SYS_STDIO, op, u64::from(value), 0) }
}
pub(crate) fn redirected() -> bool {
    use core::sync::atomic::{AtomicU8, Ordering};
    static ROLE: AtomicU8 = AtomicU8::new(0);
    let mut role = ROLE.load(Ordering::Relaxed);
    if role == 0 {
        let info = operation(0, 0);
        role = if info != SYSCALL_ERROR && info & 1 != 0 {
            2
        } else {
            1
        };
        // The child role is assigned before its first instruction and cannot
        // change while it runs. Host attachment never changes the host role.
        // Relaxed stores publish only this immutable role value, with no other data.
        ROLE.store(role, Ordering::Relaxed);
    }
    role == 2
}
fn send(word: u32) -> Result<(), MessageError> {
    match operation(1, word) {
        0 => Ok(()),
        1 => Err(MessageError::Full),
        _ => Err(MessageError::Unavailable),
    }
}
fn receive() -> Result<Option<u32>, MessageError> {
    match operation(2, 0) {
        0 => Ok(None),
        SYSCALL_ERROR => Err(MessageError::Unavailable),
        value => Ok(Some((value - 1) as u32)),
    }
}

/// A terminal owns one child endpoint. Dropping it kills and reaps the child.
/// This is a bounded transport, not a POSIX PTY or file-descriptor API.
pub struct Child {
    pid: u32,
}
impl Child {
    /// Spawn an executable and connect its standard I/O before it first runs.
    pub fn spawn(path: &str) -> Result<Self, ()> {
        let path = sys::path_buf(path).ok_or(())?;
        // SAFETY: The synchronous call reads this NUL-terminated stack buffer;
        // it is valid for its complete length and no reference is retained.
        let pid = unsafe { sys::syscall(sys::SYS_SPAWN, path.as_ptr() as u64, 1, 0) };
        if pid == SYSCALL_ERROR {
            Err(())
        } else {
            Ok(Self { pid: pid as u32 })
        }
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
    /// Forward one input byte. Full means retry this byte after draining output.
    pub fn write_byte(&mut self, byte: u8) -> Result<(), MessageError> {
        send((1 << 24) | u32::from(byte))
    }
    /// Drain one output packet. None means live but empty; unavailable means EOF.
    pub fn read(&mut self) -> Result<Option<([u8; 3], usize)>, MessageError> {
        let Some(word) = receive()? else {
            return Ok(None);
        };
        let count = (word >> 24) as usize;
        if !(1..=3).contains(&count) {
            return Err(MessageError::Unavailable);
        }
        Ok(Some((
            [word as u8, (word >> 8) as u8, (word >> 16) as u8],
            count,
        )))
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = sys::kill(self.pid);
        let _ = sys::waitpid(self.pid);
    }
}

pub(crate) fn write(bytes: &[u8]) -> usize {
    let mut written = 0;
    for chunk in bytes.chunks(3) {
        let mut word = (chunk.len() as u32) << 24;
        for (i, b) in chunk.iter().enumerate() {
            word |= u32::from(*b) << (i * 8);
        }
        loop {
            match send(word) {
                Ok(()) => break,
                Err(MessageError::Full) => {
                    let _ = sys::sleep_ticks(1);
                }
                Err(_) => return written,
            }
        }
        written += chunk.len();
    }
    written
}
pub(crate) fn read(bytes: &mut [u8], blocking: bool) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    loop {
        match receive() {
            Ok(Some(word)) if word >> 24 == 1 => {
                bytes[0] = word as u8;
                return 1;
            }
            Ok(Some(_)) => return 0,
            Ok(None) if blocking => {
                let _ = sys::sleep_ticks(1);
            }
            Ok(None) => return 0,
            Err(_) => sys::exit(0),
        }
    }
}
