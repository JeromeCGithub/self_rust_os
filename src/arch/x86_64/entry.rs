//! Canonical interrupt/syscall entry and coarse kernel serialization.
//! The gate covers all ordinary kernel entry, and is released on the incoming
//! stack after a switch. User instructions execute concurrently on every CPU.
//! NMI shootdown bypasses the gate and acquires no locks.
use crate::scheduler::{self, SavedContext};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
static SERVING: AtomicUsize = AtomicUsize::new(0);
static OWNER: AtomicUsize = AtomicUsize::new(usize::MAX);
static TICKETS: [AtomicUsize; 32] = [const { AtomicUsize::new(usize::MAX) }; 32];
// Kernel IRQs cannot switch a service's live stack/guards. Remember their
// request until syscall return, otherwise repeated PIO calls can consume every
// timer interrupt inside the kernel and starve siblings on a single CPU.
// Only the owning CPU touches its slot; Relaxed carries no associated payload.
static RESCHEDULE: [AtomicBool; 32] = [const { AtomicBool::new(false) }; 32];
// Publish before halting, clear under entry before user dispatch. A BSP PIT
// interrupt can request timer-wait service without acquiring scheduler locks.
static IDLE_CPUS: AtomicU32 = AtomicU32::new(0);

/// Enter with local maskable interrupts disabled. No blocking while held.
pub extern "C" fn enter() {
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    let cpu = super::smp::current_cpu();
    TICKETS[cpu].store(ticket, Ordering::SeqCst);
    while SERVING.load(Ordering::Acquire) != ticket {
        // No service locks or user references exist before entry. While waiting
        // for our ticket, IRQs may acknowledge devices and queue pending input;
        // they never acquire this gate. The unlock IPI and STI shadow prevent a
        // lost wakeup. HLT avoids 31 busy vCPUs starving QEMU's PIO device work.
        x86_64::instructions::interrupts::enable_and_hlt();
        x86_64::instructions::interrupts::disable();
    }
    TICKETS[cpu].store(usize::MAX, Ordering::Release);
    OWNER.store(cpu, Ordering::Release);
}
/// Release after the outgoing thread's stack is no longer executing.
pub extern "C" fn leave() {
    super::smp::wake_idle();
    OWNER.store(usize::MAX, Ordering::Release);
    let next = SERVING.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    // Only the next owner needs a wake, preventing an IPI thundering herd with
    // 32 waiting CPUs. SeqCst publication pairs with each waiter before HLT.
    for (cpu, ticket) in TICKETS.iter().enumerate() {
        if ticket.load(Ordering::Acquire) == next {
            super::smp::wake_cpus(1u32 << cpu);
            break;
        }
    }
}

/// Run service-owned work without scheduler/mapping serialization. The closure
/// borrows only owned kernel storage and releases service locks before return.
/// CPU ownership keeps the syscall's process and stack alive while entry is free.
pub fn without_gate<R>(work: impl FnOnce() -> R) -> R {
    if OWNER.load(Ordering::Acquire) != super::smp::current_cpu() {
        return work();
    }
    leave();
    // Device IRQs never take filesystem/allocator/entry locks. Kernel-mode
    // timer IRQs acknowledge without scheduling, so the service guard stays on
    // this CPU while PIT/input continue progressing even when the BSP does PIO.
    x86_64::instructions::interrupts::enable();
    let result = work();
    x86_64::instructions::interrupts::disable();
    enter();
    result
}

extern "C" fn dispatch(saved_rsp: u64, source: u64) -> u64 {
    // SAFETY: Assembly supplies a valid aligned canonical saved frame on this
    // CPU's live kernel stack; the frame is accessed only during this entry.
    let from_user = unsafe { (*(saved_rsp as *const SavedContext)).cs & 3 == 3 };
    if source != 0 {
        if source == 1 {
            crate::interrupts::advance_ticks();
        } else {
            super::apic::eoi();
        }
        if !from_user {
            RESCHEDULE[super::smp::current_cpu()].store(true, Ordering::Relaxed);
            if source == 1 {
                let idle = IDLE_CPUS.load(Ordering::Acquire);
                let secondary = idle & !1;
                // An idle BSP services waits itself. During BSP kernel work,
                // ask one idle AP to process deadlines at the public tick rate.
                if idle & 1 == 0 && secondary != 0 {
                    super::smp::wake_cpus(1 << secondary.trailing_zeros());
                }
            }
            return saved_rsp;
        }
    }
    enter();
    scheduler::wake_timers();
    if source == 0 {
        if !scheduler::current_is_live() {
            return scheduler::switch_next(0);
        }
        // SAFETY: No user reference is formed; this is the CPU-owned kernel
        // register frame, fully initialized by entry assembly.
        let (number, a, b, c) = unsafe {
            let f = &*(saved_rsp as *const SavedContext);
            (f.rax, f.rdi, f.rsi, f.rdx)
        };
        let result = crate::userspace::syscall::syscall_dispatch(number, a, b, c);
        if result == u64::MAX {
            return scheduler::switch_next(0);
        }
        if result == scheduler::PROCESS_BLOCK_SENTINEL {
            return scheduler::switch_next(saved_rsp);
        }
        // Block/yield deliberately changed Running to Blocked/Ready and must
        // save/normalize their frame above. Only an ordinary returning syscall
        // needs this stopped-during-service check after reacquiring entry.
        if !scheduler::current_is_live() {
            return scheduler::switch_next(0);
        }
        // SAFETY: Exclusive kernel frame as above; update only the return value.
        unsafe {
            (*(saved_rsp as *mut SavedContext)).rax = result;
        }
        if RESCHEDULE[super::smp::current_cpu()].swap(false, Ordering::Relaxed) {
            timer_next(saved_rsp)
        } else {
            saved_rsp
        }
    } else {
        RESCHEDULE[super::smp::current_cpu()].store(false, Ordering::Relaxed);
        timer_next(saved_rsp)
    }
}

