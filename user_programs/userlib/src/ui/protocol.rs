//! Versions 1 and 2 of the Ring 3 window protocol. The kernel transports opaque u32s.
pub const MAX_WIDTH: usize = 1920;
pub const MAX_HEIGHT: usize = 1080;
pub const VERSION2: u32 = 1 << 27;
pub const RESIZE: u32 = 0xf0000000;
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
    /// Preflights content plus decorations. The compositor enforces the actual
    /// workspace requested by the desktop environment when accepting a window.
    pub fn fits_display(self, width: usize, height: usize) -> bool {
        self.width <= width && self.height <= height.saturating_sub(28)
    }
    pub fn encode(self) -> u32 {
        CONFIGURE
            | VERSION2
            | self.width as u32
            | ((self.height as u32) << 11)
            | ((self.material as u32) << 22)
    }
    pub fn decode(message: u32) -> Option<Self> {
        let v2 = message & VERSION2 != 0;
        let (width, height, material) = if v2 {
            if message & !0x08ffffff != CONFIGURE {
                return None;
            }
            (
                (message & 2047) as usize,
                ((message >> 11) & 2047) as usize,
                (message >> 22) & 3,
            )
        } else {
            if message & !0x001fffff != CONFIGURE {
                return None;
            }
            (
                (message & 1023) as usize,
                ((message >> 10) & 511) as usize,
                (message >> 19) & 3,
            )
        };
        let material = match material {
            0 => Material::Opaque,
            1 => Material::Glass,
            2 => Material::GlassClear,
            _ => return None,
        };
        let (max_width, max_height) = if v2 {
            (MAX_WIDTH, MAX_HEIGHT)
        } else {
            (640, 400)
        };
        if !(160..=max_width).contains(&width) || !(96..=max_height).contains(&height) {
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
    fn client_geometry_reserves_only_decorations_and_leaves_panels_to_the_shell() {
        let mut config = Configuration {
            width: 640,
            height: 400,
            material: Material::Glass,
        };
        assert!(config.fits_display(1920, 1080));
        assert!(config.fits_display(640, 480));
        assert!(config.fits_display(640, 428));
        assert!(!config.fits_display(640, 427));
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

#[cfg(test)]
mod resize_tests {
    use super::*;
    #[test]
    fn desktop_sized_content_round_trips_without_geometry_or_material_aliasing() {
        for (width, height) in [(960, 600), (1920, 992), (1920, 1052)] {
            let c = Configuration {
                width,
                height,
                material: Material::GlassClear,
            };
            assert_eq!(Configuration::decode(c.encode()), Some(c));
            assert!(c.fits_display(1920, 1080));
        }
        // The old wire encoding is still accepted for existing fixed clients.
        assert_eq!(
            Configuration::decode(CONFIGURE | 640 | (400 << 10)),
            Some(Configuration {
                width: 640,
                height: 400,
                material: Material::Opaque
            })
        );
    }
}

/// Version-2 frame includes the actual immutable submitted dimensions. A resize
/// offer never changes how an older in-flight frame is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub serial: u8,
}
impl Frame {
    pub fn encode(self) -> u32 {
        COMMIT
            | self.width as u32
            | ((self.height as u32) << 11)
            | ((u32::from(self.serial) & 63) << 22)
    }
    pub fn decode(message: u32) -> Option<Self> {
        if message & KIND != COMMIT {
            return None;
        }
        let width = (message & 2047) as usize;
        let height = ((message >> 11) & 2047) as usize;
        if !(160..=MAX_WIDTH).contains(&width) || !(96..=MAX_HEIGHT).contains(&height) {
            return None;
        }
        Some(Self {
            width,
            height,
            serial: ((message >> 22) & 63) as u8,
        })
    }
}
pub fn resize_message(width: usize, height: usize) -> u32 {
    RESIZE | width as u32 | ((height as u32) << 11)
}
pub fn decode_resize(message: u32) -> Option<(usize, usize)> {
    if message & !0x003fffff != RESIZE {
        return None;
    }
    let frame = Frame::decode(COMMIT | (message & 0x003fffff))?;
    Some((frame.width, frame.height))
}
/// Signed local coordinates cover the entire supported 1920x1080 display.
pub fn pointer(x: i32, y: i32, buttons: u8) -> u32 {
    POINTER
        | (x.clamp(-2048, 2047) as u32 & 4095)
        | ((y.clamp(-2048, 2047) as u32 & 4095) << 12)
        | (u32::from(buttons & 7) << 24)
}
pub fn decode_pointer(message: u32) -> Option<(i32, i32, u8)> {
    if message & (KIND | VERSION2) != POINTER {
        return None;
    }
    Some((
        ((message << 20) as i32) >> 20,
        ((message << 8) as i32) >> 20,
        ((message >> 24) & 7) as u8,
    ))
}
#[cfg(test)]
mod frame_tests {
    use super::*;
    #[test]
    fn frame_size_and_pointer_coordinates_do_not_alias_serials_or_buttons() {
        for serial in [0, 1, 31, 63] {
            let frame = Frame {
                width: 1920,
                height: 992,
                serial,
            };
            assert_eq!(Frame::decode(frame.encode()), Some(frame));
        }
        assert_eq!(decode_resize(resize_message(1920, 992)), Some((1920, 992)));
        assert_eq!(decode_resize(resize_message(1920, 992) | (1 << 24)), None);
        for (x, y) in [(-2048, -2048), (-1, -1), (0, 0), (1919, 1079), (2047, 2047)] {
            for buttons in [0, 1, 7] {
                assert_eq!(
                    decode_pointer(pointer(x, y, buttons)),
                    Some((x, y, buttons))
                );
            }
        }
        assert_eq!(decode_pointer(pointer(1, 2, 0) | VERSION2), None);
    }
}
