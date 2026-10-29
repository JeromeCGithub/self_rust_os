//! Version 1 of the Ring 3 window protocol. The kernel transports opaque u32s.
pub const MAX_WIDTH: usize = 640;
pub const MAX_HEIGHT: usize = 400;
pub const TITLE_BYTES: usize = 24;
pub const CONFIGURE: u32 = 0x10000000;
pub const TITLE: u32 = 0x20000000;
pub const COMMIT: u32 = 0x30000000;
pub const RELEASE: u32 = 0x40000000;
pub const POINTER: u32 = 0x50000000;
pub const FOCUS: u32 = 0x60000000;
pub const CLOSE: u32 = 0x70000000;
pub const ACCEPT: u32 = 0x80000000;
pub const REJECT: u32 = 0x90000000;
pub const KIND: u32 = 0xf0000000;
pub const SERIAL: u32 = 0x0fffffff;

/// A compositor-owned background material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Material {
    Opaque = 0,
    Glass = 1,
    GlassClear = 2,
}

/// Validated, independently sized content geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub width: usize,
    pub height: usize,
    pub material: Material,
}
impl Configuration {
    /// Checks that content, titlebar and desktop panels fit the display.
    pub fn fits_display(self, width: usize, height: usize) -> bool {
        // 28px top panel/placement margin + 28px titlebar + 32px bottom panel.
        self.width <= width && self.height <= height.saturating_sub(88)
    }
    pub fn encode(self) -> u32 {
        CONFIGURE
            | self.width as u32
            | ((self.height as u32) << 10)
            | ((self.material as u32) << 19)
    }
    pub fn decode(message: u32) -> Option<Self> {
        if message & !0x001fffff != CONFIGURE {
            return None;
        }
        let width = (message & 1023) as usize;
        let height = ((message >> 10) & 511) as usize;
        let material = match (message >> 19) & 3 {
            0 => Material::Opaque,
            1 => Material::Glass,
            2 => Material::GlassClear,
            _ => return None,
        };
        if !(160..=MAX_WIDTH).contains(&width) || !(96..=MAX_HEIGHT).contains(&height) {
            return None;
        }
        Some(Self {
            width,
            height,
            material,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_reserves_space_for_title_and_desktop_panels() {
        let mut config = Configuration {
            width: 640,
            height: 400,
            material: Material::Glass,
        };
        assert!(config.fits_display(1920, 1080));
        assert!(!config.fits_display(640, 480));
        config.height = 392;
        assert!(config.fits_display(640, 480));
        assert!(!config.fits_display(639, 480));
        assert!(!config.fits_display(640, 0));
    }
    #[test]
    fn geometry_and_material_round_trip_and_reject_malformed_requests() {
        for material in [Material::Opaque, Material::Glass, Material::GlassClear] {
            for (width, height) in [(160, 96), (488, 288), (640, 400)] {
                let config = Configuration {
                    width,
                    height,
                    material,
                };
                assert_eq!(Configuration::decode(config.encode()), Some(config));
            }
        }
        for message in [
            0,
            CONFIGURE,
            CONFIGURE | 641 | (400 << 10),
            CONFIGURE | 640 | (401 << 10),
            CONFIGURE | 640 | (400 << 10) | (3 << 19),
            CONFIGURE | 640 | (400 << 10) | (1 << 25),
        ] {
            assert_eq!(Configuration::decode(message), None);
        }
    }
}
