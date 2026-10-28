//! Multiboot2 information structure parsing.
//!
//! GRUB2 enters the kernel in 32-bit protected mode with `eax =
//! 0x36D76289` and `ebx` pointing at the Multiboot2 information structure.
//! This module parses the tags the kernel needs:
//!
//! - tag 6: the firmware memory map (available RAM), and
//! - tag 8: the framebuffer GRUB configured for us.
//!
//! The parser is allocation-free (fixed-size arrays) because it runs before
//! the heap exists. It is pure enough to be unit-tested without GRUB: tests
//! build a fake information structure in memory and parse it.

use crate::memory::MemRegion;

/// Magic value GRUB places in `eax` for Multiboot2.
pub const MULTIBOOT2_MAGIC: u32 = 0x36D7_6289;

/// Maximum number of memory regions recorded from the memory map tag.
pub const MAX_REGIONS: usize = 64;

/// Memory map entry type: available RAM.
const MEM_AVAILABLE: u32 = 1;

/// Information extracted from the Multiboot2 structure.
#[derive(Debug)]
pub struct BootInfo {
    /// Available-RAM regions (`[start, end)`), sorted by address.
    pub regions: [MemRegion; MAX_REGIONS],
    /// Number of valid entries in `regions`.
    pub region_count: usize,
    /// Framebuffer description from GRUB.
    pub framebuffer: Framebuffer,
}

impl BootInfo {
    /// Total available RAM in bytes.
    #[must_use]
    pub fn total_usable_bytes(&self) -> u64 {
        self.regions[..self.region_count]
            .iter()
            .map(|r| r.len())
            .sum()
    }

    /// Usable regions as a slice.
    #[must_use]
    pub fn regions(&self) -> &[MemRegion] {
        &self.regions[..self.region_count]
    }
}

/// Framebuffer information (Multiboot2 tag type 8).
#[derive(Debug, Clone, Copy)]
pub struct Framebuffer {
    /// Physical address of the framebuffer.
    pub addr: u64,
    /// Bytes per scanline.
    pub pitch: u32,
    /// Visible width in pixels.
    pub width: u32,
    /// Visible height in pixels.
    pub height: u32,
    /// Bits per pixel.
    pub bpp: u8,
    /// Framebuffer type: 0 = indexed, 1 = RGB, 2 = EGA text.
    pub fb_type: u8,
}

/// Errors returned by [`parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// `eax` did not contain the Multiboot2 magic.
    BadMagic(u32),
    /// Null info pointer or structure smaller than its own header.
    Truncated,
    /// No memory map tag (type 6) was present.
    NoMemoryMap,
    /// No framebuffer tag (type 8) was present.
    NoFramebuffer,
    /// The memory map holds more than [`MAX_REGIONS`] available regions.
    TooManyRegions,
}

// Multiboot2 tag types we interpret.
const TAG_END: u32 = 0;
const TAG_MEMORY_MAP: u32 = 6;
const TAG_FRAMEBUFFER: u32 = 8;

/// Reads a little-endian `u32` from an arbitrary (possibly unaligned) address.
///
/// # Safety
///
/// `addr` must be readable for 4 bytes.
unsafe fn read_u32(addr: u64) -> u32 {
    // SAFETY: The caller guarantees readability; `read_unaligned` tolerates
    // any alignment.
    unsafe { (addr as *const u32).read_unaligned() }
}

/// Reads a little-endian `u64` from an arbitrary (possibly unaligned) address.
///
/// # Safety
///
/// `addr` must be readable for 8 bytes.
unsafe fn read_u64(addr: u64) -> u64 {
    // SAFETY: The caller guarantees readability; `read_unaligned` tolerates
    // any alignment.
    unsafe { (addr as *const u64).read_unaligned() }
}

