//! Compositor geometry policy, independent of IPC and framebuffer ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
}
#[derive(Clone, Copy)]
pub struct Workspace {
    pub width: usize,
    pub height: usize,
    pub top: usize,
    pub bottom: usize,
}
pub const TITLE: usize = 28;
pub const LEFT: u8 = 1;
pub const RIGHT: u8 = 2;
pub const TOP: u8 = 4;
pub const BOTTOM: u8 = 8;
impl Workspace {
    pub fn maximized(self) -> Bounds {
        Bounds {
            x: 0,
            y: self.top as i32,
            width: self.width,
            height: self.height.saturating_sub(self.top + self.bottom + TITLE),
        }
    }
    pub fn resized(
        self,
        start: Bounds,
        edges: u8,
        dx: i32,
        dy: i32,
        minimum: (usize, usize),
    ) -> Bounds {
        let (mut left, mut top) = (i64::from(start.x), i64::from(start.y));
        let (mut right, mut bottom) = (
            left + start.width as i64,
            top + (start.height + TITLE) as i64,
        );
        let (min_w, min_h) = (minimum.0 as i64, (minimum.1 + TITLE) as i64);
        if edges & LEFT != 0 {
            left = (left + i64::from(dx)).clamp(0, right - min_w);
        }
        if edges & RIGHT != 0 {
            right = (right + i64::from(dx)).clamp(left + min_w, self.width as i64);
        }
        if edges & TOP != 0 {
            top = (top + i64::from(dy)).clamp(self.top as i64, bottom - min_h);
        }
        if edges & BOTTOM != 0 {
            bottom =
                (bottom + i64::from(dy)).clamp(top + min_h, (self.height - self.bottom) as i64);
        }
        Bounds {
            x: left as i32,
            y: top as i32,
            width: (right - left) as usize,
            height: (bottom - top) as usize - TITLE,
        }
    }
}
pub fn edges(bounds: Bounds, x: i32, y: i32) -> u8 {
    if x < bounds.x
        || y < bounds.y
        || x >= bounds.x + bounds.width as i32
        || y >= bounds.y + (bounds.height + TITLE) as i32
    {
        return 0;
    }
    let mut edge = 0;
    if x < bounds.x + 5 {
        edge |= LEFT;
    } else if x >= bounds.x + bounds.width as i32 - 5 {
        edge |= RIGHT;
    }
    if y < bounds.y + 5 {
        edge |= TOP;
    } else if y >= bounds.y + (bounds.height + TITLE) as i32 - 5 {
        edge |= BOTTOM;
    }
    edge
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corners_resize_with_fixed_opposite_edges_and_workspace_clipping() {
        let work = Workspace {
            width: 1920,
            height: 1080,
            top: 28,
            bottom: 32,
        };
        let start = Bounds {
            x: 100,
            y: 120,
            width: 640,
            height: 400,
        };
        assert_eq!(
            work.resized(start, RIGHT | BOTTOM, 320, 200, (320, 200)),
            Bounds {
                width: 960,
                height: 600,
                ..start
            }
        );
        assert_eq!(
            work.resized(start, LEFT | TOP, 40, 30, (320, 200)),
            Bounds {
                x: 140,
                y: 150,
                width: 600,
                height: 370
            }
        );
        let large = work.resized(start, LEFT | TOP, i32::MIN, i32::MIN, (320, 200));
        assert_eq!(
            large,
            Bounds {
                x: 0,
                y: 28,
                width: 740,
                height: 492
            }
        );
        let small = work.resized(start, RIGHT | BOTTOM, i32::MIN, i32::MIN, (320, 200));
        assert_eq!((small.width, small.height), (320, 200));
        let full = work.resized(start, RIGHT | BOTTOM, i32::MAX, i32::MAX, (320, 200));
        assert_eq!(
            (
                full.x + full.width as i32,
                full.y + (full.height + TITLE) as i32
            ),
            (1920, 1048)
        );
        assert_eq!(
            work.maximized(),
            Bounds {
                x: 0,
                y: 28,
                width: 1920,
                height: 992
            }
        );
        assert_eq!(edges(start, 739, 547), RIGHT | BOTTOM);
        assert_eq!(edges(start, 100, 120), LEFT | TOP);
        assert_eq!(edges(start, 300, 134), 0);
        assert_eq!(edges(start, 740, 548), 0);
    }
}

