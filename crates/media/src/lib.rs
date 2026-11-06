//! Bounded userspace media parsing. No device, file, network or syscall access.
#![no_std]
/// Decoder/resource failure, separate from transport failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Unsupported,
    Limit,
}
/// Maximum buffered clip bytes for the initial PCM profile.
pub const MAX_CLIP: usize = 16 * 1024 * 1024;
/// Validated borrowed RIFF/WAVE stereo S16LE48kHz clip. No samples are copied.
#[derive(Clone, Copy)]
pub struct Wav<'a> {
    pcm: &'a [u8],
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(Error::Invalid)?
            .try_into()
            .map_err(|_| Error::Invalid)?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::Invalid)?
            .try_into()
            .map_err(|_| Error::Invalid)?,
    ))
}
impl<'a> Wav<'a> {
    /// Validate the complete container, chunk arithmetic and fixed PCM profile.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_CLIP {
            return Err(Error::Limit);
        }
        if bytes.len() < 12
            || &bytes[..4] != b"RIFF"
            || &bytes[8..12] != b"WAVE"
            || u64::from(u32_at(bytes, 4)?) + 8 != bytes.len() as u64
        {
            return Err(Error::Invalid);
        }
        let mut at = 12usize;
        let mut format = false;
        let mut pcm = None;
        while at < bytes.len() {
            let header = bytes.get(at..at + 8).ok_or(Error::Invalid)?;
            let length = u32_at(header, 4)? as usize;
            let start = at.checked_add(8).ok_or(Error::Invalid)?;
            let end = start.checked_add(length).ok_or(Error::Invalid)?;
            let chunk = bytes.get(start..end).ok_or(Error::Invalid)?;
            match &header[..4] {
                b"fmt " => {
                    if format {
                        return Err(Error::Invalid);
                    }
                    if length < 16
                        || u16_at(chunk, 0)? != 1
                        || u16_at(chunk, 2)? != 2
                        || u32_at(chunk, 4)? != 48000
                        || u32_at(chunk, 8)? != 192000
                        || u16_at(chunk, 12)? != 4
                        || u16_at(chunk, 14)? != 16
                    {
                        return Err(Error::Unsupported);
                    }
                    format = true;
                }
                b"data" => {
                    if pcm.is_some() || length % 4 != 0 {
                        return Err(Error::Invalid);
                    }
                    pcm = Some(chunk);
                }
                _ => {}
            }
            at = end
                .checked_add(length & 1)
                .filter(|end| *end <= bytes.len())
                .ok_or(Error::Invalid)?;
        }
        if !format {
            return Err(Error::Invalid);
        }
        Ok(Self {
            pcm: pcm.ok_or(Error::Invalid)?,
        })
    }
    /// Duration in complete stereo PCM frames.
    pub fn frames(&self) -> u64 {
        (self.pcm.len() / 4) as u64
    }
    /// Borrow at most one hardware-sized chunk beginning at a validated frame.
    pub fn chunk(&self, frame: u64, max_bytes: usize) -> Result<&'a [u8], Error> {
        if frame > self.frames() || max_bytes == 0 || max_bytes > 4096 || max_bytes % 4 != 0 {
            return Err(Error::Invalid);
        }
        let start = usize::try_from(frame)
            .ok()
            .and_then(|n| n.checked_mul(4))
            .ok_or(Error::Invalid)?;
        Ok(&self.pcm[start..start.saturating_add(max_bytes).min(self.pcm.len())])
    }
}
