use rustos_audio_abi::*;
#[test]
fn records_are_padding_free_and_errors_do_not_alias_handles_or_other_services() {
    assert_eq!(core::mem::size_of::<Status>(), 40);
    assert_eq!(core::mem::align_of::<Status>(), 8);
    for error in [
        Error::Invalid,
        Error::NoDevice,
        Error::Busy,
        Error::WouldBlock,
        Error::Failed,
        Error::NotOwner,
    ] {
        assert_eq!(decode(error.encode()), Err(error));
        assert!(error.encode() < u64::MAX - 511);
    }
    assert_eq!(decode(42), Ok(42));
    assert_eq!(RATE, 48000);
    assert_eq!(CHANNELS, 2);
    assert_eq!(FRAME_BYTES, 4);
}
