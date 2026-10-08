//! The 64-bit block cipher (32-round XTEA with the key schedule as the login
//! uses it) that protects the part of a login block after its RSA block, the
//! account-creation requests and the sign-on replies. The key is the four
//! session seeds. Only whole 8-byte blocks of a range are processed; a
//! trailing partial block stays as it is.

const DELTA: u32 = 0x9E37_79B9;
/// `32 * DELTA` modulo 2^32, the sum the decrypting direction starts from.
const FINAL_SUM: u32 = 0xC6EF_3720;
const ROUNDS: usize = 32;

fn word(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn round_value(other: u32, key: &[i32; 4], key_index: u32, sum: u32) -> u32 {
    (((other << 4) ^ (other >> 5)).wrapping_add(other))
        ^ (key[key_index as usize & 3] as u32).wrapping_add(sum)
}

/// Encrypt the whole blocks of `data[start..end]` in place.
pub fn encrypt_range(data: &mut [u8], start: usize, end: usize, key: &[i32; 4]) {
    let blocks = end.saturating_sub(start) / 8;
    for block in 0..blocks {
        let at = start + block * 8;
        let mut v0 = word(&data[at..]);
        let mut v1 = word(&data[at + 4..]);
        let mut sum = 0u32;
        for _ in 0..ROUNDS {
            v0 = v0.wrapping_add(round_value(v1, key, sum, sum));
            sum = sum.wrapping_add(DELTA);
            v1 = v1.wrapping_add(round_value(v0, key, sum >> 11, sum));
        }
        data[at..at + 4].copy_from_slice(&v0.to_be_bytes());
        data[at + 4..at + 8].copy_from_slice(&v1.to_be_bytes());
    }
}

/// Decrypt the whole blocks of `data[start..end]` in place.
pub fn decrypt_range(data: &mut [u8], start: usize, end: usize, key: &[i32; 4]) {
    let blocks = end.saturating_sub(start) / 8;
    for block in 0..blocks {
        let at = start + block * 8;
        let mut v0 = word(&data[at..]);
        let mut v1 = word(&data[at + 4..]);
        let mut sum = FINAL_SUM;
        for _ in 0..ROUNDS {
            v1 = v1.wrapping_sub(round_value(v0, key, sum >> 11, sum));
            sum = sum.wrapping_sub(DELTA);
            v0 = v0.wrapping_sub(round_value(v1, key, sum, sum));
        }
        data[at..at + 4].copy_from_slice(&v0.to_be_bytes());
        data[at + 4..at + 8].copy_from_slice(&v1.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Encryption of a whole buffer and of a range, and decryption of the
    /// range, equal what the original client's own code produced (the
    /// 37-byte buffer ends in a partial block that stays untouched).
    #[test]
    fn matches_the_original_client() {
        let recorded = include_str!("../../../fixtures/recorded/login-crypto/primitives.txt");
        let key = [
            0x0123_4567,
            0x89ab_cdefu32 as i32,
            0xfedc_ba98u32 as i32,
            0x7654_3210,
        ];
        let mut seen = 0;
        for line in recorded.lines() {
            let fields: Vec<&str> = line.split(' ').collect();
            match fields[0] {
                "tiny-whole" => {
                    let mut data = unhex(fields[2]);
                    encrypt_range(&mut data, 0, 32, &key);
                    assert_eq!(data, unhex(fields[3]));
                    seen += 1;
                }
                "tiny-range-5" => {
                    let mut data = unhex(fields[2]);
                    let end = data.len();
                    encrypt_range(&mut data, 5, end, &key);
                    assert_eq!(data, unhex(fields[3]));
                    decrypt_range(&mut data, 5, end, &key);
                    assert_eq!(data, unhex(fields[2]));
                    seen += 1;
                }
                _ => {}
            }
        }
        assert_eq!(seen, 2);
    }
}
