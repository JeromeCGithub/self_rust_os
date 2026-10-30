//! Allocation-free, pitch-aware software drawing. Pixels use premultiplied ARGB.

/// Copies visible rows between owned surfaces with independent pixel strides.
/// Rejects inconsistent/overflowing geometry before writing any destination pixel.
pub fn copy_surface(
    source: &[u32],
    destination: &mut [u32],
    width: usize,
    height: usize,
    source_stride: usize,
    destination_stride: usize,
) -> bool {
    if width == 0 || height == 0 || width > source_stride || width > destination_stride {
        return false;
    }
    let source_end = (height - 1)
        .checked_mul(source_stride)
        .and_then(|n| n.checked_add(width));
    let destination_end = (height - 1)
        .checked_mul(destination_stride)
        .and_then(|n| n.checked_add(width));
    if source_end.map(|end| end > source.len()).unwrap_or(true)
        || destination_end
            .map(|end| end > destination.len())
            .unwrap_or(true)
    {
        return false;
    }
    for y in 0..height {
        destination[y * destination_stride..y * destination_stride + width]
            .copy_from_slice(&source[y * source_stride..y * source_stride + width]);
    }
    true
}

/// A premultiplied color; construct with straight RGB and alpha components.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);
impl Color {
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self(0);
    /// Opaque RGB color.
    pub const fn rgb(rgb: u32) -> Self {
        Self(0xff000000 | (rgb & 0xffffff))
    }
    /// Straight RGBA components, converted to premultiplied storage.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        let alpha = a as u32;
        Self(
            (alpha << 24)
                | (((r as u32 * alpha + 127) / 255) << 16)
                | (((g as u32 * alpha + 127) / 255) << 8)
                | ((b as u32 * alpha + 127) / 255),
        )
    }
}

/// Rectangle in content-local pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Rect {
    /// Constructs a content-local rectangle.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    /// Tests content-local coordinates, including negative origins and large sizes.
    pub fn contains(self, x: i32, y: i32) -> bool {
        i64::from(x) >= i64::from(self.x)
            && i64::from(y) >= i64::from(self.y)
            && i64::from(x) < i64::from(self.x) + i64::from(self.width)
            && i64::from(y) < i64::from(self.y) + i64::from(self.height)
    }
}

/// Premultiplied source-over blend; opaque XRGB destinations are also accepted.
pub fn over(source: u32, destination: u32) -> u32 {
    if source >> 24 == 255 {
        return source;
    }
    if source >> 24 == 0 {
        return destination;
    }
    let inv = 255 - (source >> 24);
    let mut result = 0;
    for shift in [0, 8, 16, 24] {
        let channel =
            ((source >> shift) & 255) + (((destination >> shift) & 255) * inv + 127) / 255;
        result |= channel.min(255) << shift;
    }
    result
}

/// Rounded rectangle coverage, sampled at four subpixel positions.
pub fn coverage(x: i32, y: i32, width: i32, height: i32, radius: i32) -> u8 {
    if x < 0 || y < 0 || x >= width || y >= height {
        return 0;
    }
    let radius = radius.max(0).min(width / 2).min(height / 2);
    if (x >= radius && x < width - radius) || (y >= radius && y < height - radius) {
        return 255;
    }
    let r = i64::from(radius) * 4;
    if r == 0 {
        return 255;
    }
    let mut count = 0;
    for sy in [1, 3] {
        for sx in [1, 3] {
            let px = i64::from(x) * 4 + sx;
            let py = i64::from(y) * 4 + sy;
            let dx = (r - px).max(px - (i64::from(width) * 4 - r)).max(0);
            let dy = (r - py).max(py - (i64::from(height) * 4 - r)).max(0);
            // Large public rectangle dimensions can exceed i64 when squared.
            // Wider integer intermediates keep clipping/coverage defined too.
            count += u32::from(
                i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy)
                    <= i128::from(r) * i128::from(r),
            );
        }
    }
    (count * 255 / 4) as u8
}

/// Applies coverage to every premultiplied component.
pub fn opacity(color: u32, alpha: u8) -> u32 {
    if alpha == 255 {
        return color;
    }
    if alpha == 0 {
        return 0;
    }
    let mut result = 0;
    for shift in [0, 8, 16, 24] {
        result |= ((((color >> shift) & 255) * u32::from(alpha) + 127) / 255) << shift;
    }
    result
}

