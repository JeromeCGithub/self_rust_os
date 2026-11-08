//! Optional proportional text painter. Immutable metrics can be shared with layout.
use super::{Canvas, Color};
use alloc::{collections::BTreeMap, sync::Arc};
use rustos_text::Glyph;
pub use rustos_text::{Error, Family, FontSet, Metrics, Style};
const CACHE_BYTES: usize = 1024 * 1024;
const CACHE_GLYPHS: usize = 512;
/// Thread-owned glyph cache; no process-global fonts or mutable cross-thread data.
pub struct Painter {
    fonts: Arc<FontSet>,
    cache: BTreeMap<(Style, char), Glyph>,
    bytes: usize,
}
impl Painter {
    /// Retain shared immutable faces and an initially empty bounded coverage cache.
    pub fn new(fonts: Arc<FontSet>) -> Self {
        Self {
            fonts,
            cache: BTreeMap::new(),
            bytes: 0,
        }
    }
    /// Draw single-line text with the same integer advances used by document layout.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas<'_>,
        mut x: i32,
        y: i32,
        text: &str,
        style: Style,
        color: Color,
    ) -> Result<i32, Error> {
        for ch in text.chars() {
            let key = (style, ch);
            if !self.cache.contains_key(&key) {
                let glyph = self.fonts.glyph(ch, style)?;
                if self.bytes + glyph.coverage.len() > CACHE_BYTES
                    || self.cache.len() == CACHE_GLYPHS
                {
                    self.cache.clear();
                    self.bytes = 0;
                }
                self.bytes += glyph.coverage.len();
                self.cache.insert(key, glyph);
            }
            let glyph = self.cache.get(&key).ok_or(Error::Font)?;
            if !canvas.alpha_mask(
                x.saturating_add(glyph.left),
                y.saturating_add(glyph.top),
                glyph.width,
                glyph.height,
                &glyph.coverage,
                color,
            ) {
                return Err(Error::Limit);
            }
            x = x.saturating_add(glyph.advance as i32);
        }
        Ok(x)
    }
    /// Live coverage bytes; cache bookkeeping is separately bounded by 512 entries.
    pub fn cached_bytes(&self) -> usize {
        self.bytes
    }
}
