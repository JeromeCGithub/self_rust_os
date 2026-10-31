//! Saved x86-64 context shared by timer and syscall assembly.
/// Saved CPU context layout.
///
/// Must match the register save order in the naked `timer_entry` and
/// `syscall_entry` handlers: pushes go rax, rbx, rcx, rdx, rsi, rdi, rbp,
/// r8, r9, r10, r11, r12, r13, r14, r15 (so r15 ends up at the lowest
/// address), followed by the CPU-pushed interrupt frame.
#[repr(C)]
pub struct SavedContext {
    /// R15 general-purpose register.
    pub r15: u64,
    /// R14 general-purpose register.
    pub r14: u64,
    /// R13 general-purpose register.
    pub r13: u64,
    /// R12 general-purpose register.
    pub r12: u64,
    /// R11 general-purpose register.
    pub r11: u64,
    /// R10 general-purpose register.
    pub r10: u64,
    /// R9 general-purpose register.
    pub r9: u64,
    /// R8 general-purpose register.
    pub r8: u64,
    /// Base pointer.
    pub rbp: u64,
    /// RDI general-purpose register.
    pub rdi: u64,
    /// RSI general-purpose register.
    pub rsi: u64,
    /// RDX general-purpose register.
    pub rdx: u64,
    /// RCX general-purpose register.
    pub rcx: u64,
    /// RBX general-purpose register.
    pub rbx: u64,
    /// RAX general-purpose register.
    pub rax: u64,
    /// Instruction pointer.
    pub rip: u64,
    /// Code segment selector.
    pub cs: u64,
    /// CPU flags.
    pub rflags: u64,
    /// Stack pointer.
    pub rsp: u64,
    /// Stack segment selector.
    pub ss: u64,
}

#[cfg(test)]
mod tests {
    use super::SavedContext;
    #[test_case]
    fn canonical_frame_matches_interrupt_assembly_offsets() {
        assert_eq!(core::mem::size_of::<SavedContext>(), 160);
        assert_eq!(core::mem::offset_of!(SavedContext, rax), 0x70);
        assert_eq!(core::mem::offset_of!(SavedContext, rip), 0x78);
        assert_eq!(core::mem::offset_of!(SavedContext, cs), 0x80);
        assert_eq!(core::mem::offset_of!(SavedContext, rsp), 0x90);
        assert_eq!(core::mem::offset_of!(SavedContext, ss), 0x98);
    }
}
