//! The client's Windows-1252 byte <-> UTF-16 unit mapping, one copy for the
//! whole client. Bytes `0x80-0x9F` go through [`EXTENSION`]; the five bytes
//! it leaves unassigned decode to `?` in runtime string readers.

/// The UTF-16 units of bytes `0x80-0x9F`; 0 marks an unassigned byte.
pub const EXTENSION: [u16; 32] = [
    8364, 0, 8218, 402, 8222, 8230, 8224, 8225, 710, 8240, 352, 8249, 338, 0, 381, 0, 0, 8216,
    8217, 8220, 8221, 8226, 8211, 8212, 732, 8482, 353, 8250, 339, 0, 382, 376,
];

/// The first byte [`EXTENSION`] covers.
const EXTENSION_START: u8 = 0x80;

/// One Windows-1252 byte to `char`; `None` for the five unassigned bytes.
pub fn cp1252(byte: u8) -> Option<char> {
    match byte.checked_sub(EXTENSION_START) {
        Some(i) if usize::from(i) < EXTENSION.len() => match EXTENSION[usize::from(i)] {
            0 => None,
            unit => char::from_u32(u32::from(unit)),
        },
        _ => Some(char::from(byte)),
    }
}

/// Decode one byte; the unassigned bytes decode to `?`.
pub fn cp1252_decode_byte(byte: u8) -> char {
    cp1252(byte).unwrap_or('?')
}

/// Encode one UTF-16 unit: `1..128` and `160..=255` map directly, the
/// [`EXTENSION`] units to their byte, everything else to `?`.
pub fn cp1252_encode_unit(unit: u16) -> u8 {
    if (1..128).contains(&unit) || (160..=255).contains(&unit) {
        return unit as u8;
    }
    match EXTENSION.iter().position(|&u| u != 0 && u == unit) {
        Some(i) => EXTENSION_START + i as u8,
        None => b'?',
    }
}
