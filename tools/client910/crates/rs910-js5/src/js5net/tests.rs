use super::*;

#[test]
fn handshake_encode_vectors_are_byte_exact() {
    // Empty token: p1(15) p1(10) p4(910) p4(1) pjstr("") p1(0).
    assert_eq!(
        encode_js5_handshake(910, 1, "", 0).unwrap(),
        vec![15, 10, 0, 0, 3, 142, 0, 0, 0, 1, 0, 0]
    );
    assert_eq!(
        encode_js5_handshake(910, 1, "token", 0).unwrap(),
        vec![15, 15, 0, 0, 3, 142, 0, 0, 0, 1, 116, 111, 107, 101, 110, 0, 0]
    );
    assert_eq!(encode_js5_handshake(910, 1, "", 7).unwrap()[11], 7);
    assert!(encode_js5_handshake(910, 1, "😀", 0).is_err());
}

/// The group checksum is the IEEE CRC-32; the name hash of "huffman" is the 31-multiplier
/// string hash.
#[test]
fn crc_and_name_hash_are_the_standard_functions() {
    assert_eq!(getcrc(b"123456789"), 0xCBF4_3926_u32 as i32);
    // The name is lowercased first.
    assert_eq!(name_hash("HUFFMAN"), 1_258_058_669);
}
