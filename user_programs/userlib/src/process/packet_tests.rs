#[path = "packet.rs"]
mod packet;
use packet::Packet;
#[test]
fn packets_preserve_binary_values_and_reject_empty_oversize_or_noncanonical_words() {
    for source in [&[0u8][..], &[0, 255][..], &[0, 255, 128][..]] {
        let encoded = Packet::from_bytes(source).unwrap();
        let decoded = Packet::from_word(encoded.word()).unwrap();
        assert_eq!(decoded.bytes(), source);
        let (bytes, length) = decoded.parts();
        assert_eq!(&bytes[..length], source);
    }
    assert!(Packet::from_bytes(&[]).is_none());
    assert!(Packet::from_bytes(&[0; 4]).is_none());
    for word in [0, 0x04000000, 0xff000000, 0x01000100, 0x02010000] {
        assert!(Packet::from_word(word).is_none());
    }
}
