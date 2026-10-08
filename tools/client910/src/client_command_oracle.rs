use std::io::Write;
#[test]
fn command_packets() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("client-commands");
    let out = scratch.dir().to_path_buf();
    std::fs::create_dir_all(&out)?;
    let mut input = std::fs::File::create(out.join("input.bin"))?;
    let mut result = std::fs::File::create(out.join("rust.bin"))?;
    let mut commands = vec![
        "".into(),
        "tele 3212 3428 0".into(),
        "tele 2965 3380 3".into(),
        "logout".into(),
        "js5_reload".into(),
        "reboottimer".into(),
        "Test €‚ƒ„…†‡ˆ‰Š‹ŒŽ‘’“”•–—˜™š›œžŸ".into(),
        "áéíóú ÿ 中 😀".into(),
        "nul\0test".into(),
    ];
    for n in [1, 127, 128, 200, 251, 252, 253, 254, 255, 256, 300] {
        commands.push("x".repeat(n));
    }
    for c in 0..256u32 {
        commands.push(format!("char {}", char::from_u32(c).unwrap()));
    }
    let mut commands: Vec<Vec<u16>> = commands
        .into_iter()
        .map(|s: String| s.encode_utf16().collect())
        .collect();
    commands.extend([
        vec![0xd800],
        vec![0xdc00],
        vec![65, 0xd800, 66],
        vec![0xdc00, 0xd800],
        vec![0xd800; 255],
        vec![0xdc00; 256],
    ]);
    let count = commands.len() * 4;
    input.write_all(&(count as i32).to_be_bytes())?;
    for command in commands {
        for scripted in [false, true] {
            for suggest in [false, true] {
                let units = &command;
                input.write_all(&(units.len() as i32).to_be_bytes())?;
                for unit in units {
                    input.write_all(&unit.to_be_bytes())?;
                }
                input.write_all(&[scripted as u8, suggest as u8])?;
                match crate::client_command::remote_units(&command, scripted, suggest) {
                    Ok(bytes) => {
                        result.write_all(&[0])?;
                        result.write_all(&(bytes.len() as i32).to_be_bytes())?;
                        result.write_all(&bytes)?;
                    }
                    Err(_) => result.write_all(&[1])?,
                }
            }
        }
    }
    println!("Rust command packets: {count}");
    scratch.finish("client-commands", &[("rust.bin", "recording")]);
    Ok(())
}
