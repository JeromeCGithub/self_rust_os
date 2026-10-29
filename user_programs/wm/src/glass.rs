//! Small separable CPU blur. This operates only on a downsampled backdrop.
pub fn blur(
    source: &[u32],
    destination: &mut [u32],
    width: usize,
    height: usize,
    horizontal: bool,
) {
    let Some(len) = width.checked_mul(height) else {
        return;
    };
    if width == 0 || height == 0 || source.len() < len || destination.len() < len {
        return;
    }
    // Five-tap separable box filter on a 1/4 resolution backdrop (~20px blur).
    // All math is integer: the OS does not yet save floating-point/SIMD state.
    for y in 0..height {
        for x in 0..width {
            let mut channels = [0u32; 3];
            for offset in -2isize..=2 {
                let sx = if horizontal {
                    x.saturating_add_signed(offset).min(width - 1)
                } else {
                    x
                };
                let sy = if horizontal {
                    y
                } else {
                    y.saturating_add_signed(offset).min(height - 1)
                };
                let pixel = source[sy * width + sx];
                for (channel, shift) in channels.iter_mut().zip([16, 8, 0]) {
                    *channel += (pixel >> shift) & 255;
                }
            }
            destination[y * width + x] =
                ((channels[0] / 5) << 16) | ((channels[1] / 5) << 8) | channels[2] / 5;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constant_backdrops_survive_blur_without_touching_guards() {
        let source = [0x123456; 35];
        let mut dest = [0xdeadbeef; 39];
        blur(&source, &mut dest, 7, 5, true);
        assert!(dest[..35].iter().all(|p| *p == 0x123456));
        assert_eq!(&dest[35..], &[0xdeadbeef; 4]);
    }
    #[test]
    fn blur_spreads_background_changes_in_both_directions() {
        let mut source = [0; 81];
        source[40] = 0xffffff;
        let mut horizontal = [0; 81];
        let mut result = [0; 81];
        blur(&source, &mut horizontal, 9, 9, true);
        blur(&horizontal, &mut result, 9, 9, false);
        assert!(result[40] > 0 && result[39] > 0 && result[31] > 0);
        assert_eq!(result[0], 0, "distant pixels stay unchanged");
        assert_eq!(result[39], result[41]);
        assert_eq!(result[31], result[49]);
    }
    #[test]
    fn malformed_geometry_does_not_write_anything() {
        let mut dest = [42; 4];
        for (w, h) in [(0, 2), (3, 2), (usize::MAX, 2)] {
            blur(&[0; 4], &mut dest, w, h, true);
            assert_eq!(dest, [42; 4]);
        }
    }
}
