//! The client's Windows-1252
//! byte <-> UTF-16 unit mapping, one copy for the whole client (Phase 2.1).
//!
//! - [`cp1252_decode_byte`]: decode one byte
//!   (exact: the five bytes the client leaves unassigned decode to `?`).
//!   It replaced `session::cp1252_byte` and the inline table of
//!   `protocol910`'s `Packet::string` (both identical on all 256 bytes).
//! - [`cp1252_encode_unit`]: encode one UTF-16 unit. It
//!   replaced `client_command::byte` and `ui_var_store`'s inline table search
//!   (both identical on all 65536 units).
//! - [`cp1252`]: the same table as an `Option`, `None` for the five
//!   unassigned bytes, for callers that need charset membership. Runtime
//!   string readers use [`cp1252_decode_byte`]'s `?` policy.
//!
//! Loading-screen text uses the same mapping. Native codecs apply this
//! unassigned-byte policy over their Windows-1252 decoder. Their agreement is
//! pinned by client910's `core_goldens::cp1252`.

/// Decode one byte: `0x80-0x9F` via the extension table, unassigned
/// entries map to `?`; all other bytes map directly.
pub fn cp1252_decode_byte(byte: u8) -> char {
    match byte {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        0x81 | 0x8D | 0x8F | 0x90 | 0x9D => '?',
        _ => byte as char,
    }
}

/// Encode one UTF-16 unit: ASCII
/// `1..128` and `160..=255` map directly, the named specials map to
/// `0x80-0x9F`, everything else (including surrogates and unpaired
/// units) maps to `?` (`63`).
pub fn cp1252_encode_unit(unit: u16) -> u8 {
    if (1..128).contains(&unit) || (160..=255).contains(&unit) {
        return unit as u8;
    }
    match unit {
        8364 => 0x80,
        8218 => 0x82,
        402 => 0x83,
        8222 => 0x84,
        8230 => 0x85,
        8224 => 0x86,
        8225 => 0x87,
        710 => 0x88,
        8240 => 0x89,
        352 => 0x8A,
        8249 => 0x8B,
        338 => 0x8C,
        381 => 0x8E,
        8216 => 0x91,
        8217 => 0x92,
        8220 => 0x93,
        8221 => 0x94,
        8226 => 0x95,
        8211 => 0x96,
        8212 => 0x97,
        732 => 0x98,
        8482 => 0x99,
        353 => 0x9A,
        8250 => 0x9B,
        339 => 0x9C,
        382 => 0x9E,
        376 => 0x9F,
        _ => b'?',
    }
}

/// One Windows-1252 byte to `char`. `0x00-0x7F` and `0xA0-0xFF` map directly;
/// `0x80-0x9F` follow the Windows-1252 table; the five unassigned bytes
/// (`0x81 0x8D 0x8F 0x90 0x9D`) are `None`.
pub fn cp1252(byte: u8) -> Option<char> {
    match byte {
        0x80 => Some('€'),
        0x82 => Some('‚'),
        0x83 => Some('ƒ'),
        0x84 => Some('„'),
        0x85 => Some('…'),
        0x86 => Some('†'),
        0x87 => Some('‡'),
        0x88 => Some('ˆ'),
        0x89 => Some('‰'),
        0x8A => Some('Š'),
        0x8B => Some('‹'),
        0x8C => Some('Œ'),
        0x8E => Some('Ž'),
        0x91 => Some('‘'),
        0x92 => Some('’'),
        0x93 => Some('“'),
        0x94 => Some('”'),
        0x95 => Some('•'),
        0x96 => Some('–'),
        0x97 => Some('—'),
        0x98 => Some('˜'),
        0x99 => Some('™'),
        0x9A => Some('š'),
        0x9B => Some('›'),
        0x9C => Some('œ'),
        0x9E => Some('ž'),
        0x9F => Some('Ÿ'),
        0x81 | 0x8D | 0x8F | 0x90 | 0x9D => None,
        _ => Some(byte as char),
    }
}
