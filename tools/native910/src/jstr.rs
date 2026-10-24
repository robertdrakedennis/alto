//! UTF-16 string semantics over the VM's Rust `String` lane.
//!
//! CS2 strings are sequences of UTF-16 code units that may hold unpaired
//! surrogates (`append_char` with a surrogate half, `substring` through a
//! surrogate pair, a typed surrogate half).
//! A Rust `String` cannot hold a lone surrogate, so the VM stores unit
//! `0xD800 + k` (k < 0x800) as the scalar `U+10F800 + k` — the top 2048 code
//! points of supplementary private use plane 16. A real scalar in that range
//! is stored as two escaped surrogate units, so it cannot collide with a lone
//! surrogate. [`units`] and [`from_units`]
//! translate at every point where indices, lengths or code units are
//! observable (`string_length`, `substring`, `string_indexof_*`,
//! `append_char`, font measurement, `compare`), so those ops see exactly
//! the UTF-16 sequence.

/// First scalar of the lone-surrogate escape range.
const ESCAPE_BASE: u32 = 0x10_F800;

/// The UTF-16 code units of a VM string.
pub fn units(s: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let v = u32::from(c);
        if v >= ESCAPE_BASE {
            out.push((0xD800 + (v - ESCAPE_BASE)) as u16);
        } else {
            let mut buf = [0; 2];
            out.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    out
}

/// The number of UTF-16 code units.
pub fn len(s: &str) -> usize {
    s.chars()
        .map(|c| {
            if u32::from(c) >= ESCAPE_BASE {
                1
            } else {
                c.len_utf16()
            }
        })
        .sum()
}

/// A VM string holding exactly these UTF-16 code units; paired
/// surrogates decode to their scalar, unpaired ones use the escape range.
pub fn from_units(units: &[u16]) -> String {
    let mut text = String::new();
    let escape = |unit: u16| {
        char::from_u32(ESCAPE_BASE + (u32::from(unit) - 0xD800))
            .unwrap_or(char::REPLACEMENT_CHARACTER)
    };
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(scalar) if u32::from(scalar) >= ESCAPE_BASE => {
                let mut pair = [0; 2];
                for unit in scalar.encode_utf16(&mut pair) {
                    text.push(escape(*unit));
                }
            }
            Ok(scalar) => text.push(scalar),
            Err(error) => text.push(escape(error.unpaired_surrogate())),
        }
    }
    text
}

/// Convert ordinary Rust Unicode text at an external VM boundary. Calling
/// this on an already encoded VM string would reinterpret its surrogate escapes.
pub fn from_text(text: &str) -> String {
    from_units(&text.encode_utf16().collect::<Vec<_>>())
}

/// Re-pair adjacent escaped surrogate halves after a concatenation so equal
/// UTF-16 strings stay equal Rust strings (`"\uD83D" + "\uDE00"` is `"😀"`).
pub fn normalize(s: String) -> String {
    if s.chars().any(|c| u32::from(c) >= ESCAPE_BASE) {
        from_units(&units(&s))
    } else {
        s
    }
}

/// `(char) code` as a VM string of one code unit.
pub fn from_unit(unit: u16) -> String {
    from_units(&[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lone_surrogates_round_trip_as_single_units() {
        let pair = "a😀z";
        assert_eq!(units(pair), pair.encode_utf16().collect::<Vec<_>>());
        assert_eq!(len(pair), 4);
        // substring(1, 2) of "a😀z" is the lone high surrogate.
        let high = from_units(&units(pair)[1..2]);
        assert_eq!(len(&high), 1);
        assert_eq!(units(&high), vec![0xD83D]);
        // Re-joining the halves restores the pair.
        let low = from_units(&units(pair)[2..3]);
        let joined = format!("{high}{low}");
        assert_eq!(units(&joined), vec![0xD83D, 0xDE00]);
        assert_eq!(normalize(joined), "😀");
        assert_eq!(from_unit(0xDFFF).chars().count(), 1);
        assert_eq!(units(&from_unit(0xDFFF)), vec![0xDFFF]);
        // Valid private-use pairs must stay two code units, even when their
        // scalar shares the transport range used for lone surrogates.
        for codepoint in ESCAPE_BASE..=u32::from(char::MAX) {
            let scalar = char::from_u32(codepoint).unwrap();
            let mut pair = [0; 2];
            let expected = scalar.encode_utf16(&mut pair);
            let encoded = from_units(expected);
            assert_eq!(units(&encoded), expected);
            assert_eq!(units(&from_text(&scalar.to_string())), expected);
            assert_eq!(len(&encoded), expected.len());
            assert_eq!(normalize(encoded.clone()), encoded);
        }
    }
}