/// Parses the Multiboot2 information structure.
///
/// # Safety
///
/// `info_ptr` must point to a valid Multiboot2 information structure (as
/// passed by GRUB in `ebx`). At boot time it is identity-mapped, so raw
/// reads are valid.
///
/// # Errors
///
/// Returns [`ParseError`] when the magic is wrong, the structure is
/// truncated, or a required tag is missing.
pub unsafe fn parse(magic: u64, info_ptr: u64) -> Result<BootInfo, ParseError> {
    if magic as u32 != MULTIBOOT2_MAGIC {
        return Err(ParseError::BadMagic(magic as u32));
    }
    if info_ptr == 0 {
        return Err(ParseError::Truncated);
    }

    // SAFETY: Guaranteed readable by the caller.
    let total_size = unsafe { read_u32(info_ptr) };
    if total_size < 16 {
        return Err(ParseError::Truncated);
    }

    let mut info = BootInfo {
        regions: [MemRegion { start: 0, end: 0 }; MAX_REGIONS],
        region_count: 0,
        framebuffer: Framebuffer {
            addr: 0,
            pitch: 0,
            width: 0,
            height: 0,
            bpp: 0,
            fb_type: 0,
        },
    };
    let mut have_mmap = false;
    let mut have_fb = false;

    // Tags start after the 8-byte header; each tag is 8-byte aligned.
    let mut offset = 8_u64;
    while offset + 8 <= u64::from(total_size) {
        // SAFETY: Bounds-checked against `total_size` above.
        let tag_type = unsafe { read_u32(info_ptr + offset) };
        let tag_size = unsafe { read_u32(info_ptr + offset + 4) };
        if tag_size < 8 || offset + u64::from(tag_size) > u64::from(total_size) {
            return Err(ParseError::Truncated);
        }
        let body = info_ptr + offset + 8;

        match tag_type {
            TAG_END => break,
            TAG_MEMORY_MAP => {
                parse_memory_map(body, tag_size, &mut info)?;
                have_mmap = true;
            }
            TAG_FRAMEBUFFER => {
                info.framebuffer = parse_framebuffer(body);
                have_fb = true;
            }
            _ => {}
        }

        offset += (u64::from(tag_size) + 7) & !7;
    }

    if !have_mmap {
        return Err(ParseError::NoMemoryMap);
    }
    if !have_fb {
        return Err(ParseError::NoFramebuffer);
    }
    Ok(info)
}

/// Parses a memory map tag body into `info.regions`.
///
/// Only entries of type "available RAM" are recorded.
fn parse_memory_map(
    body: u64,
    tag_size: u32,
    info: &mut BootInfo,
) -> Result<(), ParseError> {
    // SAFETY: `body` points inside the validated information structure.
    let entry_size = unsafe { read_u32(body) } as u64;
    let entries_start = body + 8;
    let entries_end = body + u64::from(tag_size);

    let mut addr = entries_start;
    while addr + entry_size <= entries_end && entry_size >= 24 {
        // SAFETY: Bounds-checked against the tag size.
        let base = unsafe { read_u64(addr) };
        let length = unsafe { read_u64(addr + 8) };
        let entry_type = unsafe { read_u32(addr + 16) };

        if entry_type == MEM_AVAILABLE && length > 0 {
            if info.region_count >= MAX_REGIONS {
                return Err(ParseError::TooManyRegions);
            }
            // `end` is exclusive; guard against overflow.
            let end = base.saturating_add(length);
            if let Some(region) = MemRegion::new(base, end) {
                info.regions[info.region_count] = region;
                info.region_count += 1;
            }
        }

        addr += entry_size;
    }
    Ok(())
}

