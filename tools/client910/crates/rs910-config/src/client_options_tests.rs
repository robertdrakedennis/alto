//! Thin file-matrix oracles for ClientOptions: the v38 block layout, the
//! legacy (v22) reader, defaults for missing or corrupt files, ordered
//! clamping, the presets and the autosetup thresholds. Temp dirs only: every
//! file test uses `std::env::temp_dir()`.
use super::{ClientOptions, Profile};

fn profile() -> Profile {
    Profile {
        max_memory_mb: 512,
        cpu_count: 2,
        arm: false,
        windows: false,
        unused: false,
        mode_game: 0,
        jagdx: false,
        initial_display_mode: 1,
    }
}

fn tmp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("alto-client-options-{}-{name}", std::process::id()))
}

fn tmp_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "alto-client-options-dir-{}-{name}",
        std::process::id()
    ))
}

#[test]
fn v38_encode_matches_the_block_layout_and_round_trips() {
    // The block is version byte 38 then one byte per field in fixed order
    // (two reserved zero slots, two bytes each for unused9-12), 58 bytes.
    let o = ClientOptions::new(profile());
    let bytes = o.encode();
    assert_eq!(bytes.len(), 58);
    assert_eq!(bytes[0], 38);
    // Expected default block for profile(): 512MB, dual-CPU, non-ARM,
    // non-Windows, mode 0, display 1. Field order mirrors 630-684.
    let mut expected: Vec<u8> = vec![
        38, // version
        1,  // animDetail
        0,  // antiAliasing
        1,  // unused
        0,  // bloom
        3,  // brightness
        0,  // buildArea
        0,  // drawDistance
        1,  // flickeringEffects
        1,  // fog
        1,  // groundBlending
        1,  // groundDecoration
        1,  // idleAnimations
        1,  // lightingDetail
        2,  // sceneryShadows
        1,  // shadowQuality
        0,  // reserved (v34 slot)
        0,  // orthographic
        2,  // particles
        2,  // removeRoofs
        0,  // screenSize (software(toolkit=1) is false, so 0)
        1,  // skyboxes
        1,  // characterShadows
        1,  // textures
        1,  // toolkit (non-Windows default)
        0,  // reserved
        1,  // waterDetail
        2,  // windowMode (non-ARM default)
        1,  // unused1
        2,  // unused2
        0,  // unused3
        0,  // unused4
        1,  // unused5
        1,  // unused6
        0,  // unused7
        0,  // unused8
    ];
    for v in [70, 30, 100, 100] {
        expected.extend_from_slice(&(v as u16).to_be_bytes());
    }
    expected.extend_from_slice(&[
        1,   // customCursors (decoded constructor ignores the byte, always 1)
        0,   // preset
        4,   // cpuUsage (dual-CPU default)
        0,   // loadingScreen
        0,   // safeMode
        0,   // unknown7 (toolkit 1 is neither 3 nor 5)
        254, // unused13 (-2 as p1 byte)
        1,   // consoleKeyPress
        127, // soundVolume
        127, // backgroundSoundVolume
        127, // speechVolume
        127, // unknownVolume1
        127, // unknownVolume2
        1,   // stereo
    ]);
    assert_eq!(expected.len(), 58);
    assert_eq!(
        bytes, expected,
        "v38 default block must match the block layout"
    );
    // Decode round-trip: read every field, then clamp.
    // Note: `unused13` defaults to -2, but the block writes it as the byte
    // 0xFE and the reader reads it back as 254; the clamp (`if < -3`) keeps
    // 254. So the first decode maps -2 -> 254, and the encoding stays
    // byte-stable afterwards.
    let decoded = ClientOptions::decode(&bytes, profile()).unwrap();
    assert_eq!(decoded.encode(), bytes);
    for name in super::FIELDS {
        if name == "unused13" {
            continue;
        }
        assert_eq!(
            decoded.get(name),
            o.get(name),
            "round-trip must preserve {name}"
        );
    }
    assert_eq!(decoded.get("unused13"), Some(254));
    // Mutated round-trip preserves edited fields byte-for-byte.
    let mut edited = ClientOptions::new(profile());
    assert_eq!(edited.set_field("soundVolume", 42), Some(true));
    assert_eq!(edited.set_field("brightness", 0), Some(true));
    let reloaded = ClientOptions::decode(&edited.encode(), profile()).unwrap();
    assert_eq!(reloaded.get("soundVolume"), Some(42));
    assert_eq!(reloaded.get("brightness"), Some(0));
    assert_eq!(reloaded.encode(), edited.encode());
}

