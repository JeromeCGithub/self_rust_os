use rustos_media::{Error, Wav};
fn wav() -> Vec<u8> {
    let mut data=Vec::from(&b"RIFF\x2c\0\0\0WAVEfmt \x10\0\0\0\x01\0\x02\0\x80\xbb\0\0\0\xee\x02\0\x04\0\x10\0data\x08\0\0\0"[..]);
    data.extend_from_slice(&[1, 0, 2, 0, 3, 0, 4, 0]);
    data
}
#[test]
fn native_pcm_chunks_seek_in_frames_and_preserve_channels_without_copying() {
    let data = wav();
    let clip = Wav::parse(&data).unwrap();
    assert_eq!(clip.frames(), 2);
    assert_eq!(clip.chunk(0, 4).unwrap(), &[1, 0, 2, 0]);
    assert_eq!(clip.chunk(1, 4096).unwrap(), &[3, 0, 4, 0]);
    assert!(clip.chunk(2, 4).unwrap().is_empty());
    assert_eq!(clip.chunk(3, 4), Err(Error::Invalid));
    for length in [0, 3, 4097] {
        assert_eq!(clip.chunk(0, length), Err(Error::Invalid));
    }
}
#[test]
fn malformed_truncated_ambiguous_and_wrong_profiles_fail_closed() {
    let data = wav();
    for end in 0..data.len() {
        assert!(Wav::parse(&data[..end]).is_err());
    }
    for offset in [0, 8, 20, 22, 24, 28, 32, 34, 40] {
        let mut bad = data.clone();
        bad[offset] ^= 0x40;
        assert!(Wav::parse(&bad).is_err(), "offset {offset}");
    }
    let mut bad = data.clone();
    bad.push(0);
    assert!(Wav::parse(&bad).is_err());
    let mut bad = data.clone();
    bad[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Wav::parse(&bad).is_err());
}
#[test]
fn odd_sized_metadata_chunks_are_bounded_and_skipped_with_padding() {
    let original = wav();
    let mut data = original[..12].to_vec();
    data.extend_from_slice(b"JUNK\x01\0\0\0x\0");
    data.extend_from_slice(&original[12..]);
    let size = (data.len() - 8) as u32;
    data[4..8].copy_from_slice(&size.to_le_bytes());
    assert_eq!(Wav::parse(&data).unwrap().frames(), 2);
    data[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Wav::parse(&data).is_err());
}
