//! Read-only caller principal snapshots. Scope factories are separate authority.
use super::{ProcessState, ERROR, SCHEDULER};
use rustos_process_abi::principal as abi;
/// Copy a complete canonical record only after validating the full writable range.
/// Kernel entry stabilizes mappings. No process reference survives the lock/copy.
pub fn principal_user(pointer: u64, size: u64, reserved: u64) -> u64 {
    if size != abi::SIZE as u64
        || reserved != 0
        || !crate::userspace::usercopy::validate(pointer, size, true)
    {
        return ERROR;
    }
    let bytes = {
        let scheduler = SCHEDULER.lock();
        let Some(process) = scheduler
            .current_pid()
            .and_then(|pid| scheduler.processes.get(pid as usize))
            .and_then(Option::as_ref)
            .filter(|process| process.state == ProcessState::Live)
        else {
            return ERROR;
        };
        let Ok(bytes) = process.principal.encode() else {
            return ERROR;
        };
        bytes
    };
    if !crate::userspace::usercopy::to_user(pointer, &bytes) {
        return ERROR;
    }
    0
}