fn build_legacy_v22() -> Vec<u8> {
    // The legacy (version 22) reader. Field order below is the file order
    // (skips are a byte advance, discards are reads whose values are dropped).
    let mut out = vec![22u8];
    out.push(2); // brightness
    out.push(9); // skipped (pos++)
    out.push(1); // removeRoofs raw -> +1 = 2
    out.push(1); // groundDecoration
    out.push(9); // skipped (pos++)
    out.push(1); // idleAnimations (later overwritten by v>=12 read)
    out.push(1); // flickeringEffects
    out.push(9); // discarded g1
    out.push(1); // characterShadows
    out.push(2); // first scenery candidate
    out.push(1); // second scenery candidate (v>=17) -> max = 2
    out.push(1); // first lighting bit (v>=2)
    out.push(1); // second lighting bit (v>=17) -> lighting 1
    out.push(1); // waterDetail
    out.push(1); // fog
    out.push(0); // antiAliasing
    out.push(1); // stereo
    out.push(100); // soundVolume
    out.push(90); // speechVolume (v>=20)
    out.push(80); // unknownVolume1
    out.push(70); // backgroundSoundVolume
    out.push(60); // unknownVolume2 (v>=21)
    out.extend_from_slice(&[0, 0]); // g2 (v>=1)
    out.extend_from_slice(&[0, 0]); // g2 (v>=1)
                                    // v22 not in 3..6 so no extra g1.
    out.push(2); // particles (v>=4)
    out.extend_from_slice(&[0, 0, 0, 0]); // g4s
    out.push(2); // windowMode (v>=6)
    out.push(0); // safeMode (v>=7)
    out.push(9); // discarded g1 (v>=8)
    out.push(0); // buildArea (v>=9)
    out.push(0); // bloom (v>=10)
    out.push(1); // customCursors byte (ignored on load -> 1)
    out.push(0); // idleAnimations overwrite (v>=12)
    out.push(1); // groundBlending (v>=13)
    out.push(1); // toolkit (v>=14)
    out.push(4); // cpuUsage (v>=15)
    out.push(1); // textures (v>=16)
    out.push(3); // preset (v>=18)
    out.push(1); // screenSize (v>=19)
    out.push(0); // loadingScreen (v>=22)
    out
}

