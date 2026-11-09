//! Owned native channel endpoints. No process inherits endpoints implicitly.
use crate::sys;
pub use abi::{Binding as WireBinding, Event, Handle, Kind, ProcessKey, Rights};
use alloc::vec::Vec;
pub use rustos_ipc_abi as abi;
/// Nonblocking send result; full queues require writable interest or dropping logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    Full,
    Closed,
    Invalid,
}
/// One process-owned endpoint. Drop closes it; successful spawn moves invalidate
/// the source wrapper. Endpoint ownership is never implicitly cloned.
pub struct Endpoint {
    handle: Option<Handle>,
    readable: bool,
}
impl Endpoint {
    pub fn pair() -> Result<[Self; 2], ()> {
        let mut bytes = [0; 16];
        // SAFETY: The initialized, uniquely borrowed stack output is writable
        // for exactly 16 bytes through this synchronous operation; reserved is zero.
        let result = unsafe { sys::syscall(abi::SYS_PAIR, bytes.as_mut_ptr() as u64, 16, 0) };
        if result != 0 {
            return Err(());
        }
        let a = Handle(u64::from_le_bytes(bytes[..8].try_into().map_err(|_| ())?));
        let b = Handle(u64::from_le_bytes(bytes[8..].try_into().map_err(|_| ())?));
        if !a.valid() || !b.valid() || a == b {
            return Err(());
        }
        Ok([
            Self {
                handle: Some(a),
                readable: true,
            },
            Self {
                handle: Some(b),
                readable: true,
            },
        ])
    }
    /// Take one explicit launch grant. A tag can be taken only once.
    pub fn bootstrap(tag: u32) -> Result<Self, ()> {
        // SAFETY: Only scalar bootstrap tag and zero reserved arguments cross.
        let handle = Handle(unsafe { sys::syscall(abi::SYS_BOOTSTRAP, u64::from(tag), 0, 0) });
        if !handle.valid() {
            return Err(());
        }
        Ok(Self {
            handle: Some(handle),
            readable: true,
        })
    }
    /// Borrow the numeric identity for matching events; this does not clone ownership.
    pub fn raw(&self) -> Option<Handle> {
        self.handle
    }
    pub fn send(&self, bytes: &[u8]) -> Result<(), SendError> {
        let handle = self.handle.ok_or(SendError::Invalid)?;
        if bytes.len() > abi::MAX_PAYLOAD {
            return Err(SendError::Invalid);
        }
        // SAFETY: The full bounded slice is readable for this call. The kernel
        // copies before returning and retains no user pointer or Rust reference.
        match unsafe {
            sys::syscall(
                abi::SYS_SEND,
                handle.0,
                bytes.as_ptr() as u64,
                bytes.len() as u64,
            )
        } {
            0 => Ok(()),
            abi::SEND_FULL => Err(SendError::Full),
            abi::SEND_CLOSED => Err(SendError::Closed),
            _ => Err(SendError::Invalid),
        }
    }
    /// Enable/pause receive readiness without consuming queued data. Updating
    /// receive interest also cancels any outstanding writable request.
    pub fn set_readable(&mut self, enabled: bool) -> Result<(), ()> {
        let handle = self.handle.ok_or(())?;
        // SAFETY: Only an owned scalar identity, bounded flags and reserved zero.
        let result = unsafe {
            sys::syscall(
                abi::SYS_INTEREST,
                handle.0,
                if enabled { abi::INTEREST_READ } else { 0 },
                0,
            )
        };
        if result != 0 {
            return Err(());
        }
        self.readable = enabled;
        Ok(())
    }
    /// Request one writable event after backpressure; delivery clears interest.
    pub fn request_writable(&self, enabled: bool) -> Result<(), ()> {
        let handle = self.handle.ok_or(())?;
        let flags = (if self.readable { abi::INTEREST_READ } else { 0 })
            | (if enabled { abi::INTEREST_WRITE } else { 0 });
        // SAFETY: Process-local handle, bounded flags and zero reserved scalar only.
        if unsafe { sys::syscall(abi::SYS_INTEREST, handle.0, flags, 0) } == 0 {
            Ok(())
        } else {
            Err(())
        }
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: Only this process-local scalar identity crosses close.
            // A moved wrapper is None and never closes its child's endpoint.
            let _ = unsafe { sys::syscall(abi::SYS_CLOSE, handle.0, 0, 0) };
        }
    }
}
/// One explicit move, borrowed mutably until the launch completes. Failed launches
/// preserve the source wrapper and rights; success invalidates it in the parent.
pub struct Binding<'a> {
    pub endpoint: &'a mut Endpoint,
    pub tag: u32,
    pub rights: Rights,
}
fn records<'a>(
    strings: impl Iterator<Item = &'a str>,
    count: usize,
    budget: &mut usize,
) -> Result<Vec<[u8; 16]>, ()> {
    let mut records = Vec::new();
    records.try_reserve_exact(count).map_err(|_| ())?;
    for string in strings {
        *budget = budget
            .checked_add(string.len() + 1)
            .filter(|&n| n <= rustos_process_abi::MAX_STARTUP_BYTES)
            .ok_or(())?;
        if string.as_bytes().contains(&0) {
            return Err(());
        }
        let mut record = [0; 16];
        record[..8].copy_from_slice(&(string.as_ptr() as u64).to_le_bytes());
        record[8..].copy_from_slice(&(string.len() as u64).to_le_bytes());
        records.push(record);
    }
    Ok(records)
}
/// Spawn one explicit executable with v2 endpoint bindings and explicit environment.
/// Connected standard I/O remains a separate v1 interface; no endpoint is inherited.
pub fn spawn(
    path: &str,
    arguments: &[&str],
    environment: &[&str],
    bindings: &mut [Binding<'_>],
) -> Result<u32, ()> {
    if arguments.len() >= rustos_process_abi::MAX_VECTOR
        || environment.len() > rustos_process_abi::MAX_VECTOR
        || bindings.len() > abi::MAX_BINDINGS
    {
        return Err(());
    }
    let mut budget = 0;
    let argv = records(
        core::iter::once(path).chain(arguments.iter().copied()),
        arguments.len() + 1,
        &mut budget,
    )?;
    let envp = records(environment.iter().copied(), environment.len(), &mut budget)?;
    let mut grants = Vec::new();
    grants.try_reserve_exact(bindings.len()).map_err(|_| ())?;
    for binding in bindings.iter() {
        grants.push(
            WireBinding {
                source: binding.endpoint.handle.ok_or(())?,
                tag: binding.tag,
                rights: binding.rights,
            }
            .encode()
            .map_err(|_| ())?,
        );
    }
    let request = abi::Spawn {
        argv: argv.as_ptr() as u64,
        argc: argv.len() as u64,
        envp: if envp.is_empty() {
            0
        } else {
            envp.as_ptr() as u64
        },
        envc: envp.len() as u64,
        bindings: if grants.is_empty() {
            0
        } else {
            grants.as_ptr() as u64
        },
        binding_count: grants.len() as u64,
    }
    .encode()
    .map_err(|_| ())?;
    let path = sys::path_buf(path).ok_or(())?;
    // SAFETY: The NUL path, exact initialized request, contiguous padding-free
    // string/binding records and every referenced immutable string remain live
    // throughout this synchronous call. Kernel copies all records/strings and
    // revalidates ownership before publication; no user reference is retained.
    let result = unsafe {
        sys::syscall(
            abi::SYS_SPAWN_BOUND,
            path.as_ptr() as u64,
            request.as_ptr() as u64,
            abi::SPAWN_SIZE as u64,
        )
    };
    let pid = u32::try_from(result).map_err(|_| ())?;
    for binding in bindings.iter_mut() {
        binding.endpoint.handle = None;
    }
    Ok(pid)
}
/// Consume at most one message, closure or requested writable event for this process.
pub fn next() -> Result<Option<Event>, ()> {
    let mut bytes = [0; abi::EVENT_SIZE];
    // SAFETY: The complete initialized stack buffer is uniquely writable through
    // this nonblocking call. Copy failure preserves the kernel's pending event.
    match unsafe {
        sys::syscall(
            abi::SYS_NEXT,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
    } {
        0 => Ok(None),
        1 => Event::decode(&bytes).map(Some).map_err(|_| ()),
        _ => Err(()),
    }
}
