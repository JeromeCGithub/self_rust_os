//! Dirty tiles bound writes to video memory without another full-screen buffer.
pub const TILE: usize = 32;
pub const ROWS: usize = 34;
/// Conservative reach of the quarter-resolution glass backdrop sampler.
pub const BACKDROP_MARGIN: usize = 16;

pub type Bounds = (usize, usize, usize, usize);

/// A content-only update can redraw one window when its complete paint bounds
/// and backdrop sampling reach do not intersect another window. Layout and
/// background changes use full compose.
pub fn isolated_content(
    bounds: &[Bounds],
    changed: u32,
    scene_changed: bool,
) -> Option<(usize, Bounds)> {
    if scene_changed || changed.count_ones() != 1 {
        return None;
    }
    let index = changed.trailing_zeros() as usize;
    let target = *bounds.get(index)?;
    for (other, rect) in bounds.iter().enumerate() {
        if rect.0 >= rect.2 || rect.1 >= rect.3 {
            return None;
        }
        if other != index
            && target.0 < rect.2.saturating_add(BACKDROP_MARGIN)
            && rect.0 < target.2.saturating_add(BACKDROP_MARGIN)
            && target.1 < rect.3.saturating_add(BACKDROP_MARGIN)
            && rect.1 < target.3.saturating_add(BACKDROP_MARGIN)
        {
            return None;
        }
    }
    Some((index, target))
}

pub struct Damage {
    rows: [u64; ROWS],
}

impl Damage {
    pub const fn new() -> Self {
        Self { rows: [0; ROWS] }
    }
    pub fn mark(&mut self, x: usize, y: usize, width: usize, height: usize) {
        let right = x.saturating_add(width).min(1920);
        let bottom = y.saturating_add(height).min(1080);
        if width == 0 || height == 0 || x >= right || y >= bottom {
            return;
        }
        let first = x / TILE;
        let end = (right - 1) / TILE + 1;
        let mask = ((1u64 << end) - 1) & !((1u64 << first) - 1);
        for row in y / TILE..=(bottom - 1) / TILE {
            self.rows[row] |= mask;
        }
    }

    /// Emits packed-source/destination row spans, clipped to current geometry.
    pub fn flush(
        &mut self,
        width: usize,
        height: usize,
        mut copy: impl FnMut(usize, usize, usize),
    ) {
        for row in 0..(height + TILE - 1) / TILE {
            let mut bits = self.take_row(row);
            while bits != 0 {
                let first = bits.trailing_zeros() as usize;
                let count = (bits >> first).trailing_ones() as usize;
                bits &= !(((1u64 << count) - 1) << first);
                let x = first * TILE;
                let len = ((first + count) * TILE).min(width).saturating_sub(x);
                if len == 0 {
                    continue;
                }
                for y in row * TILE..((row + 1) * TILE).min(height) {
                    copy(x, y, len);
                }
            }
        }
    }
    pub fn take_row(&mut self, row: usize) -> u64 {
        core::mem::take(&mut self.rows[row])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disjoint_content_does_not_redraw_unchanged_windows() {
        let bounds = [(868, 310, 1380, 650), (80, 120, 592, 460)];
        assert_eq!(isolated_content(&bounds, 2, false), Some((1, bounds[1])));
        assert_eq!(isolated_content(&bounds, 1, false), Some((0, bounds[0])));
        assert_eq!(isolated_content(&bounds, 0, false), None);
        assert_eq!(
            isolated_content(&bounds, 2, true),
            None,
            "background, focus and geometry changes require a complete scene"
        );
        assert_eq!(isolated_content(&bounds, 3, false), None);
        assert_eq!(isolated_content(&bounds, 1 << 31, false), None);
        assert_eq!(
            isolated_content(&[(0, 0, 100, 100), (90, 90, 200, 200)], 2, false),
            None,
            "overlapping glass retains bottom-to-top backdrop recomposition"
        );
        assert_eq!(isolated_content(&[(0, 0, 0, 100)], 1, false), None);
        assert_eq!(
            isolated_content(&[(0, 0, 100, 100), (108, 0, 208, 100)], 1, false),
            None,
            "nearby glass can sample changed pixels even when paint rectangles do not overlap"
        );
    }
    #[test]
    fn idle_and_single_pixel_do_not_damage_the_screen() {
        let mut damage = Damage::new();
        assert_eq!(damage.take_row(0), 0);
        damage.mark(45, 70, 1, 1);
        for row in 0..ROWS {
            assert_eq!(damage.take_row(row), if row == 2 { 2 } else { 0 });
            assert_eq!(damage.take_row(row), 0, "presentation consumes damage");
        }
    }
    #[test]
    fn cursor_crosses_tiles_and_clips_at_1080p_edges() {
        let mut damage = Damage::new();
        damage.mark(30, 30, 8, 14);
        assert_eq!(damage.take_row(0), 3);
        assert_eq!(damage.take_row(1), 3);
        damage.mark(1919, 1079, usize::MAX, usize::MAX);
        assert_eq!(damage.take_row(33), 1 << 59);
        damage.mark(1920, 1080, 1, 1);
        damage.mark(0, 0, 0, 10);
        for row in 0..ROWS {
            assert_eq!(damage.take_row(row), 0);
        }
    }
    #[test]
    fn presentation_clips_partial_tiles_and_preserves_pitch_padding_and_guards() {
        let (width, height, pitch) = (1919, 1079, 2048);
        let source = vec![0x123456u32; width * height];
        let mut display = vec![0xaabbccu32; pitch * height + 4];
        let mut damage = Damage::new();
        damage.mark(1918, 1078, 8, 14);
        let mut copied = 0;
        damage.flush(width, height, |x, y, len| {
            display[y * pitch + x..y * pitch + x + len]
                .copy_from_slice(&source[y * width + x..y * width + x + len]);
            copied += len;
        });
        assert_eq!(copied, (width - 1888) * (height - 1056));
        for y in 0..height {
            for x in 0..pitch {
                assert_eq!(
                    display[y * pitch + x],
                    if y >= 1056 && (1888..width).contains(&x) {
                        0x123456
                    } else {
                        0xaabbcc
                    }
                );
            }
        }
        assert!(display[pitch * height..]
            .iter()
            .all(|pixel| *pixel == 0xaabbcc));
        damage.flush(width, height, |_, _, _| {
            panic!("unchanged frame must not present")
        });
    }
    #[test]
    fn full_redraw_covers_the_partial_last_tile_row() {
        let mut damage = Damage::new();
        damage.mark(0, 0, 1920, 1080);
        for row in 0..ROWS {
            assert_eq!(damage.take_row(row), (1 << 60) - 1);
        }
    }
}
