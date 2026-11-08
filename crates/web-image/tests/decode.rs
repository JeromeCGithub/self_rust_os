use rustos_web_image::{Error, Image};
#[test]
fn rgba_and_alpha_are_native_pixels_and_ancillary_profiles_are_not_expanded() {
    for bytes in [
        include_bytes!("fixtures/two-pixels.png").as_slice(),
        include_bytes!("fixtures/metadata.png").as_slice(),
    ] {
        let image = Image::decode(bytes).expect("native PNG");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [255, 0, 0, 255, 0, 255, 0, 128]);
    }
}
#[test]
fn dimensions_crc_and_truncated_inputs_fail_before_output() {
    assert!(matches!(
        Image::decode(include_bytes!("fixtures/oversized.png")),
        Err(Error::Limit)
    ));
    let source = include_bytes!("fixtures/two-pixels.png");
    for length in 0..source.len() {
        assert!(Image::decode(&source[..length]).is_err());
    }
    let mut broken = source.to_vec();
    broken[45] ^= 1;
    assert!(matches!(Image::decode(&broken), Err(Error::Format)));
}
