//! Irreversible, process-wide worker principal. No host OS ABI is implied.
/// Drop ambient rights on a connected, single-threaded child.
pub const SYS_RESTRICT: u64 = 34;
/// Versioned restricted-worker profile identifier.
pub const WORKER: u64 = 1;
/// Minimum and maximum page-aligned heap budgets accepted by this profile.
pub const MIN_HEAP: u64 = 64 * 1024;
/// Maximum worker heap; initial image/stack/surface mappings are separate.
pub const MAX_HEAP: u64 = 64 * 1024 * 1024;
/// Validate scalar budget before changing any process state.
pub fn valid_budget(bytes: u64) -> bool {
    (MIN_HEAP..=MAX_HEAP).contains(&bytes) && bytes % 4096 == 0
}
/// Least-privilege allowlist: only private connected I/O, bounded heap, clocks,
/// entropy and single-thread progress. Future syscalls are denied by default.
pub fn permits(number: u64, first: u64) -> bool {
    match number {
        0 | 3 | 17 | 22 | 31 | 32 | 80 | 81 | 96 | 97 | 102 | 103 => true,
        109 => first != 0 && first & !6 == 0,
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_bounds_and_page_units_are_explicit() {
        for n in [0, MIN_HEAP - 1, MIN_HEAP + 1, MAX_HEAP + 4096, u64::MAX] {
            assert!(!valid_budget(n));
        }
        assert!(valid_budget(MIN_HEAP));
        assert!(valid_budget(MAX_HEAP));
    }
    #[test]
    fn private_transport_and_progress_do_not_grant_ambient_authority() {
        assert!(permits(22, 2));
        assert!(permits(31, 0));
        assert!(permits(109, 6));
        for n in [
            1,
            2,
            4,
            5,
            6,
            7,
            8,
            9,
            10,
            11,
            12,
            13,
            14,
            15,
            16,
            18,
            19,
            20,
            21,
            23,
            24,
            25,
            26,
            27,
            28,
            29,
            30,
            33,
            34,
            35,
            36,
            37,
            64,
            65,
            66,
            67,
            68,
            69,
            70,
            98,
            100,
            101,
            104,
            105,
            106,
            107,
            108,
            110,
            u64::MAX,
        ] {
            assert!(!permits(n, 0), "ambient syscall {}", n);
        }
        for events in [0, 1, 7, 8, 16, 32, u64::MAX] {
            assert!(!permits(109, events));
        }
    }
}
