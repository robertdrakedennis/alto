//! The ISO/NESSIE Whirlpool digest the JS5 layer checks master indexes,
//! archive indexes and groups with.

/// The Whirlpool S-box.
pub(crate) const SBOX: [u8; 256] = [
    0x18, 0x23, 0xc6, 0xe8, 0x87, 0xb8, 0x01, 0x4f, 0x36, 0xa6, 0xd2, 0xf5, 0x79, 0x6f, 0x91, 0x52,
    0x60, 0xbc, 0x9b, 0x8e, 0xa3, 0x0c, 0x7b, 0x35, 0x1d, 0xe0, 0xd7, 0xc2, 0x2e, 0x4b, 0xfe, 0x57,
    0x15, 0x77, 0x37, 0xe5, 0x9f, 0xf0, 0x4a, 0xda, 0x58, 0xc9, 0x29, 0x0a, 0xb1, 0xa0, 0x6b, 0x85,
    0xbd, 0x5d, 0x10, 0xf4, 0xcb, 0x3e, 0x05, 0x67, 0xe4, 0x27, 0x41, 0x8b, 0xa7, 0x7d, 0x95, 0xd8,
    0xfb, 0xee, 0x7c, 0x66, 0xdd, 0x17, 0x47, 0x9e, 0xca, 0x2d, 0xbf, 0x07, 0xad, 0x5a, 0x83, 0x33,
    0x63, 0x02, 0xaa, 0x71, 0xc8, 0x19, 0x49, 0xd9, 0xf2, 0xe3, 0x5b, 0x88, 0x9a, 0x26, 0x32, 0xb0,
    0xe9, 0x0f, 0xd5, 0x80, 0xbe, 0xcd, 0x34, 0x48, 0xff, 0x7a, 0x90, 0x5f, 0x20, 0x68, 0x1a, 0xae,
    0xb4, 0x54, 0x93, 0x22, 0x64, 0xf1, 0x73, 0x12, 0x40, 0x08, 0xc3, 0xec, 0xdb, 0xa1, 0x8d, 0x3d,
    0x97, 0x00, 0xcf, 0x2b, 0x76, 0x82, 0xd6, 0x1b, 0xb5, 0xaf, 0x6a, 0x50, 0x45, 0xf3, 0x30, 0xef,
    0x3f, 0x55, 0xa2, 0xea, 0x65, 0xba, 0x2f, 0xc0, 0xde, 0x1c, 0xfd, 0x4d, 0x92, 0x75, 0x06, 0x8a,
    0xb2, 0xe6, 0x0e, 0x1f, 0x62, 0xd4, 0xa8, 0x96, 0xf9, 0xc5, 0x25, 0x59, 0x84, 0x72, 0x39, 0x4c,
    0x5e, 0x78, 0x38, 0x8c, 0xd1, 0xa5, 0xe2, 0x61, 0xb3, 0x21, 0x9c, 0x1e, 0x43, 0xc7, 0xfc, 0x04,
    0x51, 0x99, 0x6d, 0x0d, 0xfa, 0xdf, 0x7e, 0x24, 0x3b, 0xab, 0xce, 0x11, 0x8f, 0x4e, 0xb7, 0xeb,
    0x3c, 0x81, 0x94, 0xf7, 0xb9, 0x13, 0x2c, 0xd3, 0xe7, 0x6e, 0xc4, 0x03, 0x56, 0x44, 0x7f, 0xa9,
    0x2a, 0xbb, 0xc1, 0x53, 0xdc, 0x0b, 0x9d, 0x6c, 0x31, 0x74, 0xf6, 0x46, 0xac, 0x89, 0x14, 0xe1,
    0x16, 0x3a, 0x69, 0x09, 0x70, 0xb6, 0xd0, 0xed, 0xcc, 0x42, 0x98, 0xa4, 0x28, 0x5c, 0xf8, 0x86,
];

