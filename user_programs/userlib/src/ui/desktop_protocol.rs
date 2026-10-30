//! Shell protocol layered on the existing authenticated scalar channel.
pub const DESKTOP: u32 = 0xa0000000;
pub const WORK_REPLY: u32 = 0x08000000;
pub const WORK_AREA: u32 = 0xb0000000;
pub const MANAGE: u32 = 0xc0000000;
pub const MANAGED_TITLE: u32 = 0xd0000000;
pub const DESKTOP_POINTER: u32 = 0xe0000000;
pub const PID_MASK: u32 = 0x03ffffff;

/// Vertical space reserved by the shell; the compositor adds window decorations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkArea {
    pub top: u16,
    pub bottom: u16,
}
impl WorkArea {
    pub fn encode(self) -> u32 {
        WORK_AREA | u32::from(self.top) | (u32::from(self.bottom) << 12)
    }
    pub fn decode(message: u32, display_height: usize) -> Option<Self> {
        if message & 0xff000000 != WORK_AREA {
            return None;
        }
        let area = Self {
            top: (message & 4095) as u16,
            bottom: ((message >> 12) & 4095) as u16,
        };
        if usize::from(area.top) + usize::from(area.bottom) + 28 + 96 > display_height {
            return None;
        }
        Some(area)
    }
}
/// Register an already spawned legacy application. Large is a generic 640x480
/// source viewport, scaled into 488x288 content; ordinary legacy clients are cropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Managed {
    pub pid: u32,
    pub large: bool,
}
impl Managed {
    pub fn encode(self) -> Option<u32> {
        if self.pid == 0 || self.pid > PID_MASK {
            return None;
        }
        Some(MANAGE | self.pid | (u32::from(self.large) << 26))
    }
    pub fn decode(message: u32) -> Option<Self> {
        if message & 0xf8000000 != MANAGE {
            return None;
        }
        let request = Self {
            pid: message & PID_MASK,
            large: message & (1 << 26) != 0,
        };
        request
            .encode()
            .filter(|value| *value == message)
            .map(|_| request)
    }
}
/// Only the compositor-spawned shell may claim the desktop or manage other clients.
pub fn authorized(sender: u32, shell: Option<u32>) -> bool {
    sender != 0 && shell == Some(sender)
}
/// Packs full-display coordinates without the signed app-local coordinate limit.
pub fn pointer(x: u16, y: u16, buttons: u8) -> u32 {
    DESKTOP_POINTER
        | u32::from(x.min(4095))
        | (u32::from(y.min(4095)) << 12)
        | (u32::from(buttons & 7) << 24)
}
pub fn decode_pointer(value: u32) -> (i32, i32, u8) {
    (
        (value & 4095) as i32,
        ((value >> 12) & 4095) as i32,
        ((value >> 24) & 7) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_role_is_bound_to_spawned_shell_identity() {
        assert!(authorized(17, Some(17)));
        assert!(!authorized(18, Some(17)));
        assert!(!authorized(0, None));
    }
    #[test]
    fn work_area_reserves_room_for_a_decorated_minimum_window() {
        let area = WorkArea {
            top: 28,
            bottom: 32,
        };
        assert_eq!(WorkArea::decode(area.encode(), 480), Some(area));
        assert!(WorkArea::decode(0, 480).is_none());
        assert!(WorkArea::decode(area.encode() | (1 << 27), 480).is_none());
        assert!(WorkArea::decode(
            WorkArea {
                top: 400,
                bottom: 32
            }
            .encode(),
            480
        )
        .is_none());
        assert!(WorkArea::decode(area.encode(), 0).is_none());
    }
    #[test]
    fn legacy_registration_cannot_wrap_pids_or_flags() {
        for large in [false, true] {
            let request = Managed { pid: 19, large };
            assert_eq!(Managed::decode(request.encode().unwrap()), Some(request));
        }
        assert!(Managed {
            pid: 0,
            large: false
        }
        .encode()
        .is_none());
        assert!(Managed {
            pid: PID_MASK + 1,
            large: false
        }
        .encode()
        .is_none());
        assert!(Managed::decode(MANAGE | (1 << 27) | 1).is_none());
    }
    #[test]
    fn desktop_pointer_reaches_the_right_and_bottom_display_edges() {
        assert_eq!(decode_pointer(pointer(1919, 1079, 5)), (1919, 1079, 5));
        assert_eq!(decode_pointer(pointer(0, 0, 0)), (0, 0, 0));
    }
}