fn timer_next(saved_rsp: u64) -> u64 {
    let next = scheduler::on_timer(saved_rsp);
    if next == scheduler::IDLE_RSP {
        0
    } else if next == 0 {
        saved_rsp
    } else {
        next
    }
}

/// Restore a selected user frame. The gate remains held until the stack switch.
#[naked]
pub unsafe extern "C" fn restore(_rsp: u64) -> ! {
    // SAFETY: Caller owns a live selected SavedContext, has set CR3/TSS, and
    // holds the gate. Switching first makes the old stack safe to reclaim.
    unsafe {
        core::arch::naked_asm!(
                "mov rsp,rdi",
                "call {leave}",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rbp",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rbx",
                "pop rax",
                "iretq", leave=sym leave,
        );
    }
}

/// Enter the CPU's dedicated idle stack with the gate held.
#[naked]
unsafe extern "C" fn switch_idle() -> ! {
    // SAFETY: CPU-lifetime idle stack is allocated and 16-byte aligned. The
    // caller holds the gate until this switch removes its outgoing stack.
    unsafe {
        core::arch::naked_asm!(
            "and rsp,-16", "call {stack}", "mov rsp,rax", "call {run}", "ud2",
            stack=sym super::smp::idle_stack, run=sym idle_loop,
        );
    }
}
extern "C" fn idle_loop() -> ! {
    let cpu = super::smp::current_cpu();
    IDLE_CPUS.fetch_or(1 << cpu, Ordering::Release);
    if cpu != 0 && super::apic::available() {
        super::apic::idle_timer(true);
    }
    leave();
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
        x86_64::instructions::interrupts::disable();
        enter();
        let next = scheduler::idle_next();
        if next != 0 {
            IDLE_CPUS.fetch_and(!(1 << cpu), Ordering::AcqRel);
            if cpu != 0 && super::apic::available() {
                super::apic::idle_timer(false);
            }
            // SAFETY: idle_next selected a live context under the entry gate.
            unsafe {
                restore(next);
            }
        }
        leave();
    }
}
/// Start an AP on its own idle stack, with no current user context.
pub fn idle() -> ! {
    x86_64::instructions::interrupts::disable();
    enter();
    // SAFETY: smp::start allocated the current CPU's lifetime idle stack.
    unsafe {
        switch_idle();
    }
}

/// Architecture entry stub; all variants share the same register layout.
#[naked]
pub extern "C" fn syscall_entry() {
    // SAFETY: Save every GPR before Rust, align the call stack, then restore the
    // complete canonical frame selected by dispatch. Kernel interrupts bypass
    // gate release; user returns release it only on the incoming stack.
    unsafe {
        core::arch::naked_asm!(
                "push rax",
                "push rbx",
                "push rcx",
                "push rdx",
                "push rsi",
                "push rdi",
                "push rbp",
                "push r8",
                "push r9",
                "push r10",
                "push r11",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                "cld", "mov rdi,rsp", "and rsp,-16", "mov esi,0",
                "call {dispatch}", "test rax,rax", "jz 2f", "mov rsp,rax",
                "test byte ptr [rsp+0x80],3", "jz 3f", "call {leave}", "3:",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rbp",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rbx",
                "pop rax",
                "iretq", "2:", "jmp {idle}",
                dispatch=sym dispatch, leave=sym leave, idle=sym switch_idle,
        );
    }
}

/// Architecture entry stub; all variants share the same register layout.
#[naked]
pub extern "C" fn timer_entry() {
    // SAFETY: Save every GPR before Rust, align the call stack, then restore the
    // complete canonical frame selected by dispatch. Kernel interrupts bypass
    // gate release; user returns release it only on the incoming stack.
    unsafe {
        core::arch::naked_asm!(
                "push rax",
                "push rbx",
                "push rcx",
                "push rdx",
                "push rsi",
                "push rdi",
                "push rbp",
                "push r8",
                "push r9",
                "push r10",
                "push r11",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                "cld", "mov rdi,rsp", "and rsp,-16", "mov esi,1",
                "call {dispatch}", "test rax,rax", "jz 2f", "mov rsp,rax",
                "test byte ptr [rsp+0x80],3", "jz 3f", "call {leave}", "3:",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rbp",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rbx",
                "pop rax",
                "iretq", "2:", "jmp {idle}",
                dispatch=sym dispatch, leave=sym leave, idle=sym switch_idle,
        );
    }
}