/// An old immutable frame keeps its old size while an offer is outstanding.
/// Only a published offer can authorize a different frame geometry.
pub fn frame_bounds(
    current: Bounds,
    offered: Option<Bounds>,
    sent: bool,
    size: (usize, usize),
) -> Option<Bounds> {
    if sent {
        if let Some(bounds) = offered {
            if (bounds.width, bounds.height) == size {
                return Some(bounds);
            }
        }
    }
    if (current.width, current.height) == size {
        Some(current)
    } else {
        None
    }
}
#[cfg(test)]
mod frame_tests {
    use super::*;
    #[test]
    fn outstanding_old_frames_cannot_be_reinterpreted_at_an_offered_size() {
        let old = Bounds {
            x: 92,
            y: 128,
            width: 640,
            height: 400,
        };
        let next = Bounds {
            x: 0,
            y: 28,
            width: 1920,
            height: 992,
        };
        assert_eq!(frame_bounds(old, Some(next), true, (640, 400)), Some(old));
        assert_eq!(frame_bounds(old, Some(next), true, (1920, 992)), Some(next));
        assert_eq!(frame_bounds(old, Some(next), false, (1920, 992)), None);
        assert_eq!(frame_bounds(old, None, true, (1920, 992)), None);
        assert_eq!(frame_bounds(old, Some(next), true, (960, 600)), None);
    }
}

/// Reservation changes cannot invalidate an already published resize offer.
pub fn accepts_reservation(work: Workspace, height: usize, pending: bool) -> bool {
    !pending
        && height
            .checked_add(TITLE)
            .and_then(|n| n.checked_add(work.top))
            .and_then(|n| n.checked_add(work.bottom))
            .is_some_and(|n| n <= work.height)
}
#[cfg(test)]
mod reservation_tests {
    use super::*;
    #[test]
    fn reservations_wait_for_inflight_geometry_and_keep_a_valid_decorated_window() {
        let work = Workspace {
            width: 1920,
            height: 1080,
            top: 28,
            bottom: 32,
        };
        assert!(accepts_reservation(work, 992, false));
        assert!(!accepts_reservation(work, 993, false));
        assert!(!accepts_reservation(work, 400, true));
        assert!(!accepts_reservation(work, usize::MAX, false));
    }
}

/// Desired, offered and committed expansion state have separate lifetimes.
#[derive(Clone, Copy, Default)]
pub struct Expansion {
    desired: Option<Bounds>,
    offered: Option<Bounds>,
    applied: Option<Bounds>,
}
impl Expansion {
    pub fn toggle(&mut self, current: Bounds, maximum: Bounds) -> Bounds {
        if let Some(saved) = self.desired.take() {
            saved
        } else {
            self.desired = Some(self.applied.unwrap_or(current));
            maximum
        }
    }
    pub fn offered(&mut self) {
        self.offered = self.desired;
    }
    pub fn committed(&mut self) {
        self.applied = self.offered;
    }
    pub fn refused(&mut self) {
        self.desired = self.applied;
    }
    pub fn expanded(&self) -> bool {
        self.applied.is_some()
    }
}
#[cfg(test)]
mod expansion_tests {
    use super::*;
    #[test]
    fn failed_allocations_restore_the_previous_mode_and_restore_rectangle() {
        let normal = Bounds {
            x: 100,
            y: 120,
            width: 640,
            height: 400,
        };
        let max = Workspace {
            width: 1920,
            height: 1080,
            top: 28,
            bottom: 32,
        }
        .maximized();
        let mut mode = Expansion::default();
        assert_eq!(mode.toggle(normal, max), max);
        mode.refused();
        assert!(!mode.expanded());
        assert_eq!(mode.toggle(normal, max), max);
        mode.offered();
        mode.committed();
        assert!(mode.expanded());
        assert_eq!(mode.toggle(max, max), normal);
        mode.refused();
        assert!(mode.expanded());
        assert_eq!(mode.toggle(max, max), normal);
        mode.offered();
        mode.committed();
        assert!(!mode.expanded());
    }
    #[test]
    fn rapid_restore_does_not_change_the_mode_of_an_inflight_maximize_frame() {
        let normal = Bounds {
            x: 100,
            y: 120,
            width: 640,
            height: 400,
        };
        let max = Workspace {
            width: 1920,
            height: 1080,
            top: 28,
            bottom: 32,
        }
        .maximized();
        let mut mode = Expansion::default();
        assert_eq!(mode.toggle(normal, max), max);
        mode.offered();
        assert_eq!(mode.toggle(normal, max), normal); // Coalesced while max frame is in flight.
        mode.committed();
        assert!(mode.expanded());
        mode.offered();
        mode.committed();
        assert!(!mode.expanded());
    }
}
