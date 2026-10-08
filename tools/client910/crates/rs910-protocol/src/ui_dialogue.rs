//! Dialogue transport CLIENT builders (thin-oracle).
//!
//! The `RESUME_P_*` and `ABORT_P_DIALOG` packets a CS2 dialogue resume sends:
//!
//! - count dialogue: the entered text is popped from the object stack; if it
//!   is a decimal `i32` (optional leading sign, at least one digit, no
//!   overflow) it is parsed, otherwise the count is 0. The packet is
//!   `RESUME_P_COUNTDIALOG` + `p4(count)` (big-endian).
//! - name / string dialogue: the text is popped from the object stack and sent
//!   as `p1(length + 1)` (low byte) + a NUL-terminated Windows-1252 string.
//!   Unrepresentable chars encode as `?` (63); an embedded NUL cannot be
//!   sent, `None` here. `length` counts UTF-16 units, so astral codepoints
//!   cost 2 `?` bytes and 2 length units.
//! - object / HSL dialogue: an int is popped from the int stack and sent as
//!   `p2(value)` (big-endian, low 16 bits).
//! - abort dialogue: `ABORT_P_DIALOG` with no payload (queued inline by the UI
//!   runtime; no builder here).
//!
//! Wire: `crate::proto::client::RESUME_P_COUNTDIALOG` (10, size 4),
//! `RESUME_P_STRINGDIALOG` (3, size -1), `RESUME_P_NAMEDIALOG` (101,
//! size -1), `RESUME_P_OBJDIALOG` (99, size 2), `RESUME_P_HSLDIALOG`
//! (1, size 2) and `crate::proto::client::ABORT_P_DIALOG` (106, size 0).
//!
//! Mirrors the `ui_player_options.rs` builder pattern: bare
//! `(opcode, payload)` tuples with no dispatch or state.

/// The Windows-1252 codec lives in rs910-core (Phase 2.1); the
/// re-export keeps `crate::ui_dialogue::cp1252_*` call sites compiling.
pub use rs910_core::cp1252::{cp1252_decode_byte, cp1252_encode_unit};

/// `RESUME_P_COUNTDIALOG` payload for an already-resolved count.
///
/// The count is a big-endian `p4`: `b24, b16, b8, b0`.
pub fn build_resume_count(value: i32) -> (u8, [u8; 4]) {
    (
        crate::proto::client::RESUME_P_COUNTDIALOG,
        [
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ],
    )
}

/// The count dialogue resume: the count is 0 unless the text is a decimal
/// `i32`, then `p4(count)`. Non-numeric (including empty, sign-only, or overflowing) input sends 0.
pub fn build_resume_count_str(text: &str) -> (u8, [u8; 4]) {
    let value = if is_decimal_int(text) {
        parse_decimal_int(text)
    } else {
        0
    };
    build_resume_count(value)
}

/// The string dialogue resume: `p1(length + 1)` +
/// `pjstr`. Returns `None` for text that cannot be sent (an embedded NUL). Payload is the bytes after the opcode, length prefix
/// included: `[p1, cp1252..., 0]`.
pub fn build_resume_string(text: &str) -> Option<(u8, Vec<u8>)> {
    encode_dialog_text(crate::proto::client::RESUME_P_STRINGDIALOG, text)
}

/// The name dialogue resume: `p1(length + 1)` +
/// `pjstr`. Same wire shape as `build_resume_string` on opcode 101.
/// Returns `None` on an embedded NUL.
pub fn build_resume_name(text: &str) -> Option<(u8, Vec<u8>)> {
    encode_dialog_text(crate::proto::client::RESUME_P_NAMEDIALOG, text)
}

/// The object dialogue resume: `p2(value)` (big-endian hi, lo; low 16 bits).
pub fn build_resume_obj(value: i32) -> (u8, [u8; 2]) {
    (
        crate::proto::client::RESUME_P_OBJDIALOG,
        [(value >> 8) as u8, value as u8],
    )
}

/// The HSL dialogue resume: `p2(value)` (big-endian hi, lo; low 16 bits).
pub fn build_resume_hsl(value: i32) -> (u8, [u8; 2]) {
    (
        crate::proto::client::RESUME_P_HSLDIALOG,
        [(value >> 8) as u8, value as u8],
    )
}

/// Shared `p1(len + 1) + pjstr` encoder.
/// `len` is the string length in UTF-16 units; each unit encodes to
/// exactly one byte, unrepresentable units (including lone surrogates) to
/// `?` (`63`). `p1` keeps the low byte, like a byte cast.
fn encode_dialog_text(opcode: u8, text: &str) -> Option<(u8, Vec<u8>)> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.contains(&0) {
        return None;
    }
    let prefix = (units.len() + 1) as u8;
    let mut payload = Vec::with_capacity(units.len() + 2);
    payload.push(prefix);
    for unit in units {
        payload.push(cp1252_encode_unit(unit));
    }
    payload.push(0);
    Some((opcode, payload))
}

/// Whether the text is a decimal `i32`: optional
/// leading `-` or `+` at index 0 only,
/// ASCII decimal digits, at least one digit, and an `i32` overflow check
/// (a value that does not divide back rejects).
fn is_decimal_int(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut negative = false;
    let mut value: i32 = 0;
    let mut any = false;
    for (i, &b) in bytes.iter().enumerate() {
        if i == 0 {
            if b == b'-' {
                negative = true;
                continue;
            }
            if b == b'+' {
                continue;
            }
        }
        if !b.is_ascii_digit() {
            return false;
        }
        let mut digit = (b - b'0') as i32;
        if negative {
            digit = -digit;
        }
        let next = 10i32.wrapping_mul(value).wrapping_add(digit);
        if next / 10 != value {
            return false;
        }
        value = next;
        any = true;
    }
    any
}