/// Architecture entry stub; all variants share the same register layout.
#[naked]
pub extern "C" fn local_timer_entry() {
    // SAFETY: Save every GPR before Rust, align the call stack, then restore the
    // complete canonical frame selected by dispatch. Kernel interrupts bypass
    // gate release; user returns release it only on the incoming stack.
    unsafe {
        core::arch::naked_asm!(
                "push rax",
                "push rbx",
                "push rcx",
                "push rdx",
                "push rsi",
                "push rdi",
                "push rbp",
                "push r8",
                "push r9",
                "push r10",
                "push r11",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                "cld", "mov rdi,rsp", "and rsp,-16", "mov esi,2",
                "call {dispatch}", "test rax,rax", "jz 2f", "mov rsp,rax",
                "test byte ptr [rsp+0x80],3", "jz 3f", "call {leave}", "3:",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rbp",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rbx",
                "pop rax",
                "iretq", "2:", "jmp {idle}",
                dispatch=sym dispatch, leave=sym leave, idle=sym switch_idle,
        );
    }
}

/// Architecture entry stub; all variants share the same register layout.
#[naked]
pub extern "C" fn reschedule_entry() {
    // SAFETY: Save every GPR before Rust, align the call stack, then restore the
    // complete canonical frame selected by dispatch. Kernel interrupts bypass
    // gate release; user returns release it only on the incoming stack.
    unsafe {
        core::arch::naked_asm!(
                "push rax",
                "push rbx",
                "push rcx",
                "push rdx",
                "push rsi",
                "push rdi",
                "push rbp",
                "push r8",
                "push r9",
                "push r10",
                "push r11",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                "cld", "mov rdi,rsp", "and rsp,-16", "mov esi,3",
                "call {dispatch}", "test rax,rax", "jz 2f", "mov rsp,rax",
                "test byte ptr [rsp+0x80],3", "jz 3f", "call {leave}", "3:",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rbp",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rbx",
                "pop rax",
                "iretq", "2:", "jmp {idle}",
                dispatch=sym dispatch, leave=sym leave, idle=sym switch_idle,
        );
    }
}

extern "C" fn fault_dispatch(saved: u64, error: u64, vector: u64) -> u64 {
    // SAFETY: Fault assembly normalizes the hardware error-code frame to the
    // same complete, aligned GPR/iret frame used by ordinary entry.
    let user = unsafe { (*(saved as *const SavedContext)).cs & 3 == 3 };
    let address = x86_64::registers::control::Cr2::read().as_u64();
    if !user {
        panic!(
            "kernel fault vector={} error={:#x} address={:#x}",
            vector, error, address
        );
    }
    enter();
    crate::serial_println!(
        "[fault] pid={:?} vector={} error={:#x} address={:#x}",
        scheduler::current_pid(),
        vector,
        error,
        address
    );
    scheduler::exit_current(vector);
    scheduler::switch_next(0)
}

/// User page faults stop the process; supervisor page faults remain fatal.
#[naked]
pub extern "C" fn page_fault_entry() {
    // SAFETY: Exchange the CPU error word for original RAX, then push the other
    // 14 registers. This produces exactly SavedContext without losing a GPR.
    // The error is passed separately; the dead faulting context is never resumed.
    unsafe {
        core::arch::naked_asm!(
            "xchg rax,[rsp]", "push rbx", "push rcx", "push rdx", "push rsi", "push rdi", "push rbp",
            "push r8", "push r9", "push r10", "push r11", "push r12", "push r13", "push r14", "push r15",
            "cld", "mov rdi,rsp", "mov rsi,rax", "mov edx,14", "and rsp,-16", "call {dispatch}",
            "test rax,rax", "jz 2f", "mov rdi,rax", "jmp {restore}", "2:", "jmp {idle}",
            dispatch=sym fault_dispatch,restore=sym restore,idle=sym switch_idle,
        );
    }
}
/// Privileged instructions and invalid segment operations in Ring 3 stop its PID.
#[naked]
pub extern "C" fn general_protection_entry() {
    // SAFETY: Same hardware-error-code normalization and handoff as page fault.
    unsafe {
        core::arch::naked_asm!(
            "xchg rax,[rsp]", "push rbx", "push rcx", "push rdx", "push rsi", "push rdi", "push rbp",
            "push r8", "push r9", "push r10", "push r11", "push r12", "push r13", "push r14", "push r15",
            "cld", "mov rdi,rsp", "mov rsi,rax", "mov edx,13", "and rsp,-16", "call {dispatch}",
            "test rax,rax", "jz 2f", "mov rdi,rax", "jmp {restore}", "2:", "jmp {idle}",
            dispatch=sym fault_dispatch,restore=sym restore,idle=sym switch_idle,
        );
    }
}
