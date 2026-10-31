//! Versioned native threading ABI shared by Ring 0 and Ring 3.
#![no_std]
/// Online logical CPU count.
pub const SYS_CPU_COUNT: u64 = 96;
/// Current dense logical CPU index.
pub const SYS_CURRENT_CPU: u64 = 97;
/// Create from a versioned request record.
pub const SYS_THREAD_CREATE: u64 = 98;
/// Exit only the calling thread.
pub const SYS_THREAD_EXIT: u64 = 99;
/// Join one same-process thread.
pub const SYS_THREAD_JOIN: u64 = 100;
/// Detach one same-process thread.
pub const SYS_THREAD_DETACH: u64 = 101;
/// Yield the CPU.
pub const SYS_THREAD_YIELD: u64 = 102;
/// Current thread identifier.
pub const SYS_THREAD_CURRENT: u64 = 103;
/// Compare and park on an aligned process-private atomic word.
pub const SYS_THREAD_WAIT: u64 = 104;
/// Wake up to the specified number of process-private word waiters.
pub const SYS_THREAD_WAKE: u64 = 105;
/// Current process's mapped thread-stack pages (diagnostic).
pub const SYS_THREAD_PAGES: u64 = 106;
/// Current reusable physical-frame allocation count (diagnostic).
pub const SYS_FRAME_COUNT: u64 = 107;
/// Wait until the calling process's mailbox is nonempty or a deadline expires.
pub const SYS_MESSAGE_WAIT: u64 = 108;
/// ABI version.
pub const VERSION: u32 = 1;
/// Default worker stack, excluding the guard page.
pub const DEFAULT_STACK: u64 = 256 * 1024;
/// Maximum worker stack.
pub const MAX_STACK: u64 = DEFAULT_STACK;
/// First worker-stack arena address. Existing main stack/surface stay unchanged.
pub const STACK_ARENA: u64 = 0x4000000;
/// Per-stack reservation includes a permanently unmapped guard.
pub const STACK_STRIDE: u64 = MAX_STACK + 4096;
/// Worker stack slots per process (plus one initial thread).
pub const STACK_SLOTS: usize = 63;
/// End of the private user region, including the worker-stack arena.
pub const USER_END: u64 = 0x6000000;
/// Creation request; no padding, references or kernel addresses.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Create {
    /// Must be VERSION.
    pub version: u32,
    /// Reserved, must be zero.
    pub flags: u32,
    /// User-space runtime entry address.
    pub entry: u64,
    /// Opaque user-space argument consumed by the runtime.
    pub argument: u64,
    /// Requested usable bytes, page-rounded, 16 KiB through MAX_STACK.
    pub stack_size: u64,
    /// Allowed CPU bits, zero means all online CPUs.
    pub affinity: u64,
}
/// Validate pure creation parameters before inspecting user mappings.
pub fn valid(request: &Create, cpus: usize) -> bool {
    (1..=32).contains(&cpus)
        && request.version == VERSION
        && request.flags == 0
        && (16384..=MAX_STACK).contains(&request.stack_size)
        && request.entry >= 0x400000
        && request.entry < USER_END
        && request.affinity & !((1u64 << cpus) - 1) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Create {
        Create {
            version: VERSION,
            flags: 0,
            entry: 0x400000,
            argument: 0,
            stack_size: DEFAULT_STACK,
            affinity: 0,
        }
    }
    #[test]
    fn wire_layout_stack_arena_and_supported_cpus() {
        assert_eq!(core::mem::size_of::<Create>(), 40);
        assert_eq!(core::mem::offset_of!(Create, affinity), 32);
        assert!(STACK_ARENA + STACK_STRIDE * STACK_SLOTS as u64 <= USER_END);
        for cpus in 1..=32 {
            let mut r = request();
            r.affinity = 1u64 << (cpus - 1);
            assert!(valid(&r, cpus));
            r.affinity = 1u64 << cpus;
            assert!(!valid(&r, cpus));
        }
    }
    #[test]
    fn rejects_versions_flags_stack_boundaries_and_invalid_cpu_counts() {
        let mut r = request();
        for size in [0, 16383, MAX_STACK + 1, u64::MAX] {
            r.stack_size = size;
            assert!(!valid(&r, 8));
        }
        r = request();
        r.version = 0;
        assert!(!valid(&r, 8));
        r = request();
        r.flags = 1;
        assert!(!valid(&r, 8));
        assert!(!valid(&request(), 0));
        assert!(!valid(&request(), 33));
    }
}
