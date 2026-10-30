//! Platform services shared by kernel and Ring 3: randomness and UTC time.
#![no_std]
/// Fill a validated output buffer with hardware random bytes.
pub const SYS_RANDOM: u64 = 27;
/// Return validated RTC UTC seconds since the Unix epoch.
pub const SYS_UNIX_TIME: u64 = 28;
/// Bound the work and temporary storage of a single randomness syscall.
pub const MAX_RANDOM_BYTES: usize = 256;
/// Platform failures occupy a range distinct from socket and scheduler returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    /// Malformed arguments or an inaccessible output mapping.
    Invalid = 1,
    /// Hardware entropy or a valid RTC sample is unavailable.
    Unavailable = 2,
    /// Hardware failed within its retry bound.
    Failed = 3,
}
const BASE: u64 = u64::MAX - 511;
impl Error {
    /// Encode an error below the syscall control and networking error ranges.
    pub const fn encode(self) -> u64 {
        BASE + self as u64
    }
}
/// Decode a platform syscall result into a scalar or typed failure.
pub fn decode(value: u64) -> Result<u64, Error> {
    match value.checked_sub(BASE) {
        None => Ok(value),
        Some(1) => Err(Error::Invalid),
        Some(2) => Err(Error::Unavailable),
        _ => Err(Error::Failed),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn errors_do_not_alias_control_or_network_returns() {
        for error in [
            super::Error::Invalid,
            super::Error::Unavailable,
            super::Error::Failed,
        ] {
            assert_eq!(super::decode(error.encode()), Err(error));
            assert!(error.encode() < u64::MAX - 255);
        }
        assert_eq!(super::decode(1800000000), Ok(1800000000));
    }
}