/// Parse a decimal `i32`. The caller only invokes this after
/// [`is_decimal_int`] passes, so the wrapping arithmetic cannot overflow here.
fn parse_decimal_int(text: &str) -> i32 {
    let bytes = text.as_bytes();
    let mut negative = false;
    let mut value: i32 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if i == 0 {
            if b == b'-' {
                negative = true;
                continue;
            }
            if b == b'+' {
                continue;
            }
        }
        let mut digit = (b - b'0') as i32;
        if negative {
            digit = -digit;
        }
        value = 10i32.wrapping_mul(value).wrapping_add(digit);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_count_str_numeric_sends_p4_be() {
        // "123" is numeric -> `p4(123)`.
        assert_eq!(build_resume_count_str("123"), (10, [0, 0, 0, 123]));
        assert_eq!(build_resume_count(123), (10, [0, 0, 0, 123]));
    }

    #[test]
    fn resume_count_str_non_numeric_sends_zero() {
        // The count is 0 unless the text is a decimal `i32`.
        assert_eq!(build_resume_count_str("abc"), (10, [0, 0, 0, 0]));
        assert_eq!(build_resume_count_str(""), (10, [0, 0, 0, 0]));
        assert_eq!(build_resume_count_str("-"), (10, [0, 0, 0, 0]));
        assert_eq!(build_resume_count_str("12a"), (10, [0, 0, 0, 0]));
        assert_eq!(build_resume_count_str(" 12"), (10, [0, 0, 0, 0]));
        // Overflow guard: an out-of-`i32` decimal is non-numeric.
        assert_eq!(build_resume_count_str("9999999999"), (10, [0, 0, 0, 0]));
    }

    #[test]
    fn resume_count_p4_byte_order_matches_packet_p4() {
        // Big-endian `p4`: `b24, b16, b8, b0`.
        assert_eq!(build_resume_count(0x01020304), (10, [1, 2, 3, 4]));
        assert_eq!(build_resume_count(-1), (10, [0xFF, 0xFF, 0xFF, 0xFF]));
        assert_eq!(build_resume_count(i32::MIN), (10, [0x80, 0, 0, 0]));
        assert_eq!(build_resume_count_str("-1"), (10, [0xFF, 0xFF, 0xFF, 0xFF]));
        assert_eq!(build_resume_count_str("+42"), (10, [0, 0, 0, 42]));
    }

    #[test]
    fn resume_string_p1_pjstr_roundtrip() {
        // `p1(len + 1) + pjstr`.
        assert_eq!(build_resume_string("hi"), Some((3, vec![3, b'h', b'i', 0])));
        // Empty string: `p1(1)` + bare NUL.
        assert_eq!(build_resume_string(""), Some((3, vec![1, 0])));
    }

    #[test]
    fn resume_name_p1_pjstr_roundtrip() {
        // Same shape on opcode 101.
        assert_eq!(
            build_resume_name("Zezima"),
            Some((101, vec![7, b'Z', b'e', b'z', b'i', b'm', b'a', 0]))
        );
        assert_eq!(build_resume_name(""), Some((101, vec![1, 0])));
    }

    #[test]
    fn resume_string_non_cp1252_maps_to_question_mark() {
        // Unrepresentable -> `?` (63), one
        // byte per UTF-16 unit, never raw UTF-8.
        assert_eq!(build_resume_string("Ā"), Some((3, vec![2, b'?', 0])));
        // Astral codepoint: the length is 2 units -> `p1(3)`, two `?`.
        assert_eq!(build_resume_string("😀"), Some((3, vec![3, b'?', b'?', 0])));
        // `€` is representable as `0x80`.
        assert_eq!(build_resume_string("€"), Some((3, vec![2, 0x80, 0])));
        // 0x80 reads back as `€`, unassigned bytes decode to `?`.
        assert_eq!(cp1252_decode_byte(0x80), '€');
        assert_eq!(cp1252_decode_byte(0x81), '?');
    }

    #[test]
    fn resume_string_nul_is_rejected() {
        // An embedded NUL cannot be sent.
        assert_eq!(build_resume_string("a\0b"), None);
        assert_eq!(build_resume_name("a\0b"), None);
        assert_eq!(build_resume_string("\0"), None);
    }

    #[test]
    fn resume_obj_p2_be_roundtrip() {
        // `p2`: hi, lo.
        assert_eq!(build_resume_obj(0x1234), (99, [0x12, 0x34]));
        assert_eq!(build_resume_obj(0), (99, [0, 0]));
        // Byte casts keep the low 16 bits of a negative int.
        assert_eq!(build_resume_obj(-1), (99, [0xFF, 0xFF]));
    }

    #[test]
    fn resume_hsl_p2_be_roundtrip() {
        // `p2`: hi, lo.
        assert_eq!(build_resume_hsl(0x0102), (1, [1, 2]));
        assert_eq!(build_resume_hsl(0), (1, [0, 0]));
        assert_eq!(build_resume_hsl(-2), (1, [0xFF, 0xFE]));
    }
}
