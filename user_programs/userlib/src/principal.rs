//! Read-only descriptions of kernel-owned native process principals.
pub use rustos_process_abi::principal as abi;
pub use rustos_process_abi::principal::{Capabilities, Principal};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
}
/// Query this process; user-supplied environment/data never selects an identity.
pub fn current() -> Result<Principal, Error> {
    let mut bytes = [0; abi::SIZE];
    // SAFETY: The complete initialized array is exclusively writable and live
    // throughout this scalar ABI call. The kernel validates every mapping and
    // copies its snapshot without retaining a reference or user-selected identity.
    let result = unsafe {
        crate::sys::syscall(
            abi::SYS_PRINCIPAL,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
    };
    if result != 0 {
        return Err(Error::Unavailable);
    }
    Principal::decode(&bytes).map_err(|_| Error::Invalid)
}

/// Irreversibly reduce this process's mechanism rights before creating threads.
/// Only subsets are accepted; a live sibling thread or native PID 1 is refused.
/// Existing children/grants are not revoked. This is not a resource scope or a
/// filesystem path sandbox. Owned IPC endpoints and cleanup remain usable.
pub fn attenuate(capabilities: Capabilities) -> Result<(), Error> {
    // SAFETY: Only a known scalar rights mask and zero reserved words cross the
    // boundary; the kernel derives the caller and validates attenuation atomically.
    match unsafe { crate::sys::syscall(abi::SYS_ATTENUATE, capabilities.bits(), 0, 0) } {
        0 => Ok(()),
        _ => Err(Error::Unavailable),
    }
}
