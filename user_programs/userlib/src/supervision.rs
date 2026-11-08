//! Owned child lifecycle wrappers. No Linux process ABI is implied.
use crate::sys;
use rustos_process_abi::supervision as abi;
pub use rustos_process_abi::supervision::{Cause, ChildEvent, ProcessKey};
/// Query this process's complete kernel-issued identity.
pub fn current() -> Result<ProcessKey, ()> {
    // SAFETY: Scalar identity operation with all reserved words zero.
    let value = unsafe { sys::syscall(abi::SYS_SELF, 0, 0, 0) };
    ProcessKey::decode(value).ok_or(())
}
/// Consume one owned child event; None means no event currently pending.
/// A completion reaps the process; an adoption event leaves it alive.
pub fn next_child() -> Result<Option<ChildEvent>, ()> {
    let mut bytes = [0; abi::EVENT_SIZE];
    // SAFETY: Exclusive initialized local output remains writable for this
    // synchronous call. The kernel retains no pointer after return or scheduling.
    match unsafe {
        sys::syscall(
            abi::SYS_CHILD_NEXT,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
    } {
        0 => Ok(None),
        1 => ChildEvent::decode(&bytes).map(Some).ok_or(()),
        _ => Err(()),
    }
}
/// Force-stop a live owned child. Its completion still must be consumed.
pub fn terminate(child: ProcessKey) -> Result<(), ()> {
    // SAFETY: Only the complete scalar identity crosses the boundary. Kernel
    // ownership and generation checks authorize the operation, not the Rust type.
    match unsafe { sys::syscall(abi::SYS_TERMINATE, child.encode(), 0, 0) } {
        0 => Ok(()),
        _ => Err(()),
    }
}