/// The eight circulant tables and the round constants.
fn tables() -> &'static ([[u64; 256]; 8], [u64; 11]) {
    static TABLES: std::sync::OnceLock<([[u64; 256]; 8], [u64; 11])> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let mut c = [[0_u64; 256]; 8];
        for x in 0..256 {
            let v1 = u64::from(SBOX[x]);
            let mut v2 = v1 << 1;
            if v2 >= 256 {
                v2 ^= 0x11D;
            }
            let mut v4 = v2 << 1;
            if v4 >= 256 {
                v4 ^= 0x11D;
            }
            let v5 = v4 ^ v1;
            let mut v8 = v4 << 1;
            if v8 >= 256 {
                v8 ^= 0x11D;
            }
            let v9 = v8 ^ v1;
            c[0][x] =
                v1 << 56 | v1 << 48 | v4 << 40 | v1 << 32 | v8 << 24 | v5 << 16 | v2 << 8 | v9;
            for t in 1..8 {
                c[t][x] = c[t - 1][x].rotate_right(8);
            }
        }
        let mut rc = [0_u64; 11];
        for (r, slot) in rc.iter_mut().enumerate().skip(1) {
            let i = (r - 1) * 8;
            *slot = c[0][i] & 0xFF00_0000_0000_0000
                ^ c[1][i + 1] & 0x00FF_0000_0000_0000
                ^ c[2][i + 2] & 0x0000_FF00_0000_0000
                ^ c[3][i + 3] & 0x0000_00FF_0000_0000
                ^ c[4][i + 4] & 0x0000_0000_FF00_0000
                ^ c[5][i + 5] & 0x0000_0000_00FF_0000
                ^ c[6][i + 6] & 0x0000_0000_0000_FF00
                ^ c[7][i + 7] & 0x0000_0000_0000_00FF;
        }
        (c, rc)
    })
}

/// One hashing state: the message bit length, the block buffer, its bit
/// count and position, and the running hash.
struct Whirlpool {
    bit_length: [u8; 32],
    buffer: [u8; 64],
    buffer_bits: i32,
    buffer_pos: usize,
    hash: [u64; 8],
}

impl Whirlpool {
    /// A fresh state.
    fn new() -> Self {
        Self {
            bit_length: [0; 32],
            buffer: [0; 64],
            buffer_bits: 0,
            buffer_pos: 0,
            hash: [0; 8],
        }
    }

    /// The compression function over the buffered block.
    fn process_buffer(&mut self) {
        let (c, rc) = tables();
        let mut block = [0_u64; 8];
        for (i, word) in block.iter_mut().enumerate() {
            let b = &self.buffer[i * 8..i * 8 + 8];
            *word = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        }
        let mut k = self.hash;
        let mut state = [0_u64; 8];
        for i in 0..8 {
            state[i] = block[i] ^ k[i];
        }
        let mut l = [0_u64; 8];
        for &round in &rc[1..=10] {
            for i in 0..8 {
                l[i] = 0;
                let mut shift = 56;
                for t in 0..8 {
                    l[i] ^= c[t][((k[(i.wrapping_sub(t)) & 7] >> shift) & 0xFF) as usize];
                    shift -= 8;
                }
            }
            k = l;
            k[0] ^= round;
            for i in 0..8 {
                l[i] = k[i];
                let mut shift = 56;
                for t in 0..8 {
                    l[i] ^= c[t][((state[(i.wrapping_sub(t)) & 7] >> shift) & 0xFF) as usize];
                    shift -= 8;
                }
            }
            state = l;
        }
        for i in 0..8 {
            self.hash[i] ^= state[i] ^ block[i];
        }
    }

