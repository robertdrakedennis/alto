//! Golden tests of rs910-core modules against recordings. `trig` and `colour` compare
//! against `fixtures/recorded-goldens/` through [`crate::recorded_golden`], which
//! belongs to client910's test fixtures, so they stay in this crate
//! (tools/README.md "Crate conventions": tests that need client910 fixtures or
//! the pack stay in client910). `cp1252` checks against the original client's own
//! table (embedded) and `checksum` against the
//! standard CRC-32/Adler-32 check values.

mod trig {
    use crate::trig::*;

    /// The sin/cos/radians tables over the whole 14-bit domain and `atan2`
    /// over a grid, vs the recording (`fixtures/recorded-goldens/floor-oracle.txt`).
    #[test]
    fn tables_radians_and_atan2_match_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        golden.check("trig_sin", &(0..16384).map(sin).collect::<Vec<_>>());
        golden.check("trig_cos", &(0..16384).map(cos).collect::<Vec<_>>());
        let rad: Vec<i32> = (0..16384).map(|i| radians(i).to_bits() as i32).collect();
        golden.check("trig_radians", &rad);
        let mut grid = Vec::new();
        for y in (-300..=300).step_by(7) {
            for x in (-300..=300).step_by(5) {
                grid.push(atan2(y, x));
            }
        }
        golden.check("trig_atan2_grid", &grid);
    }

    /// The original rounding sends an exact tie towards +inf, so
    /// `atan2(y, x) * K == -(m + 0.5)` must give `-m`, not `-(m + 1)`. The
    /// `(y, x)` pairs are the ones the recording found to hit such ties
    /// exactly, with the recorded result (`trig_atan2_ties`: y, x, atan2 triples).
    #[test]
    fn atan2_rounds_negative_ties_up_like_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let ties = golden.values("trig_atan2_ties");
        assert!(ties.len() >= 30, "{} tie triples", ties.len() / 3);
        for t in ties.chunks_exact(3) {
            assert_eq!(atan2(t[0], t[1]), t[2], "atan2({}, {})", t[0], t[1]);
        }
    }
}

mod colour {
    use crate::colour::*;

    /// The colour tables vs the original client's recording (committed as
    /// `fixtures/recorded-goldens/floor-oracle.txt`). the BGR table is what every
    /// GPU floor/model colour indexes, so a gamma or channel slip recolours
    /// the whole scene.
    #[test]
    fn hsl_tables_match_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let t = build_hsl_tables();
        golden.check("colour_rgb_table", &t.rgb);
        golden.check("colour_bgr_table", &t.bgr);
        golden.check("colour_hsv_table", &build_hsv_table());
        // The cached accessor is the same table the toolkits read.
        golden.check("colour_bgr_table", &hsl_tables().bgr);
    }

    /// The saturation renormalisation over its whole domain, `hslToRgb` over
    /// every 1029th 24-bit value and `hsl24to16` over a lattice, vs the
    /// recording.
    #[test]
    fn hsl_functions_match_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let renormalised: Vec<i32> = (0..65536).map(renormalise_saturation).collect();
        golden.check("colour_saturation_renormalise", &renormalised);
        let mut h2r = Vec::new();
        let mut rgb = 0_i32;
        while (0..0x0100_0000).contains(&rgb) {
            h2r.push(rgb24_to_hsl16(rgb));
            rgb += 1029;
        }
        golden.check("colour_hslToRgb", &h2r);
        let mut packed = Vec::new();
        for hue in (0..256).step_by(5) {
            for sat in (0..256).step_by(7) {
                for lum in (0..256).step_by(3) {
                    packed.push(hsl24to16(hue, sat, lum));
                }
            }
        }
        golden.check("colour_hsl24to16", &packed);
    }
}

/// `rs910_core::cp1252` (the one Windows-1252 mapping the client uses) vs
/// the recorded client table, including native codecs and loading-screen text.
mod cp1252 {
    use rs910_core::cp1252::{cp1252, cp1252_decode_byte, cp1252_encode_unit};

