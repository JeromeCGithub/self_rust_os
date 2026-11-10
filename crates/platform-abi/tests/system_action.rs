use rustos_platform_abi::{self as abi, Error, SystemAction};
#[test]
fn final_action_accepts_only_halt_and_zero_reserved_words() {
    assert_eq!(abi::SYS_SYSTEM_ACTION, 128);
    assert_eq!(abi::validate_system_action(1, 0, 0), Ok(SystemAction::Halt));
    for action in [2, 3, 4, u64::MAX] {
        assert_eq!(
            abi::validate_system_action(action, 0, 0),
            Err(Error::Unsupported)
        );
    }
    for (action, flags, reserved) in [(1, 1, 0), (1, 0, 1), (2, 1, 0), (3, 0, u64::MAX)] {
        assert_eq!(
            abi::validate_system_action(action, flags, reserved),
            Err(Error::Invalid)
        );
    }
}
#[test]
fn authority_busy_and_unsupported_errors_have_distinct_roundtrips_below_syscall_sentinels() {
    for error in [Error::Denied, Error::Unsupported, Error::Busy] {
        assert_eq!(abi::decode(error.encode()), Err(error));
        assert!(error.encode() < u64::MAX - 255);
    }
    assert_eq!(abi::decode(0), Ok(0));
}
