//! Typed hardware randomness, UTC time and privileged terminal system actions.
use crate::sys::syscall;
use rustos_platform_abi as abi;
pub use rustos_platform_abi::{Error, SystemAction};
/// Fill bytes from hardware RNG. Missing/exhausted entropy is an explicit error.
pub fn random_bytes(output: &mut [u8]) -> Result<(), Error> {
    for chunk in output.chunks_mut(abi::MAX_RANDOM_BYTES) {
        // SAFETY: Exclusive initialized output lives through each call; the kernel
        // validates every page and retains no user pointer or reference.
        abi::decode(unsafe {
            syscall(
                abi::SYS_RANDOM,
                chunk.as_mut_ptr() as u64,
                chunk.len() as u64,
                0,
            )
        })?;
    }
    Ok(())
}
/// UTC Unix seconds from a validated stable RTC sample; never a guessed date.
pub fn unix_time() -> Result<u64, Error> {
    // SAFETY: Scalar-only query; all reserved arguments are zero.
    abi::decode(unsafe { syscall(abi::SYS_UNIX_TIME, 0, 0, 0) })
}

/// Request a privileged terminal system action. Successful halt never returns;
/// reboot/poweroff are explicitly unsupported. Errors leave the caller running.
pub fn system_action(action: SystemAction) -> Result<core::convert::Infallible, Error> {
    // SAFETY: The request contains only a typed scalar action and zero reserved
    // words. The kernel validates authority and process/device lifecycle; no
    // pointer, reference, or user buffer is retained by this terminal operation.
    let result = abi::decode(unsafe { syscall(abi::SYS_SYSTEM_ACTION, action as u64, 0, 0) });
    Err(match result {
        Err(error) => error,
        Ok(_) => Error::Failed,
    })
}