#[test]
fn legacy_below_23_restores_defaults_for_missing_fields() {
    // Versions below 23 run the legacy reader, then reset the defaults for
    // everything it does not set: every modern-only slot
    // (animDetail, drawDistance, shadowQuality, orthographic, unused*,
    // volumes beyond legacy, consoleKeyPress, ...) falls back to defaults.
    // Corrupt legacy throws inside the try and also ends in full defaults.
    let p = profile();
    let defaults = ClientOptions::new(p);
    let legacy = build_legacy_v22();
    assert_eq!(legacy.len(), 46);
    let decoded = ClientOptions::decode(&legacy, p).unwrap();
    // Legacy-mapped non-audio values survive (and are clamped, see below).
    assert_eq!(decoded.get("brightness"), Some(2));
    assert_eq!(decoded.get("removeRoofs"), Some(2));
    assert_eq!(decoded.get("sceneryShadows"), Some(2));
    assert_eq!(decoded.get("lightingDetail"), Some(1));
    assert_eq!(decoded.get("particles"), Some(2));
    assert_eq!(decoded.get("preset"), Some(3));
    // Audio volumes are always reset by the defaults pass, which refreshes
    // sound/background/speech/unknownVolume1/unknownVolume2/stereo even though
    // the legacy reader just parsed them: the legacy audio bytes are
    // discarded.
    for name in [
        "soundVolume",
        "backgroundSoundVolume",
        "speechVolume",
        "unknownVolume1",
        "unknownVolume2",
        "stereo",
    ] {
        assert_eq!(
            decoded.get(name),
            defaults.get(name),
            "legacy audio {name} resets to defaults per setDefaultPreferences(false,true)"
        );
    }
    // Modern-only slots restore defaults, not zero.
    for name in [
        "animDetail",
        "drawDistance",
        "shadowQuality",
        "orthographic",
        "unused9",
        "unused10",
        "unused11",
        "unused12",
        "consoleKeyPress",
        "unused13",
    ] {
        assert_eq!(
            decoded.get(name),
            defaults.get(name),
            "legacy must default {name} per setDefaultPreferences(false,true)"
        );
    }
    // Corrupt legacy (version byte only + one byte) hits the catch at 255-256
    // and returns full defaults.
    let mut corrupt = vec![0u8; 2];
    corrupt[0] = 22;
    assert_eq!(ClientOptions::decode(&corrupt, p).unwrap(), defaults);
    let mut short_v0 = vec![0u8; 1];
    short_v0[0] = 0;
    assert_eq!(ClientOptions::decode(&short_v0, p).unwrap(), defaults);
}

#[test]
fn future_above_38_restores_defaults() {
    // A version above 38 falls back to full defaults.
    let p = profile();
    let defaults = ClientOptions::new(p);
    let full = defaults.encode();
    for version in [39u8, 40, 100, 255] {
        let mut bytes = full.clone();
        bytes[0] = version;
        assert_eq!(
            ClientOptions::decode(&bytes, p).unwrap(),
            defaults,
            "version {version} must fall back to defaults"
        );
    }
}

#[test]
fn truncated_and_corrupt_files_load_defaults() {
    // Loading wraps the whole read and decode and returns the defaults on
    // any failure. Truncation of a modern block fails the read; truncation of
    // a legacy block is caught inside the legacy path and already yields
    // defaults.
    let p = profile();
    let defaults = ClientOptions::new(p);
    let full = defaults.encode();

    // Missing file -> defaults.
    let missing = tmp_path("missing-preferences.dat");
    let _ = std::fs::remove_file(&missing);
    assert_eq!(ClientOptions::load(&missing, p), defaults);

    // Empty file -> defaults (decode g1 fails, load catches).
    let empty = tmp_path("empty-preferences.dat");
    std::fs::write(&empty, []).unwrap();
    assert_eq!(ClientOptions::load(&empty, p), defaults);

    // Truncated modern block -> decode Err, load defaults.
    for len in [0, 1, 3, full.len() - 1] {
        assert!(
            ClientOptions::decode(&full[..len], p).is_err(),
            "modern prefix len {len} must fail direct decode"
        );
        let path = tmp_path(&format!("truncated-{len}.dat"));
        std::fs::write(&path, &full[..len]).unwrap();
        let loaded = ClientOptions::load(&path, p);
        assert_eq!(
            loaded, defaults,
            "truncated file len {len} must load defaults"
        );
        // Clamp state, not just bytes: defaults are already clamped.
        assert_eq!(loaded.encode(), defaults.encode());
        let _ = std::fs::remove_file(&path);
    }

    // Corrupt version byte with garbage body -> defaults.
    let corrupt = tmp_path("corrupt-preferences.dat");
    std::fs::write(&corrupt, [38u8, 99, 99]).unwrap();
    assert_eq!(ClientOptions::load(&corrupt, p), defaults);
    // Out-of-range version byte alone: empty-body v38 is truncated (Err),
    // bare v99 decodes to defaults via the >38 fallback.
    assert!(ClientOptions::decode(&[38u8], p).is_err());
    assert_eq!(ClientOptions::decode(&[99u8], p).unwrap(), defaults);

    let _ = std::fs::remove_file(&empty);
    let _ = std::fs::remove_file(&corrupt);
}

