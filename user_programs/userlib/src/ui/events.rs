//! Ordered delivery of events received during an internal frame wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Pointer {
        x: i32,
        y: i32,
        buttons: u8,
    },
    Focus(bool),
    CloseRequested,
    /// New content dimensions; safe drawing uses them immediately.
    Resized {
        width: u32,
        height: u32,
    },
}
pub(super) struct Pending {
    events: [Option<Event>; 64],
    start: usize,
    len: usize,
}
impl Pending {
    pub fn new() -> Self {
        Self {
            events: [None; 64],
            start: 0,
            len: 0,
        }
    }
    pub fn push(&mut self, event: Event) -> Result<(), ()> {
        if self.len == self.events.len() {
            return Err(());
        }
        self.events[(self.start + self.len) % self.events.len()] = Some(event);
        self.len += 1;
        Ok(())
    }
    pub fn pop(&mut self) -> Option<Event> {
        if self.len == 0 {
            return None;
        }
        let event = self.events[self.start].take();
        self.start = (self.start + 1) % self.events.len();
        self.len -= 1;
        event
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_wait_preserves_press_release_focus_and_close_in_order() {
        let mut queue = Pending::new();
        let events = [
            Event::Pointer {
                x: 20,
                y: 42,
                buttons: 1,
            },
            Event::Pointer {
                x: 20,
                y: 42,
                buttons: 0,
            },
            Event::Focus(false),
            Event::Resized {
                width: 1920,
                height: 992,
            },
            Event::CloseRequested,
        ];
        for event in events {
            queue.push(event).unwrap();
        }
        for event in events {
            assert_eq!(queue.pop(), Some(event));
        }
        assert_eq!(queue.pop(), None);
    }
    #[test]
    fn capacity_failure_is_explicit_and_wraparound_preserves_queued_events() {
        let mut queue = Pending::new();
        for _ in 0..64 {
            queue.push(Event::Focus(true)).unwrap();
        }
        assert!(queue.push(Event::CloseRequested).is_err());
        assert_eq!(queue.pop(), Some(Event::Focus(true)));
        queue.push(Event::CloseRequested).unwrap();
        for _ in 0..63 {
            assert_eq!(queue.pop(), Some(Event::Focus(true)));
        }
        assert_eq!(queue.pop(), Some(Event::CloseRequested));
    }
}