/// Parses a framebuffer tag body.
fn parse_framebuffer(body: u64) -> Framebuffer {
    // SAFETY: `body` points inside the validated information structure.
    unsafe {
        Framebuffer {
            addr: read_u64(body),
            pitch: read_u32(body + 8),
            width: read_u32(body + 12),
            height: read_u32(body + 16),
            bpp: ((read_u32(body + 20)) & 0xFF) as u8,
            fb_type: ((read_u32(body + 20) >> 8) & 0xFF) as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, ParseError, MULTIBOOT2_MAGIC};

    /// Builds a minimal fake Multiboot2 structure in the given buffer and
    /// returns its total size.
    fn build_fake_info(buf: &mut [u8]) -> usize {
        let mut off = 0_usize;

        let write_u32 = |buf: &mut [u8], off: &mut usize, v: u32| {
            buf[*off..*off + 4].copy_from_slice(&v.to_le_bytes());
            *off += 4;
        };
        let write_u64 = |buf: &mut [u8], off: &mut usize, v: u64| {
            buf[*off..*off + 8].copy_from_slice(&v.to_le_bytes());
            *off += 8;
        };

        // Header: total_size (patched later), reserved.
        write_u32(buf, &mut off, 0);
        write_u32(buf, &mut off, 0);

        // Memory map tag (type 6): two available regions, one reserved.
        write_u32(buf, &mut off, 6);
        let mmap_size_off = off;
        write_u32(buf, &mut off, 0); // size patched later
        write_u32(buf, &mut off, 24); // entry_size
        write_u32(buf, &mut off, 0); // entry_version
        // Region 1: 1 MiB .. 128 MiB, available.
        write_u64(buf, &mut off, 0x10_0000);
        write_u64(buf, &mut off, 0x7F0_0000);
        write_u32(buf, &mut off, 1);
        write_u32(buf, &mut off, 0);
        // Region 2: 0x1000_0000 .. 0x1400_0000, available.
        write_u64(buf, &mut off, 0x1000_0000);
        write_u64(buf, &mut off, 0x0400_0000);
        write_u32(buf, &mut off, 1);
        write_u32(buf, &mut off, 0);
        // Region 3: reserved, must be skipped.
        write_u64(buf, &mut off, 0x2000_0000);
        write_u64(buf, &mut off, 0x0100_0000);
        write_u32(buf, &mut off, 2);
        write_u32(buf, &mut off, 0);
        let mmap_end = off;
        let mmap_size = (mmap_end - (mmap_size_off - 4)) as u32;
        buf[mmap_size_off..mmap_size_off + 4].copy_from_slice(&mmap_size.to_le_bytes());

        // Framebuffer tag (type 8): 320x200x8 indexed at 0xA0000.
        // Body: addr(8) + pitch(4) + width(4) + height(4) + bpp/type(4) = 24.
        write_u32(buf, &mut off, 8);
        write_u32(buf, &mut off, 32);
        write_u64(buf, &mut off, 0xA0000);
        write_u32(buf, &mut off, 320); // pitch
        write_u32(buf, &mut off, 320); // width
        write_u32(buf, &mut off, 200); // height
        write_u32(buf, &mut off, 8); // bpp=8 (low byte), type=0 (indexed)

        // End tag.
        write_u32(buf, &mut off, 0);
        write_u32(buf, &mut off, 8);

        let total = off;
        buf[0..4].copy_from_slice(&(total as u32).to_le_bytes());
        total
    }

    #[test_case]
    fn test_parse_valid_info() {
        let mut buf = [0_u8; 256];
        build_fake_info(&mut buf);

        // SAFETY: `buf` is a valid fake structure for the parser to read.
        let info = unsafe { parse(u64::from(MULTIBOOT2_MAGIC), buf.as_ptr() as u64) }
            .expect("parse should succeed");

        assert_eq!(info.region_count, 2, "reserved region must be skipped");
        assert_eq!(info.regions[0].start, 0x10_0000);
        assert_eq!(info.regions[0].end, 0x10_0000 + 0x7F0_0000);
        assert_eq!(info.regions[1].start, 0x1000_0000);
        assert_eq!(
            info.total_usable_bytes(),
            0x7F0_0000 + 0x0400_0000,
            "usable bytes must sum both regions"
        );

        assert_eq!(info.framebuffer.addr, 0xA0000);
        assert_eq!(info.framebuffer.width, 320);
        assert_eq!(info.framebuffer.height, 200);
        assert_eq!(info.framebuffer.bpp, 8);
        assert_eq!(info.framebuffer.fb_type, 0);
    }

    #[test_case]
    fn test_parse_rejects_bad_magic() {
        let mut buf = [0_u8; 256];
        build_fake_info(&mut buf);

        // SAFETY: `buf` holds a fake structure; only the magic is wrong.
        let err =
            unsafe { parse(0xDEAD_BEEF, buf.as_ptr() as u64) }.expect_err("bad magic must fail");
        assert_eq!(err, ParseError::BadMagic(0xDEAD_BEEF));
    }

    #[test_case]
    fn test_parse_rejects_null_pointer() {
        // SAFETY: Null pointer must be rejected before any read.
        let err =
            unsafe { parse(u64::from(MULTIBOOT2_MAGIC), 0) }.expect_err("null ptr must fail");
        assert_eq!(err, ParseError::Truncated);
    }
}
