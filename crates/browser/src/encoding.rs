use crate::*;
#[derive(Debug, Clone, Copy)]
pub enum Encoding {
    Utf8,
    Windows1252,
}
/// Decode document bytes before tokenization. Explicit UTF-8 remains strict;
/// absent declarations prefer valid UTF-8 and otherwise use the HTML legacy default.
pub fn decode_html(bytes: &[u8], encoding: Option<Encoding>) -> Result<String, Error> {
    if bytes.len() > MAX_HTML {
        return Err(Error::Limit);
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if !matches!(encoding, Some(Encoding::Windows1252)) {
        if let Ok(value) = core::str::from_utf8(bytes) {
            return Ok(value.into());
        }
        if matches!(encoding, Some(Encoding::Utf8)) {
            return Err(Error::Encoding);
        }
    }
    let controls = [
        '€', '\u{fffd}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{fffd}', 'Ž',
        '\u{fffd}', '\u{fffd}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
        '\u{fffd}', 'ž', 'Ÿ',
    ];
    let mut result = String::new();
    for &byte in bytes {
        result.push(if (0x80..0xa0).contains(&byte) {
            controls[(byte - 0x80) as usize]
        } else {
            byte as char
        });
        if result.len() > MAX_HTML {
            return Err(Error::Limit);
        }
    }
    Ok(result)
}
