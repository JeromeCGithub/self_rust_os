//! Font metrics are shared by layout and painting; neither accesses the OS.
#![no_std]
extern crate alloc;
use alloc::vec::Vec;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Sans,
    Serif,
    Monospace,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Style {
    pub size: u16,
    pub family: Family,
    pub bold: bool,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            size: 16,
            family: Family::Sans,
            bold: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Font,
    Limit,
}
pub trait Metrics: Sync {
    fn advance(&self, ch: char, style: Style) -> u32;
    fn line_height(&self, style: Style) -> u32;
    fn default_size(&self) -> u16;
}
pub struct Bitmap;
impl Metrics for Bitmap {
    fn advance(&self, _: char, style: Style) -> u32 {
        u32::from(style.size).div_ceil(8).clamp(1, 4) * 8
    }
    fn line_height(&self, style: Style) -> u32 {
        self.advance(' ', style) + 4
    }
    fn default_size(&self) -> u16 {
        8
    }
}
pub struct Glyph {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub advance: u32,
    pub coverage: Vec<u8>,
}
#[cfg(feature = "truetype")]
pub struct FontSet {
    fonts: [fontdue::Font; 4],
    extra_bold: Option<[fontdue::Font; 2]>,
}
#[cfg(feature = "truetype")]
impl FontSet {
    /// Load trusted application font assets. Web fonts require separate isolation
    /// and compiled-outline budgets, beyond this initial embedded-font profile.
    pub fn load(sans: &[u8], bold: &[u8], serif: &[u8], mono: &[u8]) -> Result<Self, Error> {
        fn load(bytes: &[u8]) -> Result<fontdue::Font, Error> {
            if bytes.len() > 2 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            fontdue::Font::from_bytes(
                bytes,
                fontdue::FontSettings {
                    load_substitutions: false,
                    ..fontdue::FontSettings::default()
                },
            )
            .map_err(|_| Error::Font)
        }
        Ok(Self {
            fonts: [load(sans)?, load(bold)?, load(serif)?, load(mono)?],
            extra_bold: None,
        })
    }
    /// Add matching bold serif/monospace faces before sharing the immutable set.
    pub fn with_bold_faces(mut self, serif: &[u8], mono: &[u8]) -> Result<Self, Error> {
        let load = |bytes: &[u8]| {
            if bytes.len() > 2 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            fontdue::Font::from_bytes(
                bytes,
                fontdue::FontSettings {
                    load_substitutions: false,
                    ..fontdue::FontSettings::default()
                },
            )
            .map_err(|_| Error::Font)
        };
        self.extra_bold = Some([load(serif)?, load(mono)?]);
        Ok(self)
    }
    fn select(&self, ch: char, style: Style) -> &fontdue::Font {
        let index = match style.family {
            Family::Sans => usize::from(style.bold),
            Family::Serif => 2,
            Family::Monospace => 3,
        };
        let font = match (style.family, style.bold, &self.extra_bold) {
            (Family::Serif, true, Some(bold)) => &bold[0],
            (Family::Monospace, true, Some(bold)) => &bold[1],
            _ => &self.fonts[index],
        };
        if font.lookup_glyph_index(ch) == 0 {
            &self.fonts[usize::from(style.bold)]
        } else {
            font
        }
    }
    /// Rasterize one glyph at a bounded size with baseline-relative placement.
    pub fn glyph(&self, ch: char, style: Style) -> Result<Glyph, Error> {
        if !(1..=96).contains(&style.size) {
            return Err(Error::Limit);
        }
        let font = self.select(ch, style);
        let size = f32::from(style.size);
        let ascent = font
            .horizontal_line_metrics(size)
            .map(|line| ceil(line.ascent))
            .unwrap_or(u32::from(style.size));
        let (metrics, coverage) = font.rasterize(ch, size);
        if metrics.width > 256
            || metrics.height > 256
            || metrics.width.checked_mul(metrics.height) != Some(coverage.len())
        {
            return Err(Error::Limit);
        }
        Ok(Glyph {
            width: metrics.width as u32,
            height: metrics.height as u32,
            left: metrics.xmin,
            top: ascent as i32 - metrics.ymin - metrics.height as i32,
            advance: ceil(metrics.advance_width),
            coverage,
        })
    }
}
#[cfg(feature = "truetype")]
fn ceil(value: f32) -> u32 {
    if !value.is_finite() || value < 0.0 {
        return 0;
    }
    let whole = value as u32;
    whole.saturating_add(u32::from(value > whole as f32))
}
#[cfg(feature = "truetype")]
impl Metrics for FontSet {
    fn advance(&self, ch: char, style: Style) -> u32 {
        if !(1..=96).contains(&style.size) {
            return 0;
        }
        ceil(
            self.select(ch, style)
                .metrics(ch, f32::from(style.size))
                .advance_width,
        )
    }
    fn line_height(&self, style: Style) -> u32 {
        self.select('M', style)
            .horizontal_line_metrics(f32::from(style.size.clamp(1, 96)))
            .map(|line| ceil(line.new_line_size))
            .unwrap_or(u32::from(style.size))
    }
    fn default_size(&self) -> u16 {
        16
    }
}
