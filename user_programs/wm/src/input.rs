//! Preserve sampled button edges while composition coalesces pointer motion.
type Sample = (i32, i32, u8);
const CAPACITY: usize = 32;

pub(crate) struct PointerInput {
    edges: [Sample; CAPACITY],
    head: usize,
    len: usize,
    latest: Sample,
}

impl PointerInput {
    pub(crate) const fn new() -> Self {
        Self {
            edges: [(0, 0, 0); CAPACITY],
            head: 0,
            len: 0,
            latest: (0, 0, 0),
        }
    }

    pub(crate) fn observe(&mut self, sample: Sample) {
        if sample.2 != self.latest.2 {
            if self.len == CAPACITY {
                // Retain the oldest edges and reconcile to the newest state if
                // a client stalls us for more than a bounded burst of clicks.
                self.edges[(self.head + self.len - 1) % CAPACITY] = sample;
            } else {
                self.edges[(self.head + self.len) % CAPACITY] = sample;
                self.len += 1;
            }
        }
        self.latest = sample;
    }

    pub(crate) fn next(&mut self) -> Sample {
        if self.len == 0 {
            return self.latest;
        }
        let sample = self.edges[self.head];
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        sample
    }

    pub(crate) fn pending(&self) -> bool {
        self.len != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_during_composition_keeps_both_edges_and_positions() {
        let mut input = PointerInput::new();
        input.observe((20, 30, 1));
        input.observe((80, 90, 1));
        input.observe((100, 110, 0));
        assert!(input.pending());
        assert_eq!(input.next(), (20, 30, 1));
        assert_eq!(input.next(), (100, 110, 0));
        assert!(!input.pending());
        assert_eq!(input.next(), (100, 110, 0));
    }

    #[test]
    fn motion_coalesces_without_duplicate_button_edges() {
        let mut input = PointerInput::new();
        for x in 0..100 {
            input.observe((x, 50, 0));
        }
        assert!(!input.pending());
        assert_eq!(input.next(), (99, 50, 0));
        input.observe((100, 50, 3));
        input.observe((101, 50, 3));
        assert_eq!(input.next(), (100, 50, 3));
        assert!(!input.pending());
        assert_eq!(input.next(), (101, 50, 3));
    }

    #[test]
    fn overflow_is_bounded_and_reconciles_to_latest_buttons() {
        let mut input = PointerInput::new();
        for x in 0..100 {
            input.observe((x, x, ((x + 1) % 2) as u8));
        }
        assert_eq!(input.next(), (0, 0, 1));
        let mut drained = 1;
        let mut last = (0, 0, 1);
        while input.pending() {
            last = input.next();
            drained += 1;
        }
        assert_eq!(drained, CAPACITY);
        assert_eq!(last, (99, 99, 0));
        assert_eq!(input.next(), last);
        input.observe((101, 101, 1));
        assert_eq!(input.next(), (101, 101, 1));
        assert!(!input.pending());
    }
}