/// Exclusive, clipped access to a pixel buffer. Stride is measured in pixels.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
    stride: usize,
}
impl<'a> Canvas<'a> {
    /// Rejects empty, overflowing, undersized or inconsistent buffer geometry.
    pub fn new(pixels: &'a mut [u32], width: usize, height: usize, stride: usize) -> Option<Self> {
        if width == 0
            || height == 0
            || width > stride
            || width > i32::MAX as usize
            || height > i32::MAX as usize
        {
            return None;
        }
        let end = (height - 1).checked_mul(stride)?.checked_add(width)?;
        if end > pixels.len() {
            return None;
        }
        Some(Self {
            pixels,
            width,
            height,
            stride,
        })
    }
    /// Content width in pixels.
    pub fn width(&self) -> u32 {
        self.width as u32
    }
    /// Content height in pixels.
    pub fn height(&self) -> u32 {
        self.height as u32
    }
    /// Replaces visible pixels, preserving scanline padding.
    pub fn clear(&mut self, color: Color) {
        for y in 0..self.height {
            self.pixels[y * self.stride..y * self.stride + self.width].fill(color.0);
        }
    }
    /// Blends one clipped pixel.
    pub fn pixel(&mut self, x: i32, y: i32, color: Color) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let index = y as usize * self.stride + x as usize;
            self.pixels[index] = over(color.0, self.pixels[index]);
        }
    }
    /// Blends a clipped rectangle over existing pixels.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.rounded_rect(rect, 0, color);
    }
    /// Draws a clipped rounded rectangle with antialiased corners.
    pub fn rounded_rect(&mut self, rect: Rect, radius: u32, color: Color) {
        let right = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, self.width as i64) as i32;
        let bottom =
            (i64::from(rect.y) + i64::from(rect.height)).clamp(0, self.height as i64) as i32;
        for y in rect.y.max(0)..bottom {
            for x in rect.x.max(0)..right {
                let alpha = coverage(
                    x.saturating_sub(rect.x),
                    y.saturating_sub(rect.y),
                    rect.width.min(i32::MAX as u32) as i32,
                    rect.height.min(i32::MAX as u32) as i32,
                    radius.min(i32::MAX as u32) as i32,
                );
                self.pixel(x, y, Color(opacity(color.0, alpha)));
            }
        }
    }
    /// Renders a monochrome 8x8 glyph at an integer scale of 1 through 4.
    pub fn glyph(&mut self, x: i32, y: i32, glyph: &[u8; 8], color: Color, scale: u32) {
        let scale = scale.clamp(1, 4);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..8 {
                if bits & (1 << column) != 0 {
                    self.fill_rect(
                        Rect::new(
                            x.saturating_add(column * scale as i32),
                            y.saturating_add(row as i32 * scale as i32),
                            scale,
                            scale,
                        ),
                        color,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surface_copy_preserves_padding_guards_and_rejects_partial_updates() {
        let source = [1, 2, 99, 99, 3, 4, 99, 99];
        let mut destination = [42; 8];
        assert!(copy_surface(&source, &mut destination, 2, 2, 4, 3));
        assert_eq!(destination, [1, 2, 42, 3, 4, 42, 42, 42]);
        let before = destination;
        for (w, h, src, dst) in [
            (3, 2, 2, 3),
            (2, 5, 4, 3),
            (2, 2, usize::MAX, 3),
            (0, 2, 4, 3),
        ] {
            assert!(!copy_surface(&source, &mut destination, w, h, src, dst));
            assert_eq!(
                destination, before,
                "failed copy must leave the destination intact"
            );
        }
    }
    #[test]
    fn rounded_coverage_handles_large_geometry_and_preserves_clear_corners() {
        assert_eq!(coverage(0, 0, i32::MAX, i32::MAX, i32::MAX), 0);
        assert_eq!(coverage(0, 0, 100, 60, 16), 0);
        assert_eq!(coverage(50, 30, 100, 60, 16), 255);
        assert_eq!(coverage(-1, 10, 100, 60, 16), 0);
        let mut pixels = [0; 16 * 16];
        let mut canvas = Canvas::new(&mut pixels, 16, 16, 16).unwrap();
        canvas.rounded_rect(Rect::new(0, 0, 16, 16), 6, Color::rgb(0xffffff));
        assert_eq!(pixels[0], 0);
        assert_eq!(pixels[8 * 16 + 8], 0xffffffff);
        assert!(pixels
            .iter()
            .any(|pixel| pixel >> 24 > 0 && pixel >> 24 < 255));
    }
    #[test]
    fn clipping_preserves_pitch_padding_and_trailing_guards() {
        let mut pixels = [0xdeadbeef; 18];
        let mut canvas = Canvas::new(&mut pixels, 3, 3, 5).unwrap();
        canvas.fill_rect(Rect::new(-2, 1, 4, u32::MAX), Color::rgb(0x123456));
        for (i, pixel) in pixels.iter().enumerate() {
            assert_eq!(
                *pixel,
                if (5..7).contains(&i) || (10..12).contains(&i) {
                    0xff123456
                } else {
                    0xdeadbeef
                }
            );
        }
    }
    #[test]
    fn invalid_geometry_never_constructs_a_canvas() {
        let mut pixels = [0; 16];
        assert!(Canvas::new(&mut pixels, 0, 4, 4).is_none());
        assert!(Canvas::new(&mut pixels, 4, 0, 4).is_none());
        assert!(Canvas::new(&mut pixels, 5, 4, 4).is_none());
        assert!(Canvas::new(&mut pixels, 4, 5, 4).is_none());
        assert!(Canvas::new(&mut pixels, 4, usize::MAX, usize::MAX).is_none());
    }
    #[test]
    fn translucent_drawing_is_premultiplied_and_composited() {
        let mut pixel = [0xff0000ff];
        let mut canvas = Canvas::new(&mut pixel, 1, 1, 1).unwrap();
        canvas.fill_rect(Rect::new(0, 0, 1, 1), Color::rgba(255, 0, 0, 128));
        assert_eq!(pixel[0], 0xff80007f);
        assert_eq!(Color::rgba(255, 255, 255, 0), Color::TRANSPARENT);
    }
}
