//! Bounded, allocation-free validation of static x86-64 ELF executable segments.
#![no_std]

/// Invalid or unsupported executable image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidImage;

/// One private load mapping, with a file prefix followed by zero-filled RAM.
#[derive(Clone, Copy, Debug)]
pub struct Segment<'a> {
    pub address: u64,
    pub memory_size: u64,
    pub bytes: &'a [u8],
    pub writable: bool,
    pub executable: bool,
}

/// Validated executable layout; segment bytes borrow immutable kernel storage.
#[derive(Clone, Copy, Debug)]
pub struct Image<'a> {
    bytes: &'a [u8],
    entry: u64,
    base: u64,
    headers: Option<(usize, usize)>,
}

impl<'a> Image<'a> {
    /// Validate every segment before any frame or PID is allocated. Legacy raw
    /// flat images remain supported at `base`; ELF never falls back after error.
    pub fn parse(bytes: &'a [u8], base: u64, limit: u64) -> Result<Self, InvalidImage> {
        if base < 4096 || base % 4096 != 0 || base >= limit || limit > 1 << 47 {
            return Err(InvalidImage);
        }
        if !bytes.starts_with(b"\x7fELF") {
            if bytes.is_empty() || bytes.len() as u64 > limit - base {
                return Err(InvalidImage);
            }
            return Ok(Self {
                bytes,
                entry: base,
                base,
                headers: None,
            });
        }
        if bytes.len() < 64
            || bytes[4..9] != [2, 1, 1, 0, 0]
            || u16_at(bytes, 16)? != 2
            || u16_at(bytes, 18)? != 62
            || u32_at(bytes, 20)? != 1
            || u32_at(bytes, 48)? != 0
            || u16_at(bytes, 52)? != 64
            || u16_at(bytes, 54)? != 56
        {
            return Err(InvalidImage);
        }
        let count = usize::from(u16_at(bytes, 56)?);
        let offset = usize::try_from(u64_at(bytes, 32)?).map_err(|_| InvalidImage)?;
        if !(1..=128).contains(&count)
            || offset < 64
            || offset
                .checked_add(count * 56)
                .filter(|end| *end <= bytes.len())
                .is_none()
        {
            return Err(InvalidImage);
        }
        let image = Self {
            bytes,
            entry: u64_at(bytes, 24)?,
            base,
            headers: Some((offset, count)),
        };
        let mut previous_end = base;
        let mut entry_found = false;
        for index in 0..count {
            let p = offset + index * 56;
            match u32_at(bytes, p)? {
                // Runtime relocation, interpreter and compiler TLS are not
                // implemented. Reject rather than partially load such a file.
                2 | 3 | 7 => return Err(InvalidImage),
                1 => {}
                _ => continue,
            }
            let segment = image.elf_segment(p)?;
            let flags = u32_at(bytes, p + 4)?;
            let file_offset = u64_at(bytes, p + 8)?;
            let alignment = u64_at(bytes, p + 48)?;
            if segment.bytes.len() as u64 > segment.memory_size
                || flags & !7 != 0
                || flags & 4 == 0
                || segment.address % 4096 != file_offset % 4096
                || (alignment > 1
                    && (!alignment.is_power_of_two()
                        || segment.address % alignment != file_offset % alignment))
            {
                return Err(InvalidImage);
            }
            if segment.memory_size == 0 {
                continue;
            }
            let end = segment
                .address
                .checked_add(segment.memory_size)
                .ok_or(InvalidImage)?;
            let page_end = end.checked_add(4095).ok_or(InvalidImage)? & !4095;
            if segment.address < previous_end || page_end > limit {
                return Err(InvalidImage);
            }
            previous_end = end;
            entry_found |= segment.executable
                && image.entry >= segment.address
                && image.entry < segment.address + segment.bytes.len() as u64;
        }
        if !entry_found {
            return Err(InvalidImage);
        }
        Ok(image)
    }
    pub fn entry(&self) -> u64 {
        self.entry
    }
    pub fn is_elf(&self) -> bool {
        self.headers.is_some()
    }
    /// Iterate only validated PT_LOAD mappings, excluding empty segments.
    pub fn segments(&self) -> Segments<'a> {
        Segments {
            image: *self,
            index: 0,
        }
    }

    fn elf_segment(&self, p: usize) -> Result<Segment<'a>, InvalidImage> {
        let offset = usize::try_from(u64_at(self.bytes, p + 8)?).map_err(|_| InvalidImage)?;
        let size = usize::try_from(u64_at(self.bytes, p + 32)?).map_err(|_| InvalidImage)?;
        let end = offset.checked_add(size).ok_or(InvalidImage)?;
        let flags = u32_at(self.bytes, p + 4)?;
        Ok(Segment {
            address: u64_at(self.bytes, p + 16)?,
            memory_size: u64_at(self.bytes, p + 40)?,
            bytes: self.bytes.get(offset..end).ok_or(InvalidImage)?,
            writable: flags & 2 != 0,
            executable: flags & 1 != 0,
        })
    }
}

