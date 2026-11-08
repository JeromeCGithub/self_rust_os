//! Bounded static PNG decoding into owned RGBA. No URLs, files or presentation.
#![no_std]
extern crate alloc;
use alloc::vec::Vec;
use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
/// Compressed-input and decoded-pixel budgets, checked before decoder allocation.
pub const MAX_ENCODED: usize = 1024 * 1024;
pub const MAX_PIXELS: usize = 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Format,
    Unsupported,
    Limit,
    Memory,
}
#[derive(Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
fn crc(bytes: &[u8]) -> u32 {
    let mut value = u32::MAX;
    for &byte in bytes {
        value ^= u32::from(byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(value & 1));
        }
    }
    !value
}
fn word(bytes: &[u8]) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        bytes.try_into().map_err(|_| Error::Format)?,
    ))
}
/// Keep only pixel-essential chunks. In particular, never inflate untrusted ICC
/// profiles/text metadata in the upstream decoder. Validate every original CRC.
fn pixel_chunks(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), Error> {
    if bytes.len() > MAX_ENCODED {
        return Err(Error::Limit);
    }
    if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return Err(Error::Unsupported);
    }
    let mut clean = Vec::new();
    clean
        .try_reserve_exact(bytes.len())
        .map_err(|_| Error::Memory)?;
    clean.extend_from_slice(&bytes[..8]);
    let mut at = 8usize;
    let mut dimensions = None;
    while at < bytes.len() {
        let header = bytes
            .get(at..at.checked_add(8).ok_or(Error::Limit)?)
            .ok_or(Error::Format)?;
        let length = word(&header[..4])? as usize;
        let end = at
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .filter(|&n| n <= bytes.len())
            .ok_or(Error::Format)?;
        let kind = &header[4..8];
        let payload = &bytes[at + 8..end - 4];
        if crc(&bytes[at + 4..end - 4]) != word(&bytes[end - 4..end])? {
            return Err(Error::Format);
        }
        if at == 8 && kind != b"IHDR" {
            return Err(Error::Format);
        }
        if kind == b"IHDR" {
            if dimensions.is_some() || payload.len() != 13 {
                return Err(Error::Format);
            }
            let width = word(&payload[..4])?;
            let height = word(&payload[4..8])?;
            if width == 0
                || height == 0
                || width > 4096
                || height > 4096
                || (width as usize)
                    .checked_mul(height as usize)
                    .is_none_or(|n| n > MAX_PIXELS)
            {
                return Err(Error::Limit);
            }
            dimensions = Some((width, height));
        }
        if kind == b"acTL" || kind == b"fcTL" || kind == b"fdAT" {
            return Err(Error::Unsupported);
        }
        if kind == b"IHDR"
            || kind == b"PLTE"
            || kind == b"tRNS"
            || kind == b"IDAT"
            || kind == b"IEND"
        {
            clean.extend_from_slice(&bytes[at..end]);
        } else if kind[0] & 32 == 0 {
            return Err(Error::Unsupported);
        }
        if kind == b"IEND" {
            if !payload.is_empty() || end != bytes.len() {
                return Err(Error::Format);
            }
            let (width, height) = dimensions.ok_or(Error::Format)?;
            return Ok((clean, width, height));
        }
        at = end;
    }
    Err(Error::Format)
}
impl Image {
    /// Decode static PNG with strict CRC/dimensions. Metadata/animation, color
    /// management, external resources and other codecs are not silently executed.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (clean, width, height) = pixel_chunks(bytes)?;
        let options = DecoderOptions::default()
            .set_max_width(4096)
            .set_max_height(4096)
            .png_set_confirm_crc(true)
            .png_set_strip_to_8bit(true)
            .png_set_decode_animated(false);
        let mut decoder = zune_png::PngDecoder::new_with_options(clean.as_slice(), options);
        decoder.decode_headers().map_err(|_| Error::Format)?;
        if decoder.get_dimensions() != Some((width as usize, height as usize)) {
            return Err(Error::Format);
        }
        let components = match decoder.get_colorspace() {
            Some(ColorSpace::RGB) => 3,
            Some(ColorSpace::RGBA) => 4,
            Some(ColorSpace::Luma) => 1,
            Some(ColorSpace::LumaA) => 2,
            _ => return Err(Error::Unsupported),
        };
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .ok_or(Error::Limit)?;
        let length = pixels.checked_mul(components).ok_or(Error::Limit)?;
        if decoder.output_buffer_size() != Some(length) {
            return Err(Error::Format);
        }
        let mut raw = Vec::new();
        raw.try_reserve_exact(length).map_err(|_| Error::Memory)?;
        raw.resize(length, 0);
        decoder.decode_into(&mut raw).map_err(|_| Error::Format)?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(pixels * 4)
            .map_err(|_| Error::Memory)?;
        for pixel in raw.chunks_exact(components) {
            match components {
                1 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
                2 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
                3 => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
                4 => rgba.extend_from_slice(pixel),
                _ => return Err(Error::Format),
            }
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}
