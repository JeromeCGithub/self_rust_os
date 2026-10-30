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
pub const USER_STACK_TOP: u64 = 0x200_0000;

/// Size of the user-mode stack in bytes (4 MiB, mapped privately at process creation).
pub const USER_STACK_SIZE: u64 = 4 * 1024 * 1024;

/// Bottom of the user-mode stack.
pub const USER_STACK_BOTTOM: u64 = USER_STACK_TOP - USER_STACK_SIZE;

/// Interrupt vector number used for syscalls (`int 0x80`).
pub const SYSCALL_INTERRUPT_INDEX: u8 = 0x80;

/// Virtual address of the display owner's MMIO or a client's private surface.
pub const USER_FRAMEBUFFER: u64 = 0x210_0000;

/// Exclusive end of the private user page-table region (64 MiB).
/// Unmapped gaps between code, stack and display remain guard regions.
pub const USER_REGION_END: u64 = 0x400_0000;

/// Number of pages covering the complete boot-provided display, including pitch
/// padding and the final partial page. Reject layouts outside the user region.
pub fn framebuffer_pages(
    info: &crate::drivers::video::framebuffer::FbInfo,
) -> Result<u64, &'static str> {
    let row_bytes = info
        .width
        .checked_mul(4)
        .ok_or("framebuffer width overflow")?;
    if info.width == 0
        || info.height == 0
        || info.bpp != 32
        || info.pitch < row_bytes
        || info.pitch % 4 != 0
        || info.addr % 4096 != 0
    {
        return Err("unsupported framebuffer layout");
    }
    let bytes = info
        .pitch
        .checked_mul(info.height)
        .ok_or("framebuffer size overflow")?;
    let pages = (bytes as u64).div_ceil(4096);
    if pages > (USER_REGION_END - USER_FRAMEBUFFER) / 4096 {
        return Err("framebuffer exceeds user region");
    }
    Ok(pages)
}

pub mod usercopy;