/// Iterator over an immutable, fully validated image.
pub struct Segments<'a> {
    image: Image<'a>,
    index: usize,
}
impl<'a> Iterator for Segments<'a> {
    type Item = Segment<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let Some((offset, count)) = self.image.headers else {
            if self.index != 0 {
                return None;
            }
            self.index = 1;
            return Some(Segment {
                address: self.image.base,
                memory_size: self.image.bytes.len() as u64,
                bytes: self.image.bytes,
                writable: true,
                executable: true,
            });
        };
        while self.index < count {
            let p = offset + self.index * 56;
            self.index += 1;
            if u32_at(self.image.bytes, p).expect("validated program header") == 1 {
                let segment = self
                    .image
                    .elf_segment(p)
                    .expect("validated immutable segment");
                if segment.memory_size != 0 {
                    return Some(segment);
                }
            }
        }
        None
    }
}
fn word<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], InvalidImage> {
    bytes
        .get(at..at.checked_add(N).ok_or(InvalidImage)?)
        .ok_or(InvalidImage)?
        .try_into()
        .map_err(|_| InvalidImage)
}
fn u16_at(bytes: &[u8], at: usize) -> Result<u16, InvalidImage> {
    Ok(u16::from_le_bytes(word(bytes, at)?))
}
fn u32_at(bytes: &[u8], at: usize) -> Result<u32, InvalidImage> {
    Ok(u32::from_le_bytes(word(bytes, at)?))
}
fn u64_at(bytes: &[u8], at: usize) -> Result<u64, InvalidImage> {
    Ok(u64::from_le_bytes(word(bytes, at)?))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;
    const BASE: u64 = 0x400000;
    const LIMIT: u64 = 0x2c00000;
    fn put16(b: &mut [u8], at: usize, n: u16) {
        b[at..at + 2].copy_from_slice(&n.to_le_bytes());
    }
    fn put32(b: &mut [u8], at: usize, n: u32) {
        b[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    fn put64(b: &mut [u8], at: usize, n: u64) {
        b[at..at + 8].copy_from_slice(&n.to_le_bytes());
    }
    fn executable() -> Vec<u8> {
        let mut b = vec![0; 0x2004];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        put16(&mut b, 16, 2);
        put16(&mut b, 18, 62);
        put32(&mut b, 20, 1);
        put64(&mut b, 24, BASE);
        put64(&mut b, 32, 64);
        put16(&mut b, 52, 64);
        put16(&mut b, 54, 56);
        put16(&mut b, 56, 2);
        for (index, offset, address, flags, memory) in [
            (0, 0x1000, BASE, 5, 4),
            (1, 0x2000, BASE + 0x1000, 6, 0x100004),
        ] {
            let p = 64 + index * 56;
            put32(&mut b, p, 1);
            put32(&mut b, p + 4, flags);
            put64(&mut b, p + 8, offset);
            put64(&mut b, p + 16, address);
            put64(&mut b, p + 32, 4);
            put64(&mut b, p + 40, memory);
            put64(&mut b, p + 48, 0x1000);
        }
        b[0x1000..0x1004].copy_from_slice(&[0x90, 0x90, 0xeb, 0xfe]);
        b[0x2000..].copy_from_slice(b"data");
        b
    }
    #[test]
    fn accepts_compact_static_elf_with_large_zero_fill() {
        let bytes = executable();
        let image = Image::parse(&bytes, BASE, LIMIT).unwrap();
        assert_eq!(image.entry(), BASE);
        assert!(image.is_elf());
        let segments: Vec<_> = image.segments().collect();
        assert_eq!(segments.len(), 2);
        assert!(segments[0].executable && !segments[0].writable);
        assert_eq!(segments[1].bytes, b"data");
        assert_eq!(segments[1].memory_size, 0x100004);
        assert!(segments[1].writable && !segments[1].executable);
    }
    #[test]
    fn preserves_bounded_legacy_flat_images() {
        assert_eq!(Image::parse(&[0xc3], BASE, LIMIT).unwrap().entry(), BASE);
        assert!(Image::parse(&[], BASE, LIMIT).is_err());
        assert!(Image::parse(&[0; 8], LIMIT - 4, LIMIT).is_err());
    }
    #[test]
    fn accepts_distinct_byte_ranges_sharing_a_page_like_a_linker_got() {
        let mut bytes = executable();
        bytes.resize(0x200c, 0);
        put16(&mut bytes, 56, 3);
        put64(&mut bytes, 64 + 56 + 40, 4);
        put32(&mut bytes, 64 + 56 + 4, 4);
        let p = 64 + 2 * 56;
        put32(&mut bytes, p, 1);
        put32(&mut bytes, p + 4, 6);
        put64(&mut bytes, p + 8, 0x2004);
        put64(&mut bytes, p + 16, BASE + 0x1004);
        put64(&mut bytes, p + 32, 8);
        put64(&mut bytes, p + 40, 8);
        put64(&mut bytes, p + 48, 4096);
        let image = Image::parse(&bytes, BASE, LIMIT).unwrap();
        let segments: Vec<_> = image.segments().collect();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[2].address, BASE + 0x1004);
        assert!(segments[2].writable);
    }
    #[test]
    fn rejects_malformed_headers_without_panics() {
        let original = executable();
        for size in 4..176 {
            assert!(Image::parse(&original[..size], BASE, LIMIT).is_err());
        }
        for (at, value) in [
            (4, 1),
            (5, 2),
            (6, 2),
            (7, 3),
            (16, 3),
            (18, 3),
            (20, 2),
            (52, 63),
            (54, 55),
            (56, 0),
        ] {
            let mut b = original.clone();
            b[at] = value;
            assert!(Image::parse(&b, BASE, LIMIT).is_err(), "header byte {}", at);
        }
        for offset in [u64::MAX, original.len() as u64 - 1] {
            let mut b = original.clone();
            put64(&mut b, 32, offset);
            assert!(Image::parse(&b, BASE, LIMIT).is_err());
        }
        let mut b = original.clone();
        put16(&mut b, 56, 129);
        assert!(Image::parse(&b, BASE, LIMIT).is_err());
    }
    #[test]
    fn rejects_overflow_overlap_foreign_pages_and_unsupported_runtime() {
        let original = executable();
        for (field, value) in [
            (8, u64::MAX),
            (16, BASE - 4096),
            (16, LIMIT),
            (16, BASE + 1),
            (32, 5),
            (40, 3),
            (40, u64::MAX),
            (48, 3),
        ] {
            let mut b = original.clone();
            put64(&mut b, 64 + field, value);
            assert!(
                Image::parse(&b, BASE, LIMIT).is_err(),
                "segment field {}",
                field
            );
        }
        let mut b = original.clone();
        put64(&mut b, 64 + 56 + 16, BASE);
        assert!(Image::parse(&b, BASE, LIMIT).is_err());
        for kind in [2, 3, 7] {
            let mut b = original.clone();
            put32(&mut b, 64 + 56, kind);
            assert!(Image::parse(&b, BASE, LIMIT).is_err());
        }
        for entry in [BASE - 1, BASE + 4, BASE + 4096, LIMIT, u64::MAX] {
            let mut b = original.clone();
            put64(&mut b, 24, entry);
            assert!(Image::parse(&b, BASE, LIMIT).is_err());
        }
    }
}
