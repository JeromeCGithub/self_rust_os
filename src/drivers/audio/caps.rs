//! Pure validation for bounded audio I/O apertures.
/// Decode a16-bit I/O BAR whose complete aperture fits the port address space.
pub fn io_bar(raw: u32, bytes: u16) -> Option<u16> {
    let base = raw & !3;
    if raw & 1 == 0
        || base == 0
        || bytes == 0
        || base
            .checked_add(u32::from(bytes))
            .is_none_or(|end| end > 65536)
    {
        None
    } else {
        Some(base as u16)
    }
}
#[cfg(test)]
mod tests {
    #[cfg_attr(target_os="none",test_case)]
    #[cfg_attr(not(target_os="none"),test)]
    fn invalid_or_wrapping_io_apertures_fail_without_panicking() {
        assert_eq!(super::io_bar(0xc001, 256), Some(0xc000));
        for (raw, size) in [
            (0, 64),
            (0xffffffff, 64),
            (0xfffffffd, 256),
            (0xffc1, 256),
            (0xc000, 64),
            (0xc001, 0),
        ] {
            assert_eq!(super::io_bar(raw, size), None);
        }
    }
}
