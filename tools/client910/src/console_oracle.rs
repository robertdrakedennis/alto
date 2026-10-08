use crate::protocol910::pack_defaults as defaults;
use crate::{
    cache::Pack,
    console::{self, Console, Draw, Font, Host, Key, Text},
    font_metrics::Metrics,
};
use std::io::Write;
struct H {
    now: i64,
    clipboard: Option<Text>,
    commands: Vec<(Text, bool)>,
}
impl Host for H {
    fn now(&mut self) -> i64 {
        self.now
    }
    fn command(&mut self, _: &mut Console, c: Text, s: bool) {
        self.commands.push((c, s));
    }
    fn paste(&mut self) -> Option<Text> {
        self.clipboard.clone()
    }
    fn copy(&mut self, t: Text) {
        self.clipboard = Some(t);
    }
}
fn int(w: &mut impl Write, v: i32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
fn string(w: &mut impl Write, t: &[u16]) -> std::io::Result<()> {
    int(w, t.len() as i32)?;
    for c in t {
        w.write_all(&c.to_be_bytes())?;
    }
    Ok(())
}
struct Step {
    op: i32,
    now: i64,
    words: Text,
    keys: Vec<Key>,
    wheel: i32,
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("console");
    let out = scratch.dir().to_path_buf();
    let pack = Pack::open(root.join("server/data/pack"));
    let d = defaults::load(&pack)?.graphics.scalars;
    let raw: Vec<_> = [d.p11_full, d.p12_full, d.b12_full]
        .into_iter()
        .map(|id| {
            crate::js5_fetch::fetch_file(&pack, "fontmetrics", id as u32)
                .unwrap()
                .unwrap()
        })
        .collect();
    let mut sprite_file = std::fs::File::create(out.join("sprites.bin"))?;
    for id in [d.p11_full, d.p12_full, d.b12_full] {
        let sprite = crate::js5_fetch::fetch_file(&pack, "sprites", id as u32)?.unwrap();
        int(&mut sprite_file, sprite.len() as i32)?;
        sprite_file.write_all(&sprite)?;
    }
    let fonts = raw
        .iter()
        .map(|r| Metrics::decode(r).unwrap())
        .collect::<Vec<_>>();
    let mut steps = Vec::new();
    let mut now = 1_700_000_000_000i64;
    let mut push = |op, s: &str, keys: Vec<Key>, wheel| {
        steps.push(Step {
            op,
            now,
            words: console::text(s),
            keys,
            wheel,
        });
        now += 20;
    };
    push(0, "", vec![], 0);
    for _ in 0..20 {
        push(2, "", vec![], 0);
    }
    for s in [
        "tele 3212 3428 0",
        "  tele 3222 3222 1  ",
        "directlogin alice secret",
        " ",
        "zero\0internal",
        "é€😀",
    ] {
        push(1, s, vec![], 0);
        push(
            2,
            "",
            vec![Key {
                code: 84,
                ch: 65535,
                modifiers: 0,
            }],
            0,
        );
    }
    for code in [104, 104, 105, 102, 97, 85, 101, 103, 80, 84, 105] {
        push(
            2,
            "",
            vec![Key {
                code,
                ch: 65535,
                modifiers: 0,
            }],
            0,
        );
    }
    push(1, "abc", vec![], 0);
    push(
        2,
        "",
        vec![Key {
            code: 102,
            ch: 65535,
            modifiers: 0,
        }],
        0,
    );
    push(4, " tail", vec![], 0);
    for ch in "AZaz09\\/.:, _-+[]~@!$€é\n\0".encode_utf16() {
        push(
            2,
            "",
            vec![Key {
                code: 0,
                ch,
                modifiers: 0,
            }],
            0,
        );
    }
    push(
        6,
        "first\nsecond\npause 1\nthird\npause -1\nfourth\n",
        vec![],
        0,
    );
    push(
        2,
        "",
        vec![Key {
            code: 67,
            ch: 65535,
            modifiers: 4,
        }],
        0,
    );
    for _ in 0..60 {
        push(2, "", vec![], 0);
    }
    push(3, "left\u{8}right\nrow two\n", vec![], 0);
    push(2, "", vec![], -3);
    push(
        3,
        &(0..520).map(|i| format!("log {i}\n")).collect::<String>(),
        vec![],
        0,
    );
    for wheel in [-1000, 1, 7, 1000, -1] {
        push(2, "", vec![], wheel);
    }
    push(
        2,
        "",
        vec![Key {
            code: 66,
            ch: 65535,
            modifiers: 4,
        }],
        0,
    );
    push(1, "directlogin user password", vec![], 0);
    push(0, "", vec![], 0);
    push(0, "", vec![], 0);
    for _ in 0..505 {
        push(
            2,
            "",
            vec![Key {
                code: 104,
                ch: 65535,
                modifiers: 0,
            }],
            0,
        );
    }
    for s in ["pause", "pause nonsense", "pause 0", "pause -2"] {
        push(4, &format!("a\n{s}\nb"), vec![], 0);
        push(
            2,
            "",
            vec![Key {
                code: 0,
                ch: 65,
                modifiers: 0,
            }],
            0,
        );
    }
    let mut input = std::io::BufWriter::new(std::fs::File::create(out.join("input.bin"))?);
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    for r in raw {
        int(&mut input, r.len() as i32)?;
        input.write_all(&r)?;
    }
    int(&mut input, steps.len() as i32)?;
    let mut c = Console::default();
    let mut host = H {
        now: 0,
        clipboard: None,
        commands: vec![],
    };
    let selected = [
        0, 1, 16, 17, 20, 21, 24, 25, 31, 36, 80, 137, 140, 143, 144, 146, 149, 150, 153, 650, 658,
    ];
    let mut frames = std::fs::File::create(out.join("frames.bin"))?;
    int(&mut frames, selected.len() as i32)?;
    for cycle in selected {
        int(&mut frames, cycle)?;
    }
    for (cycle, s) in steps.iter().enumerate() {
        host.now = s.now;
        host.commands.clear();
        int(&mut input, s.op)?;
        input.write_all(&s.now.to_be_bytes())?;
        string(&mut input, &s.words)?;
        int(&mut input, s.wheel)?;
        int(&mut input, s.keys.len() as i32)?;
        for k in &s.keys {
            for v in [k.code, k.ch as i32, k.modifiers] {
                int(&mut input, v)?;
            }
        }
        match s.op {
            0 => c.toggle(
                &mut host,
                [
                    fonts[1].ascent + fonts[1].descent + 2,
                    fonts[2].ascent + fonts[2].descent + 2,
                ],
            ),
            1 => c.set_entry(s.words.clone()),
            2 => {
                c.tick(&mut host, &s.keys, s.wheel);
            }
            3 => c.add(&mut host, &s.words),
            4 => c.paste_lines(
                &mut host,
                s.words.split(|v| *v == 10).map(|s| s.to_vec()).collect(),
            ),
            6 => host.clipboard = Some(s.words.clone()),
            _ => unreachable!(),
        }
        for v in [
            c.open as i32,
            c.opacity,
            c.cursor as i32,
            c.history as i32,
            c.count as i32,
            c.scroll as i32,
            c.row_height,
            c.entry_height,
            c.script_next,
        ] {
            int(&mut result, v)?;
        }
        result.write_all(&c.resume_at.to_be_bytes())?;
        string(&mut result, &c.entry)?;
        for row in c.lines.as_ref().unwrap() {
            string(&mut result, row)?;
        }
        int(
            &mut result,
            c.script.as_ref().map_or(-1, |v| v.len() as i32),
        )?;
        if let Some(lines) = &c.script {
            for row in lines {
                string(&mut result, row)?;
            }
        }
        int(&mut result, host.commands.len() as i32)?;
        for (t, s) in &host.commands {
            string(&mut result, t)?;
            int(&mut result, *s as i32)?;
        }
        int(&mut result, host.clipboard.is_some() as i32)?;
        if let Some(t) = &host.clipboard {
            string(&mut result, t)?;
        }
        let draw = c.draw(
            if cycle % 2 == 0 { 800 } else { 333 },
            cycle as i32,
            cycle % 3 != 0,
            [&fonts[0], &fonts[1], &fonts[2]],
        )?;
        int(&mut result, draw.len() as i32)?;
        for d in draw {
            match d {
                Draw::Clip(r) => {
                    int(&mut result, 0)?;
                    for v in r {
                        int(&mut result, v)?;
                    }
                }
                Draw::Fill {
                    x,
                    y,
                    width,
                    height,
                    colour,
                    blend,
                } => {
                    for v in [1, x, y, width, height, colour, blend] {
                        int(&mut result, v)?;
                    }
                }
                Draw::Text {
                    font,
                    text,
                    x,
                    y,
                    right,
                    colour,
                    shadow,
                } => {
                    for v in [
                        2,
                        match font {
                            Font::P11 => 0,
                            Font::P12 => 1,
                            Font::B12 => 2,
                        },
                        x,
                        y,
                        right as i32,
                        colour,
                        shadow,
                    ] {
                        int(&mut result, v)?;
                    }
                    string(&mut result, &text)?;
                }
                Draw::Line {
                    x,
                    y,
                    length,
                    vertical,
                    colour,
                } => {
                    for v in [3, x, y, length, vertical as i32, colour] {
                        int(&mut result, v)?;
                    }
                }
                Draw::ResetClip => int(&mut result, 4)?,
            }
        }
    }
    input.flush()?;
    result.flush()?;
    println!("Rust console steps: {}", steps.len());
    result.flush()?;
    scratch.finish("console", &[("rust.bin", "recording")]);
    Ok(())
}
