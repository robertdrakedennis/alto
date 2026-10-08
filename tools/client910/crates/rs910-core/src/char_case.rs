//! Per-UTF-16-unit letter case, the way the game's string commands and the
//! name sorter see it.
//!
//! The client works on single 16-bit units (never on whole scalar values), and
//! it uses the *simple* one-to-one case mappings of the Unicode Character
//! Database: `ß` stays `ß` when upper-cased, `İ` lower-cases to `i`, and a
//! lone surrogate maps to itself. Rust's `char::to_uppercase` implements the
//! *full* mappings (which may produce several characters), so this module
//! narrows them to the simple mapping and patches the few places where the two
//! differ or where the Unicode revision Rust ships differs from the revision
//! the behaviour was frozen against (Unicode 15):
//!
//! - **Greek with iota subscript** (U+1F80..=U+1FAF, U+1FB3, U+1FC3, U+1FF3):
//!   their full upper-case mapping is two letters; the simple mapping is the
//!   title-case letter (`U+1F80..=U+1F87` -> `+8`, and the three lone letters
//!   to `U+1FBC`, `U+1FCC`, `U+1FFC`).
//! - **Later Unicode additions** (Latin Extended-D and Cyrillic Extended-C
//!   letters listed in [`FROZEN_UNASSIGNED_CASE_PAIRS`], plus the two Latin
//!   letters `U+019B`/`U+0264` that gained an upper-case partner): later
//!   Unicode revisions assign case pairs that Unicode 15 does not have; the
//!   client keeps these units caseless.
//! - **Title-case letters** are counted as "upper case" by
//!   [`is_upper_or_title`], next to the `Uppercase` property.
//!
//! `tests` freezes a digest of the whole 16-bit range, so a toolchain
//! upgrade that changes any answer fails loudly instead of silently changing
//! name sorting.

/// Code points whose case pair exists in newer Unicode revisions but not in
/// the revision the client behaviour is frozen against. Both directions of
/// each pair are listed; the mapping stays the identity for them.
const FROZEN_UNASSIGNED_CASE_PAIRS: &[u16] = &[
    0x019B, 0x0264, 0x1C89, 0x1C8A, 0xA7CB, 0xA7CC, 0xA7CD, 0xA7CE, 0xA7CF, 0xA7D2, 0xA7D3, 0xA7D4,
    0xA7D5, 0xA7DA, 0xA7DB, 0xA7DC,
];

/// Title-case letters (general category Lt) in the 16-bit range.
const TITLE_CASE: &[u16] = &[
    0x01C5, 0x01C8, 0x01CB, 0x01F2, 0x1F88, 0x1F89, 0x1F8A, 0x1F8B, 0x1F8C, 0x1F8D, 0x1F8E, 0x1F8F,
    0x1F98, 0x1F99, 0x1F9A, 0x1F9B, 0x1F9C, 0x1F9D, 0x1F9E, 0x1F9F, 0x1FA8, 0x1FA9, 0x1FAA, 0x1FAB,
    0x1FAC, 0x1FAD, 0x1FAE, 0x1FAF, 0x1FBC, 0x1FCC, 0x1FFC,
];

fn scalar(unit: u16) -> Option<char> {
    char::from_u32(u32::from(unit))
}

fn single(mut mapped: impl ExactSizeIterator<Item = char>) -> Option<u16> {
    if mapped.len() != 1 {
        return None;
    }
    mapped.next().and_then(|c| u16::try_from(u32::from(c)).ok())
}

/// The simple upper-case mapping of one UTF-16 unit.
pub fn to_upper(unit: u16) -> u16 {
    if FROZEN_UNASSIGNED_CASE_PAIRS.contains(&unit) {
        return unit;
    }
    let Some(c) = scalar(unit) else {
        return unit;
    };
    if let Some(mapped) = single(c.to_uppercase()) {
        return mapped;
    }
    match unit {
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => unit + 8,
        0x1FB3 => 0x1FBC,
        0x1FC3 => 0x1FCC,
        0x1FF3 => 0x1FFC,
        _ => unit,
    }
}

/// The simple lower-case mapping of one UTF-16 unit.
pub fn to_lower(unit: u16) -> u16 {
    if FROZEN_UNASSIGNED_CASE_PAIRS.contains(&unit) {
        return unit;
    }
    let Some(c) = scalar(unit) else {
        return unit;
    };
    if let Some(mapped) = single(c.to_lowercase()) {
        return mapped;
    }
    // The only multi-letter lower-case mapping in the 16-bit range is
    // `İ` (U+0130) -> `i` + combining dot; its simple mapping is `i`.
    if unit == 0x0130 {
        return 0x69;
    }
    unit
}

/// Whether the unit is an upper-case or title-case letter.
pub fn is_upper_or_title(unit: u16) -> bool {
    if FROZEN_UNASSIGNED_CASE_PAIRS.contains(&unit) {
        return false;
    }
    scalar(unit).is_some_and(char::is_uppercase) || TITLE_CASE.contains(&unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a over (upper, lower, upper-or-title) of every 16-bit unit, frozen
    /// from the complete per-unit table the behaviour was originally taken
    /// from. Any drift in the toolchain's Unicode data fails here.
    #[test]
    fn whole_16_bit_range_is_unchanged() {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |byte: u8| {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        };
        for unit in 0..=u16::MAX {
            for byte in to_upper(unit).to_le_bytes() {
                feed(byte);
            }
            for byte in to_lower(unit).to_le_bytes() {
                feed(byte);
            }
            feed(u8::from(is_upper_or_title(unit)));
        }
        assert_eq!(hash, 0x3951_4c8e_3f67_a508);
    }

    #[test]
    fn simple_mappings_not_full_mappings() {
        assert_eq!(to_upper(0xDF), 0xDF, "ß has no simple upper-case");
        assert_eq!(to_lower(0x130), 0x69, "İ lower-cases to plain i");
        assert_eq!(
            to_upper(0x1F80),
            0x1F88,
            "iota-subscript Greek maps to title case"
        );
        assert_eq!(to_upper(0xD83D), 0xD83D, "a lone surrogate maps to itself");
        assert!(is_upper_or_title(0x1C5) && !is_upper_or_title(0x1C6));
    }
}
