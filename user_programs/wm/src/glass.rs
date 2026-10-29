//! Small separable CPU blur. This operates only on a downsampled backdrop.
/// Bilinear reconstruction from quarter-resolution material pixels.
pub fn sample(pixels: &[u32], width: usize, height: usize, x: usize, y: usize) -> u32 {
    let Some(len) = width.checked_mul(height) else {
        return 0;
    };
    if width == 0 || height == 0 || pixels.len() < len {
        return 0;
    }
    let sx = (x / 4).min(width - 1);
    let sy = (y / 4).min(height - 1);
    let nx = (sx + 1).min(width - 1);
    let ny = (sy + 1).min(height - 1);
    let a = mix(pixels[sy * width + sx], pixels[sy * width + nx], x % 4);
    let b = mix(pixels[ny * width + sx], pixels[ny * width + nx], x % 4);
    mix(a, b, y % 4) | 0xff000000
}
fn mix(a: u32, b: u32, t: usize) -> u32 {
    let t = t as u32;
    let inv = 4 - t;
    let rb = (((a & 0xff00ff) * inv + (b & 0xff00ff) * t) >> 2) & 0xff00ff;
    let g = (((a & 0x00ff00) * inv + (b & 0x00ff00) * t) >> 2) & 0x00ff00;
    rb | g
}
/// Readability-aware dark glass tint, preserving the backdrop's color.
pub fn shade(backdrop: u32, clear: bool) -> u32 {
    let r = (backdrop >> 16) & 255;
    let g = (backdrop >> 8) & 255;
    let b = backdrop & 255;
    let luma = (r * 54 + g * 183 + b * 19) >> 8;
    let alpha = if clear {
        80 + luma * 80 / 255
    } else {
        145 + luma * 60 / 255
    };
    let inv = 255 - alpha;
    0xff000000
        | (((r * inv + 16 * alpha) / 255) << 16)
        | (((g * inv + 27 * alpha) / 255) << 8)
        | ((b * inv + 44 * alpha) / 255)
}
/// Directional edge illumination, stronger on the upper edge.
pub fn rim(x: i32, y: i32, width: i32, height: i32, active: bool) -> u8 {
    let light = if y < 2 {
        132
    } else if x < 2 {
        88
    } else if y >= height - 2 {
        44
    } else if x >= width - 2 {
        38
    } else {
        56
    };
    if active {
        light
    } else {
        light / 2
    }
}
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
    fn reconstruction_is_smooth_and_clamped_at_material_edges() {
        let image = [0xff000000, 0xffffffff, 0xff000000, 0xffffffff];
        assert_eq!(sample(&image, 2, 2, 0, 0), 0xff000000);
        assert_eq!(sample(&image, 2, 2, 2, 0), 0xff7f7f7f);
        assert_eq!(sample(&image, 2, 2, 100, 100), 0xffffffff);
        assert_eq!(sample(&[], 0, 0, 1, 1), 0);
    }
    #[test]
    fn material_limits_bright_backgrounds_and_retains_clearer_color() {
        let regular = shade(0xffffff, false);
        let clear = shade(0xffffff, true);
        assert!(clear & 255 > regular & 255);
        assert!(
            clear & 255 < 128,
            "white foreground needs a dimming layer over a bright backdrop"
        );
        let blue = shade(0x204080, true);
        assert!(blue & 255 > (blue >> 16) & 255, "retain backdrop hue");
        assert_eq!(clear >> 24, 255);
    }
    #[test]
    fn rim_has_direction_and_focus_changes_strength() {
        assert!(rim(50, 0, 100, 80, true) > rim(50, 79, 100, 80, true));
        assert!(rim(50, 0, 100, 80, true) > rim(50, 0, 100, 80, false));
    }
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