#[test]
fn atomic_save_has_no_torn_reads() {
    // Saving writes the block through a temporary file and a rename, so
    // `load` sees either the previous complete block or the new complete
    // block, never a torn prefix.
    let dir = tmp_dir("no-torn");
    let path = dir.join("preferences.dat");
    let _ = std::fs::remove_file(&path);
    let mut first = ClientOptions::new(profile());
    first.set_field("soundVolume", 11).unwrap();
    first.save(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap().len(), 58);
    // Simulate a torn write next to the real file: a prefix alone must load
    // as defaults, never as half-applied state.
    let torn = tmp_path("torn-prefix.dat");
    let full = first.encode();
    std::fs::write(&torn, &full[..10]).unwrap();
    assert_eq!(
        ClientOptions::load(&torn, profile()),
        ClientOptions::new(profile())
    );
    // Overwrite atomically; no tmp residue; new bytes fully visible to a
    // fresh loader (including out-of-range-for-UI volume 200 and stereo off).
    let mut second = ClientOptions::new(profile());
    second.set_field("soundVolume", 77).unwrap();
    second.set_field("brightness", 0).unwrap();
    second.set_field("speechVolume", 200).unwrap();
    second.set_field("stereo", 0).unwrap();
    second.save(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), second.encode());
    let reloaded = ClientOptions::load(&path, profile());
    assert_eq!(reloaded.get("soundVolume"), Some(77));
    assert_eq!(reloaded.get("brightness"), Some(0));
    assert_eq!(reloaded.get("speechVolume"), Some(200));
    assert_eq!(reloaded.get("stereo"), Some(0));
    assert_eq!(reloaded.encode(), second.encode());
    assert!(!path
        .with_extension(format!("{}.tmp", std::process::id()))
        .exists());
    for entry in std::fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        assert!(
            !entry.file_name().to_string_lossy().ends_with(".tmp"),
            "no tmp residue in {dir:?}"
        );
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&torn);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn ordered_dependent_clamping_holds_after_load() {
    // Clamping runs in a fixed order and every read ends with it; setting a
    // preference clamps too. Assert post-load values, not just bytes.
    let p = profile();
    let mut base = ClientOptions::new(p).encode();
    // Byte offsets in the v38 block (0 = version): fog=9, groundBlending=10,
    // sceneryShadows=14, textures=23, brightness=5, antiAliasing=2.
    base[10] = 0; // groundBlending 0 forces fog 0 (clamp slot 10 checks slot 11)
    base[9] = 1; // fog 1 -> must clamp to 0
    base[23] = 0; // textures 0 forces sceneryShadows 0 (slot 15 checks slot 24)
    base[14] = 2; // sceneryShadows 2 -> must clamp to 0
    base[5] = 99; // brightness out of 0..=4 -> default 3
    base[2] = 9; // antiAliasing out of 0..=2 -> default 0
    let loaded = ClientOptions::decode(&base, p).unwrap();
    assert_eq!(loaded.get("fog"), Some(0));
    assert_eq!(loaded.get("groundBlending"), Some(0));
    assert_eq!(loaded.get("sceneryShadows"), Some(0));
    assert_eq!(loaded.get("textures"), Some(0));
    assert_eq!(loaded.get("brightness"), Some(3));
    assert_eq!(loaded.get("antiAliasing"), Some(0));
    // Clamped encoding differs from the crafted invalid block but is stable.
    assert_ne!(loaded.encode(), base);
    assert_eq!(ClientOptions::decode(&loaded.encode(), p).unwrap(), loaded);
    // File-level: the same clamp holds when the invalid block comes from disk.
    let path = tmp_path("clamp-after-load.dat");
    std::fs::write(&path, &base).unwrap();
    let from_file = ClientOptions::load(&path, p);
    assert_eq!(from_file.get("fog"), Some(0));
    assert_eq!(from_file.get("sceneryShadows"), Some(0));
    assert_eq!(from_file.get("brightness"), Some(3));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn original_edge_cases_are_preserved() {
    let p = profile();
    let mut o = ClientOptions::new(p);
    assert_eq!(o.can_set("unused2", 3), Some(3));
    assert_eq!(o.set_field("unused2", 3), Some(false));
    assert_eq!(o.set_field("fog", 1), Some(true));
    assert_eq!(o.can_set("textures", 2), Some(1));
    let mut bytes = o.encode();
    // The persisted custom-cursor byte is ignored on load.
    let custom = 44;
    bytes[custom] = 0;
    let decoded = ClientOptions::decode(&bytes, p).unwrap();
    assert_eq!(decoded.get("customCursors"), Some(1));
}

fn cpu_profile(cpu_count: i32) -> Profile {
    Profile {
        cpu_count,
        ..profile()
    }
}

fn assert_fields(o: &ClientOptions, expected: &[(&str, i32)]) {
    for (field, value) in expected {
        assert_eq!(o.get(field), Some(*value), "preset field {field}");
    }
}

#[test]
fn presets_match_field_values_cpu_rule_and_round_trip() {
    // The field sequences of the high, medium, low and min presets. buildArea
    // is the standard build area (id 0). The cpuUsage column follows the CPU
    // count rule (more than one CPU gives 4, else 2), which is also the
    // default value of that preference.
    let high = [
        ("removeRoofs", 2),
        ("removeRoofs2", 2),
        ("groundDecoration", 1),
        ("groundBlending", 1),
        ("idleAnimations", 1),
        ("flickeringEffects", 1),
        ("characterShadows", 1),
        ("textures", 1),
        ("sceneryShadows", 2),
        ("lightingDetail", 1),
        ("waterDetail", 2),
        ("fog", 1),
        ("antiAliasing", 0),
        ("antiAliasing2", 0),
        ("particles", 2),
        ("buildArea", 0),
        ("bloom", 0),
        ("skyboxes", 1),
        ("animDetail", 1),
        ("screenSize", 0),
        ("preset", 4),
    ];
    let medium = [
        ("removeRoofs", 2),
        ("removeRoofs2", 2),
        ("groundDecoration", 1),
        ("groundBlending", 1),
        ("idleAnimations", 1),
        ("flickeringEffects", 1),
        ("characterShadows", 1),
        ("textures", 1),
        ("sceneryShadows", 1),
        ("lightingDetail", 1),
        ("waterDetail", 0),
        ("fog", 1),
        ("antiAliasing", 0),
        ("antiAliasing2", 0),
        ("particles", 1),
        ("buildArea", 0),
        ("bloom", 0),
        ("skyboxes", 1),
        ("animDetail", 1),
        ("screenSize", 1),
        ("preset", 3),
    ];
    let low = [
        ("removeRoofs", 1),
        ("removeRoofs2", 1),
        ("groundDecoration", 1),
        ("groundBlending", 1),
        ("idleAnimations", 0),
        ("flickeringEffects", 0),
        ("characterShadows", 0),
        ("sceneryShadows", 0),
        ("textures", 0),
        ("lightingDetail", 0),
        ("waterDetail", 0),
        ("fog", 0),
        ("antiAliasing", 0),
        ("antiAliasing2", 0),
        ("particles", 0),
        ("buildArea", 0),
        ("bloom", 0),
        ("skyboxes", 0),
        ("animDetail", 0),
        ("screenSize", 2),
        ("preset", 2),
    ];
    let min = [
        ("removeRoofs", 1),
        ("removeRoofs2", 1),
        ("groundDecoration", 0),
        ("fog", 0),
        ("groundBlending", 0),
        ("idleAnimations", 0),
        ("flickeringEffects", 0),
        ("characterShadows", 0),
        ("sceneryShadows", 0),
        ("textures", 0),
        ("lightingDetail", 0),
        ("waterDetail", 0),
        ("antiAliasing", 0),
        ("antiAliasing2", 0),
        ("particles", 0),
        ("buildArea", 0),
        ("bloom", 0),
        ("skyboxes", 0),
        ("animDetail", 0),
        ("screenSize", 2),
        ("preset", 1),
    ];
    // Single-CPU default already follows the same rule (slot 44).
    assert_eq!(ClientOptions::new(cpu_profile(1)).get("cpuUsage"), Some(2));
    assert_eq!(ClientOptions::new(cpu_profile(2)).get("cpuUsage"), Some(4));
    for cpu_count in [1, 2, 8] {
        let expected_cpu = if cpu_count > 1 { 4 } else { 2 };
        let cases: &[(_, _, &[(&str, i32)])] = &[
            ("high", 2, &high),
            ("medium", 1, &medium),
            ("low", 0, &low),
            ("min", 0, &min),
        ];
        for (label, expected_particles, fields) in cases {
            let mut o = ClientOptions::new(cpu_profile(cpu_count));
            match *label {
                "high" => o.apply_high_preset(),
                "medium" => o.apply_medium_preset(),
                "low" => o.apply_low_preset(),
                _ => o.apply_min_preset(),
            }
            assert_fields(&o, fields);
            assert_eq!(o.get("cpuUsage"), Some(expected_cpu));
            assert_eq!(o.particle_level, *expected_particles);
            // The v38 block round-trips byte-for-byte.
            let bytes = o.encode();
            assert_eq!(bytes.len(), 58);
            assert_eq!(bytes[0], 38);
            let decoded = ClientOptions::decode(&bytes, cpu_profile(cpu_count)).unwrap();
            assert_eq!(decoded.encode(), bytes);
            for name in super::FIELDS {
                if name == "unused13" {
                    continue;
                }
                assert_eq!(decoded.get(name), o.get(name), "{label} {name}");
            }
        }
        // apply_preset routes 1..4 to the same sequences; 0 (custom) is a
        // no-op on graphics fields (it only writes the preset field itself,
        // which the dispatch path handles separately).
        for (id, fields) in [
            (1, &min[..]),
            (2, &low[..]),
            (3, &medium[..]),
            (4, &high[..]),
        ] {
            let mut o = ClientOptions::new(cpu_profile(cpu_count));
            o.apply_preset(id);
            assert_fields(&o, fields);
            assert_eq!(o.get("cpuUsage"), Some(expected_cpu));
        }
        let mut o = ClientOptions::new(cpu_profile(cpu_count));
        let before = o.clone();
        o.apply_preset(0);
        assert_eq!(o, before);
    }
}

#[test]
fn autosetup_result_mapping_matches_thresholds() {
    // Autosetup: a maximum memory below 96 forces preset 1; otherwise the CPU
    // profile's millisecond value maps <=100 → 4, <=500 → 3, <=1003 → 2, else
    // 1. The profiler device run and the display mode / toolkit / save tail
    // are partial and intentionally excluded here.
    for (cpu_ms, maxmemory, expected) in [
        (50, 512, 4),
        (100, 512, 4),
        (101, 512, 3),
        (500, 512, 3),
        (501, 512, 2),
        (1003, 512, 2),
        (1004, 512, 1),
        (50_000, 512, 1),
        (50, 95, 1),
        (50, 96, 4),
        (1004, 95, 1),
    ] {
        assert_eq!(
            ClientOptions::autosetup_preset_for_cpu_profile(cpu_ms, maxmemory),
            expected,
            "cpu {cpu_ms}ms mem {maxmemory}MB"
        );
    }
    // The device metric: >100000 → 4, >50000 → 3, >10000 → 2, else 1 (the
    // device tail is excluded as partial).
    for (metric, expected) in [
        (100_001, 4),
        (100_000, 3),
        (50_001, 3),
        (50_000, 2),
        (10_001, 2),
        (10_000, 1),
        (0, 1),
        (-5, 1),
    ] {
        assert_eq!(
            ClientOptions::autosetup_preset_for_metric(metric),
            expected,
            "metric {metric}"
        );
    }
}
