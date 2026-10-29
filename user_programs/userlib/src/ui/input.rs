//! Widget input capture kept in the application, separate from WM hit-testing.
use super::raster::Rect;

#[derive(Default)]
pub struct Input {
    pub x: i32,
    pub y: i32,
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub active: Option<u32>,
    press_x: i32,
    press_y: i32,
}
impl Input {
    pub fn pointer(&mut self, x: i32, y: i32, down: bool) {
        if down && !self.down {
            self.press_x = x;
            self.press_y = y;
        }
        self.x = x;
        self.y = y;
        self.pressed |= down && !self.down;
        self.released |= !down && self.down;
        self.down = down;
    }
    pub fn button(&mut self, id: u32, bounds: Rect) -> bool {
        let hover = bounds.contains(self.x, self.y);
        if self.pressed && bounds.contains(self.press_x, self.press_y) && self.active.is_none() {
            self.active = Some(id);
        }
        self.released && hover && self.active == Some(id)
    }
    pub fn finish_frame(&mut self) {
        if self.released {
            self.active = None;
        }
        self.pressed = false;
        self.released = false;
    }
    pub fn unfocus(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn click_requires_press_and_release_on_the_same_widget() {
        let bounds = Rect::new(10, 10, 40, 30);
        let mut input = Input::default();
        input.pointer(20, 20, true);
        assert!(!input.button(7, bounds));
        input.finish_frame();
        input.pointer(20, 20, false);
        assert!(input.button(7, bounds));
        input.finish_frame();
        assert!(!input.button(7, bounds), "a click fires only once");
    }
    #[test]
    fn release_outside_or_focus_loss_cancels_activation() {
        let bounds = Rect::new(10, 10, 40, 30);
        let mut input = Input::default();
        input.pointer(20, 20, true);
        input.button(7, bounds);
        input.finish_frame();
        input.pointer(90, 90, false);
        assert!(!input.button(7, bounds));
        input.finish_frame();
        input.pointer(20, 20, true);
        input.button(7, bounds);
        input.unfocus();
        input.pointer(20, 20, false);
        assert!(!input.button(7, bounds));
    }
    #[test]
    fn coalesced_press_outside_and_release_inside_does_not_click() {
        let bounds = Rect::new(10, 10, 40, 30);
        let mut input = Input::default();
        input.pointer(90, 90, true);
        input.pointer(20, 20, false);
        assert!(!input.button(7, bounds));
    }
}
