//! Select idle CPUs for ready work without broadcasting every wakeup.
pub(super) fn wake_targets(idle: u32, affinities: impl Iterator<Item = u32>) -> u32 {
    affinities.fold(0, |selected, affinity| {
        let available = affinity & idle & !selected;
        // Reserve one distinct eligible CPU for each runnable context. Wrapping
        // negation isolates its low bit and returns zero for an empty mask.
        selected | (available & available.wrapping_neg())
    })
}

#[cfg(test)]
mod tests {
    use super::wake_targets;

    #[test_case]
    fn one_ready_thread_wakes_only_one_idle_cpu() {
        assert_eq!(wake_targets(0xffff, [u32::MAX].into_iter()).count_ones(), 1);
    }

    #[test_case]
    fn wake_selection_respects_affinity_and_never_assigns_a_cpu_twice() {
        let targets = wake_targets(0b11010, [0b10000, 0b10000, 0b01010].into_iter());
        assert_eq!(targets, 0b10010);
        assert_eq!(wake_targets(0, [u32::MAX].into_iter()), 0);
        assert_eq!(wake_targets(0xff, core::iter::empty()), 0);
        assert_eq!(wake_targets(1, [2].into_iter()), 0);
    }
}
