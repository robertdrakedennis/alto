//! Checksums, one copy each for the whole client (Phase 2.1).
//!
//! - [`crc32`]: `Packet.getcrc`, the IEEE CRC-32 (JS5 group/index checks,
//!   reflection-check and uid frames, PNG chunks). It was `js5net::getcrc`'s
//!   body; the bitwise `session::crc32` and the per-call-table copies in
//!   `png_out` and `texture` computed the same function
//!   (standard vectors: `core_goldens::checksum` in client910). `native910::repack::crc32`
//!   is a separate crate's copy and is compared in `js5net`'s tests.
//! - [`adler32`]: the zlib trailer (`png_out` writes it, `texture`'s inflate
//!   checks it); the two copies were identical.

use std::sync::OnceLock;

/// The IEEE CRC-32
/// (reflected, polynomial `0xEDB88320`, init and final xor `!0`) through
/// a 256-entry table. `js5net::getcrc` is this value as an `i32`.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0_u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut v = i as u32;
            for _ in 0..8 {
                v = if v & 1 == 1 {
                    v >> 1 ^ 0xEDB8_8320
                } else {
                    v >> 1
                };
            }
            *entry = v;
        }
        table
    });
    let mut crc = u32::MAX;
    for &b in data {
        crc = crc >> 8 ^ table[((crc ^ u32::from(b)) & 0xFF) as usize];
    }
    !crc
}

/// Adler-32 over `data` (RFC 1950 zlib trailer).
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}
