//! Eager x87/MMX/SSE2 ownership and FS-based thread-pointer state.
//! Kernel code stays soft-float; no kernel path borrows userspace SIMD registers.
use x86_64::registers::{
    control::{Cr0, Cr0Flags, Cr4, Cr4Flags},
    model_specific::Msr,
};
/// Hardware FXSAVE64 image, including x87, MXCSR and XMM0..15.
#[repr(C, align(16))]
pub struct State([u8; 512]);
impl State {
    /// Empty x87 stack, masked exceptions, round-to-nearest and zero XMM state.
    pub const fn new() -> Self {
        let mut bytes = [0; 512];
        bytes[0] = 0x7f;
        bytes[1] = 3;
        bytes[24] = 0x80;
        bytes[25] = 0x1f;
        Self(bytes)
    }
    /// # Safety
    /// This CPU has completed init_cpu. The caller owns this aligned state and
    /// the outgoing thread under scheduler/entry serialization; kernel code
    /// cannot use SIMD before or after this snapshot.
    pub unsafe fn save(&mut self) {
        // SAFETY: Exact aligned 512-byte exclusively owned initialized storage;
        // FXSAVE64 writes only that image and retains no pointer or reference.
        unsafe {
            core::arch::asm!("fxsave64 [{}]",in(reg)self.0.as_mut_ptr(),options(nostack,preserves_flags));
        }
    }
    /// # Safety
    /// init_cpu ran; image is either new() or a hardware snapshot for the
    /// incoming thread. Kernel target disables floating-point/SIMD codegen.
    pub unsafe fn restore(&self) {
        // SAFETY: Aligned complete valid FXSAVE64 image lives through the read;
        // no concurrent writer can modify it under scheduler/entry ownership.
        unsafe {
            core::arch::asm!("fxrstor64 [{}]",in(reg)self.0.as_ptr(),options(readonly,nostack,preserves_flags));
        }
    }
}
/// Enable the guaranteed x86-64 x87/FXSR/SSE2 baseline on each logical CPU.
/// AVX/XSAVE are intentionally not exposed until their larger state is managed.
pub fn init_cpu() {
    // SAFETY: CPUID is supported on the declared x86-64 platform; no pointers.
    let features = unsafe { core::arch::x86_64::__cpuid(1) }.edx;
    assert_eq!(
        features & ((1 << 0) | (1 << 24) | (1 << 25) | (1 << 26)),
        (1 << 0) | (1 << 24) | (1 << 25) | (1 << 26),
        "x87/FXSR/SSE2 required"
    );
    // SAFETY: Privileged per-CPU setup before user execution. Preserve unrelated
    // paging/protection bits, disable lazy trapping, and enable exact state the
    // scheduler saves. OSXSAVE stays off so unsupported AVX state cannot leak.
    unsafe {
        Cr0::update(|flags| {
            flags.remove(Cr0Flags::EMULATE_COPROCESSOR | Cr0Flags::TASK_SWITCHED);
            flags.insert(Cr0Flags::MONITOR_COPROCESSOR | Cr0Flags::NUMERIC_ERROR);
        });
        Cr4::update(|flags| {
            flags.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
            flags.remove(Cr4Flags::OSXSAVE);
        });
        State::new().restore();
        set_fs(0);
    }
}
/// # Safety
/// Caller runs privileged on the owning CPU; returned value is scalar state.
pub unsafe fn fs() -> u64 {
    unsafe { Msr::new(0xc0000100).read() }
}
/// # Safety
/// Only a canonical user address (or zero) owned by the incoming thread may be
/// installed; kernel code never accesses FS-relative memory or compiler TLS.
pub unsafe fn set_fs(address: u64) {
    unsafe {
        Msr::new(0xc0000100).write(address);
    }
}
