//! The IEEE CRC-32 the JS5 group and index checks, the reflection-check
//! replies and the uid frames use.

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