    /// Absorb `bits` bits of `data`.
    fn add(&mut self, data: &[u8], mut bits: i64) {
        let mut pos = 0_usize;
        let gap = (8 - (bits as i32 & 7)) & 7;
        let rem = self.buffer_bits & 7;
        let mut value = bits;
        let mut carry = 0_i32;
        for i in (0..32).rev() {
            let sum = i32::from(self.bit_length[i]) + (value as i32 & 0xFF) + carry;
            self.bit_length[i] = sum as u8;
            carry = ((sum as u32) >> 8) as i32;
            value = ((value as u64) >> 8) as i64;
        }
        while bits > 8 {
            let b = (i32::from(data[pos] as i8) << gap) & 0xFF
                | (i32::from(data[pos + 1]) & 0xFF) >> (8 - gap);
            self.buffer[self.buffer_pos] |= (b >> rem) as u8;
            self.buffer_pos += 1;
            self.buffer_bits += 8 - rem;
            if self.buffer_bits == 512 {
                self.process_buffer();
                self.buffer_pos = 0;
                self.buffer_bits = 0;
            }
            self.buffer[self.buffer_pos] = ((b << (8 - rem)) & 0xFF) as u8;
            self.buffer_bits += rem;
            bits -= 8;
            pos += 1;
        }
        let b = if bits > 0 {
            let b = (i32::from(data[pos] as i8) << gap) & 0xFF;
            self.buffer[self.buffer_pos] |= (b >> rem) as u8;
            b
        } else {
            0
        };
        if i64::from(rem) + bits < 8 {
            self.buffer_bits += bits as i32;
            return;
        }
        self.buffer_pos += 1;
        self.buffer_bits += 8 - rem;
        let left = bits - i64::from(8 - rem);
        if self.buffer_bits == 512 {
            self.process_buffer();
            self.buffer_pos = 0;
            self.buffer_bits = 0;
        }
        self.buffer[self.buffer_pos] = ((b << (8 - rem)) & 0xFF) as u8;
        self.buffer_bits += left as i32;
    }

    /// Pad, append the length and
    /// emit the 64-byte digest.
    fn finish(mut self) -> [u8; 64] {
        self.buffer[self.buffer_pos] |= (0x80_u32 >> (self.buffer_bits & 7)) as u8;
        self.buffer_pos += 1;
        if self.buffer_pos > 32 {
            while self.buffer_pos < 64 {
                self.buffer[self.buffer_pos] = 0;
                self.buffer_pos += 1;
            }
            self.process_buffer();
            self.buffer_pos = 0;
        }
        while self.buffer_pos < 32 {
            self.buffer[self.buffer_pos] = 0;
            self.buffer_pos += 1;
        }
        self.buffer[32..64].copy_from_slice(&self.bit_length);
        self.process_buffer();
        let mut out = [0_u8; 64];
        for (i, word) in self.hash.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

/// The Whirlpool digest of `data`.
#[must_use]
pub fn compute(data: &[u8]) -> [u8; 64] {
    let mut w = Whirlpool::new();
    w.add(data, data.len() as i64 * 8);
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// ISO/IEC 10118-3 test vectors.
    #[test]
    fn whirlpool_matches_reference_vectors() {
        assert_eq!(
            hex(&compute(b"")),
            "19FA61D75522A4669B44E39C1D2E1726C530232130D407F89AFEE0964997F7A73E83BE698B288FEBCF88E3E03C4F0757EA8964E59B63D93708B138CC42A66EB3"
        );
        assert_eq!(
            hex(&compute(b"abc")),
            "4E2448A4C6F486BB16B6562C73B4020BF3043E3A731BCE721AE1B303D97E6D4C7181EEBDB6C57E277D0E34957114CBD6C797FC9D95D8B582D225292076D4EEF5"
        );
        // 80 digits: the padding spills into a second 512-bit block.
        assert_eq!(
            hex(&compute("1234567890".repeat(8).as_bytes())),
            "466EF18BABB0154D25B9D38A6414F5C08784372BCCB204D6549C4AFADB6014294D5BD8DF2A6C44E538CD047B2681A51A2C60481E88C5A20B2C2A80CF3A9A083B"
        );
    }
}
