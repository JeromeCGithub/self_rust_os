//! User space support module.
//!
//! This module provides the infrastructure to load and execute user-mode binaries
//! in Ring 3. It includes:
//! - A syscall interface via `int 0x80` for user programs to request kernel services.
//! - A process loader that maps a flat binary into user-accessible pages.
//! - A mechanism to switch from kernel mode (Ring 0) to user mode (Ring 3).

pub mod process;
pub mod syscall;

/// Base virtual address where user program code is loaded.
pub const USER_CODE_START: u64 = 0x40_0000;

/// Top of the user-mode stack (stack grows downward).
pub const USER_STACK_TOP: u64 = 0x80_0000;

/// Size of the user-mode stack in bytes (16 KiB).
pub const USER_STACK_SIZE: u64 = 4096 * 4;

/// Bottom of the user-mode stack.
pub const USER_STACK_BOTTOM: u64 = USER_STACK_TOP - USER_STACK_SIZE;

/// Interrupt vector number used for syscalls (`int 0x80`).
pub const SYSCALL_INTERRUPT_INDEX: u8 = 0x80;