    /// The original table: the UTF-16 unit of each byte `0x80..0xA0`, 0 where
    /// the byte is unassigned.
    const RECORDED_C1_TABLE: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];

    /// The five bytes the original table leaves at 0.
    const UNASSIGNED: [u8; 5] = [0x81, 0x8D, 0x8F, 0x90, 0x9D];

    /// The original decode for one non-NUL byte (the original drops NUL
    /// bytes from the string).
    fn recorded_decode(byte: u8) -> u16 {
        if (128..160).contains(&byte) {
            match RECORDED_C1_TABLE[usize::from(byte - 128)] {
                0 => u16::from(b'?'),
                unit => unit,
            }
        } else {
            u16::from(byte)
        }
    }

    /// The original encode: `1..128` and `160..=255` map to themselves, each
    /// table unit to its byte, everything else to `?`.
    fn recorded_encode(unit: u16) -> u8 {
        if (1..128).contains(&unit) || (160..=255).contains(&unit) {
            return unit as u8;
        }
        RECORDED_C1_TABLE
            .iter()
            .position(|&c| c != 0 && c == unit)
            .map_or(b'?', |i| 0x80 + i as u8)
    }

    #[test]
    fn decode_matches_the_recorded_table_on_every_byte() {
        for b in 1..=255_u8 {
            let want = recorded_decode(b);
            assert_eq!(cp1252_decode_byte(b) as u32, u32::from(want), "{b:#04x}");
            // The Option form agrees except on the unassigned bytes.
            let strict = cp1252(b).map(|c| c as u32);
            if UNASSIGNED.contains(&b) {
                assert_eq!(strict, None, "{b:#04x}");
            } else {
                assert_eq!(strict, Some(u32::from(want)), "{b:#04x}");
            }
        }
        // Spot values a table slip would move.
        assert_eq!(cp1252_decode_byte(0x80), '\u{20AC}');
        assert_eq!(cp1252_decode_byte(0x9F), '\u{0178}');
        assert_eq!(cp1252_decode_byte(0x8D), '?');
        assert_eq!(cp1252_decode_byte(0xA0), '\u{00A0}');
    }

    #[test]
    fn encode_matches_the_recorded_table_on_every_unit_and_round_trips() {
        for unit in 0..=u16::MAX {
            assert_eq!(
                cp1252_encode_unit(unit),
                recorded_encode(unit),
                "{unit:#06x}"
            );
        }
        // NUL, the C1 controls and unmapped units encode to `?`.
        for unit in [0_u16, 0x81, 0x9F, 0x0100, 0xD800, 0xFFFF] {
            assert_eq!(cp1252_encode_unit(unit), b'?', "{unit:#06x}");
        }
        // Round trip on every assigned byte except NUL (the original encodes 0 to `?`).
        for b in 1..=255_u8 {
            if !UNASSIGNED.contains(&b) {
                let unit = cp1252_decode_byte(b) as u32 as u16;
                assert_eq!(cp1252_encode_unit(unit), b, "{b:#04x}");
            }
        }
    }

    #[test]
    fn iface_decode_matches_the_recorded_table_on_every_byte() {
        assert_eq!(crate::iface::decode_cp1252(&[0]), "");
        for byte in 1..=u8::MAX {
            assert_eq!(
                crate::iface::decode_cp1252(&[byte]),
                char::from_u32(u32::from(recorded_decode(byte)))
                    .unwrap()
                    .to_string()
            );
        }
    }

    #[test]
    fn native910_gjstr_matches_the_recorded_table_on_every_byte() {
        for byte in 1..=u8::MAX {
            let bytes = [byte, 0];
            let native = native910::packet::Packet::new(&bytes).gjstr().unwrap();
            let expected = char::from_u32(u32::from(recorded_decode(byte)))
                .unwrap()
                .to_string();
            assert_eq!(native, expected);
            assert_eq!(
                rs910_config::ui_bytes::Cursor::new(&bytes).gjstr().unwrap(),
                expected
            );
        }
    }
}

/// `rs910_core::checksum` (JS5 `getcrc`, PNG and texture CRC/Adler) vs the
/// standard check values (zlib `crc32` / `adler32`).
mod checksum {
    use rs910_core::checksum::{adler32, crc32};

    #[test]
    fn crc32_and_adler32_match_standard_vectors() {
        let ones = vec![0xFF_u8; 70_000];
        let zeros = vec![0_u8; 70_000];
        // (data, CRC-32, Adler-32). "123456789" is the published check value
        // of both; the 70000-byte buffers pass Adler's 5552-byte modulo
        // window many times.
        let vectors: [(&[u8], u32, u32); 7] = [
            (b"", 0, 1),
            (b"a", 0xE8B7_BE43, 0x0062_0062),
            (b"123456789", 0xCBF4_3926, 0x091E_01DE),
            (b"Wikipedia", 0xADAA_C02E, 0x11E6_0398),
            (
                b"The quick brown fox jumps over the lazy dog",
                0x414F_A339,
                0x5BDC_0FDA,
            ),
            (&ones, 0x80F9_5A0A, 0x2A28_6E81),
            (&zeros, 0xA6A9_C8DC, 0x117F_0001),
        ];
        for (data, want_crc, want_adler) in vectors {
            assert_eq!(crc32(data), want_crc, "crc32 of {} bytes", data.len());
            // JS5 compares the signed 32-bit value of the CRC.
            assert_eq!(crate::js5net::getcrc(data), want_crc as i32);
            assert_eq!(adler32(data), want_adler, "adler32 of {} bytes", data.len());
        }
    }
}
