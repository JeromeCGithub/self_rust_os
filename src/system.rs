//! Kernel ownership and validated copies for randomness and UTC platform syscalls.
use rustos_platform_abi::{self as abi, Error};
/// Dispatch validated platform requests from the existing syscall register ABI.
pub fn syscall(number: u64, first: u64, second: u64, third: u64) -> u64 {
    match dispatch(number, first, second, third) {
        Ok(value) => value,
        Err(error) => error.encode(),
    }
}
fn dispatch(number: u64, first: u64, second: u64, third: u64) -> Result<u64, Error> {
    crate::scheduler::current_pid().ok_or(Error::Invalid)?;
    if third != 0 {
        return Err(Error::Invalid);
    }
    match number {
        abi::SYS_RANDOM => {
            if second > abi::MAX_RANDOM_BYTES as u64 {
                return Err(Error::Invalid);
            }
            if second == 0 {
                return Ok(0);
            }
            if !crate::userspace::usercopy::validate(first, second, true) {
                return Err(Error::Invalid);
            }
            let mut bytes = [0; abi::MAX_RANDOM_BYTES];
            crate::drivers::char::random::fill(&mut bytes[..second as usize])?;
            // SAFETY: Every page of the full output was checked writable/user.
            // The initialized, distinct local record is copied without a user
            // reference. Syscall execution cannot unmap/schedule on this CPU.
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), first as *mut u8, second as usize);
            }
            Ok(second)
        }
        abi::SYS_UNIX_TIME => {
            if first != 0 || second != 0 {
                return Err(Error::Invalid);
            }
            crate::drivers::rtc::cmos::now().ok_or(Error::Unavailable)
        }
        _ => Err(Error::Invalid),
    }
}
#[cfg(test)]
mod tests {
    #[test_case]
    fn rejects_requests_without_a_user_process() {
        for n in [27, 28] {
            assert_eq!(
                super::syscall(n, 0, 0, 0),
                rustos_platform_abi::Error::Invalid.encode()
            );
        }
    }
}
