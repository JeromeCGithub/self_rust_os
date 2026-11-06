//! Synchronous, thread-confined host calls. No JSValue or borrowed native data escapes.
use crate::{Error, Runtime};
use alloc::{boxed::Box, string::String};
use core::ffi::{c_char, c_void};
/// Owned, bounded results copied into engine-owned values before returning to JS.
pub enum Reply {
    Text(String),
    Node(u32),
    Null,
    Unit,
}
/// A host cannot reenter its engine. Arguments are valid only during this call.
pub trait Host {
    fn call(
        &mut self,
        operation: u32,
        node: u32,
        first: &str,
        second: &str,
    ) -> Result<Reply, Error>;
}
/// Runtime drops before the boxed host, so C cannot retain an address past its owner.
pub struct HostRuntime<H: Host> {
    runtime: Runtime,
    host: Box<H>,
}
impl Runtime {
    /// Install a native call gate for a trusted bootstrap, which must capture and
    /// remove __rustosHost before any page code is evaluated.
    pub fn with_host<H: Host>(self, host: H) -> Result<HostRuntime<H>, Error> {
        let mut owner = HostRuntime {
            runtime: self,
            host: Box::new(host),
        };
        // SAFETY: The Box address is stable across moving HostRuntime. Its private
        // fields cannot be split; runtime is dropped before host, on this thread.
        // C retains only this pointer and the monomorphized synchronous callback.
        // No engine invocation overlaps an exposed borrow of host; callbacks cannot
        // access/reenter runtime. A failed installation drops both in the same order.
        let result = unsafe {
            rjs_host(
                owner.runtime.raw.as_ptr(),
                (&mut *owner.host as *mut H).cast(),
                callback::<H>,
            )
        };
        if result != 0 {
            return Err(Error::Memory);
        }
        Ok(owner)
    }
}
impl<H: Host> HostRuntime<H> {
    pub fn execute(&mut self, source: &str) -> Result<(), Error> {
        self.runtime.execute(source)
    }
    pub fn eval(&mut self, source: &str) -> Result<String, Error> {
        self.runtime.eval(source)
    }
    pub fn run_jobs(&mut self, limit: usize) -> Result<usize, Error> {
        self.runtime.run_jobs(limit)
    }
    pub fn host(&self) -> &H {
        &self.host
    }
}

type Callback = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    *const c_char,
    usize,
    *const c_char,
    usize,
    *mut u8,
    usize,
    *mut usize,
    *mut u32,
) -> i32;
extern "C" {
    fn rjs_host(runtime: *mut c_void, host: *mut c_void, callback: Callback) -> i32;
}
unsafe extern "C" fn callback<H: Host>(
    host: *mut c_void,
    operation: u32,
    node: u32,
    first: *const c_char,
    first_len: usize,
    second: *const c_char,
    second_len: usize,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
    result_node: *mut u32,
) -> i32 {
    if first_len > 8192 || second_len > 8192 || capacity != 8192 {
        return -1;
    }
    // SAFETY: Our C gate supplies two complete live JS-owned string buffers of
    // the checked lengths, output[capacity] and aligned scalar out parameters.
    // HostRuntime owns the aligned live Box<H>, uniquely borrowed only while an
    // engine call is active. Conversion finishes before this borrow and H cannot
    // reenter the runtime; C retains none of these argument/output references.
    let (host, first, second, output, written, result_node) = unsafe {
        (
            &mut *host.cast::<H>(),
            core::slice::from_raw_parts(first.cast::<u8>(), first_len),
            core::slice::from_raw_parts(second.cast::<u8>(), second_len),
            core::slice::from_raw_parts_mut(output, capacity),
            &mut *written,
            &mut *result_node,
        )
    };
    let (Ok(first), Ok(second)) = (core::str::from_utf8(first), core::str::from_utf8(second))
    else {
        return -1;
    };
    match host.call(operation, node, first, second) {
        Ok(Reply::Text(text)) if text.len() <= capacity => {
            output[..text.len()].copy_from_slice(text.as_bytes());
            *written = text.len();
            0
        }
        Ok(Reply::Node(node)) => {
            *result_node = node;
            1
        }
        Ok(Reply::Null) => 2,
        Ok(Reply::Unit) => 3,
        _ => -1,
    }
}
