use super::*;

/// Replays a corpus of preference commands (`new`, `set`, `raw`, `clamp`,
/// `flag`, `reset`, `decode`, `particle`) and returns the state dump after
/// each row.
fn replay(input: &str) -> String {
    let mut fields = FIELDS.to_vec();
    fields.sort();
    let mut profile = Profile::default();
    let mut options = ClientOptions::new(profile);
    let mut output = String::new();
    for line in input.lines() {
        let a: Vec<_> = line.split(' ').collect();
        let n = |i: usize| a[i].parse::<i32>().unwrap();
        match a[0] {
            "particle" => {
                output.push_str(&format!(
                    "particle={},preference={}\n",
                    options.particle_level,
                    options.get("particles").unwrap()
                ));
                continue;
            }
            "new" => {
                profile = Profile {
                    max_memory_mb: n(1),
                    cpu_count: n(2),
                    arm: n(3) == 1,
                    windows: n(4) == 1,
                    mode_game: n(5),
                    jagdx: n(6) == 1,
                    initial_display_mode: n(7),
                    unused: false,
                };
                options = ClientOptions::new(profile);
            }
            "set" => {
                options.set_field(a[1], n(2)).unwrap();
            }
            "raw" => {
                options.values[ClientOptions::field_index(a[1]).unwrap()] = n(2);
            }
            "clamp" => options.clamp(),
            "flag" => {
                options.set_display_flag(a[1], n(2) == 1).unwrap();
            }
            "reset" => options.set_defaults(n(1) == 1, n(2) == 1),
            "decode" => {
                let bytes: Vec<u8> = if a[1] == "-" {
                    vec![]
                } else {
                    a[1].as_bytes()
                        .chunks_exact(2)
                        .map(|pair| {
                            u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
                        })
                        .collect()
                };
                match ClientOptions::decode(&bytes, profile) {
                    Ok(value) => options = value,
                    Err(_) => {
                        output.push_str("ERROR\n");
                        continue;
                    }
                }
            }
            _ => panic!("fixture command"),
        }
        let values = fields
            .iter()
            .map(|name| format!("{name}={}", options.get(name).unwrap()))
            .collect::<Vec<_>>()
            .join(",");
        let bytes = options
            .encode()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>();
        let mods = fields
            .iter()
            .map(|name| options.can_mod(name).map_or(-1, i32::from).to_string())
            .collect::<Vec<_>>()
            .join(",");
        let can = |value| {
            fields
                .iter()
                .map(|name| options.can_set(name, value).unwrap().to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        let tk = ClientOptions::field_index("toolkit").unwrap();
        let dm = ClientOptions::field_index("displayMode").unwrap();
        output.push_str(&format!(
            "{values}|{bytes}|{mods}|{}|{}|{},{},{},{}\n",
            can(0),
            can(1),
            i32::from(options.defaulted[tk]),
            i32::from(options.display_flags[tk]),
            i32::from(options.defaulted[dm]),
            i32::from(options.display_flags[dm])
        ));
    }
    output
}

/// The state machine against the frozen recording of the original client's
/// preference class, over the boundary/version corpus and the particle-level
/// coupling corpus.
#[test]
fn preferences_differential() {
    use rs910_core::test_support::frozen;
    for (input, recording) in [
        ("client-options/input.bin", "client-options/state"),
        (
            "client-options/particles-input.bin",
            "client-options/particles-state",
        ),
    ] {
        let input = String::from_utf8(frozen::bytes(input)).unwrap();
        frozen::assert_stream(recording, replay(&input).as_bytes());
    }
}
