use crate::{
    cache::Pack,
    font_layout::{self, Draw, Image, Images, Paragraph, Style},
    font_metrics::Metrics,
};
use std::io::Write;
fn int(w: &mut impl Write, v: i32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
fn string(w: &mut impl Write, s: &[u16]) -> std::io::Result<()> {
    int(w, s.len() as i32)?;
    for c in s {
        w.write_all(&c.to_be_bytes())?;
    }
    Ok(())
}

struct Provider<'a> {
    images: &'a Images,
    draws: std::rc::Rc<std::cell::RefCell<Vec<Draw>>>,
    trace: std::rc::Rc<std::cell::RefCell<Vec<[i32; 3]>>>,
    fail: bool,
}
impl font_layout::IconProvider for Provider<'_> {
    fn width(&self, id: i32) -> anyhow::Result<i32> {
        self.trace
            .borrow_mut()
            .push([self.draws.borrow().len() as i32, 0, id]);
        anyhow::ensure!(!self.fail || id != 40, "icon width failure");
        font_layout::IconProvider::width(self.images, id)
    }
    fn frames(&self, id: i32) -> anyhow::Result<Option<std::rc::Rc<Vec<Image>>>> {
        self.trace
            .borrow_mut()
            .push([self.draws.borrow().len() as i32, 1, id]);
        anyhow::ensure!(!self.fail || id != 40, "icon draw failure");
        font_layout::IconProvider::frames(self.images, id)
    }
}
struct Sink {
    draws: std::rc::Rc<std::cell::RefCell<Vec<Draw>>>,
    attempt: i32,
    fail: bool,
}
impl font_layout::Sink for Sink {
    fn emit(&mut self, d: Draw) -> anyhow::Result<()> {
        self.attempt += 1;
        anyhow::ensure!(!self.fail || self.attempt != 3, "draw failure");
        self.draws.borrow_mut().push(d);
        Ok(())
    }
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn layout() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("font-layout");
    let out = scratch.dir().to_path_buf();
    let pack = Pack::open(root.join("server/data/pack"));
    let mut fonts = Vec::new();
    for id in pack.read_archive_index("fontmetrics")?.group_id {
        let bytes = crate::js5_fetch::fetch_file(&pack, "fontmetrics", id)?.unwrap();
        fonts.push(bytes);
    }
    // Add a valid kerned record with zero profiles and deliberate pair gaps.
    let mut kern = vec![0, 1];
    kern.extend([10; 256]);
    kern.extend([2; 256]);
    kern.extend([1; 256]);
    kern.extend([1, 0, 1, 0]);
    kern.extend(vec![0; 1024]);
    for _ in 0..2 {
        for _ in 0..256 {
            kern.extend([2, 1]);
        }
    }
    kern.extend([0, 0, 12, 3, 1]);
    fonts.push(kern);
    let texts: Vec<Vec<u16>> = [
        "",
        "Hello world",
        "One two three four five",
        "No-space-long-word",
        "A  B   C ",
        "<col=cc2211>red</col> A",
        "<argb=7f224466>A</argb>B<argb=ff112233>C",
        "<argb=-1000000><u>U</u><str=ff00>strike</str>",
        "<shad=cc0000>A</shad>B<SHAD=-1>C<shad>D",
        "<col=-1>A</col><argb=badtag>B",
        "A<br>B<br><br>C",
        "<lt><gt><nbsp><shy><times><euro><copy><reg>",
        "<col<lt>>A<bad>V",
        "A<unfinished",
        "A<img=0>V<img=1>Z",
        "A<img=-1>V<img=999>W",
        "A<img=+1>V<img=no>W",
        "A<sprite=40>V<sprite=40,1>X",
        "A<sprite=41>V<sprite=40,9>X",
        "A<sprite=40,bad>V",
        "éÿ € 😀 中",
        "A\0B",
        "<u><str><col=22>AV AV AV AV AV AV AV AV</col></str></u>",
    ]
    .iter()
    .map(|s| s.encode_utf16().collect())
    .collect();
    let mut cases = Vec::new();
    for f in 0..fonts.len() {
        for t in 0..texts.len() {
            for op in 0..6 {
                cases.push((f, t, op, ((f + t + op) * 7 % 32) as i32));
            }
        }
    }
    // Cross every horizontal/vertical alignment and max-line policy.
    for h in 0..4 {
        for v in 0..4 {
            for max in [-1, 0, 1, 2, 5] {
                cases.push((0, 22, 1, (h << 16) | (v << 20) | ((max + 1) << 24) | 32));
            }
        }
    }
    for f in 0..fonts.len() {
        for t in 0..texts.len() {
            for op in 6..10 {
                for flags in [0, 1, 2, 7, 15, 71, 135, 263, 519] {
                    cases.push((f, t, op, flags));
                }
            }
        }
    }
    let mut input = std::io::BufWriter::new(std::fs::File::create(out.join("input.bin"))?);
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    int(&mut input, fonts.len() as i32)?;
    for raw in &fonts {
        int(&mut input, raw.len() as i32)?;
        input.write_all(raw)?;
    }
    int(&mut input, cases.len() as i32)?;
    let mut errors = 0;
    let mut draw_count = 0;
    for (n, (f, t, op, flags)) in cases.iter().copied().enumerate() {
        let p = [
            13,
            -7,
            [1, 17, 45, 99, 250][n % 5],
            [5, 40, 150][n % 3],
            if n % 2 == 0 { -1 } else { 0x7f225577 },
            if n % 3 == 0 { -1 } else { 0xff001133u32 as i32 },
            if flags & 32 != 0 {
                (flags >> 16) & 3
            } else {
                (n % 4) as i32
            },
            if flags & 32 != 0 {
                (flags >> 20) & 3
            } else {
                (n / 4 % 4) as i32
            },
            if n % 2 == 0 { 0 } else { 17 },
            if flags & 32 != 0 {
                (flags >> 24) - 1
            } else {
                [-1, 0, 1, 3][n % 4]
            },
            [1, 2, 100][n % 3],
        ];
        for v in [f as i32, op as i32, flags] {
            int(&mut input, v)?;
        }
        for v in p {
            int(&mut input, v)?;
        }
        string(&mut input, &texts[t])?;
        let m = Metrics::decode(&fonts[f])?;
        let mut images = Images::default();
        if flags & 1 != 0 {
            images.images = Some(vec![
                Image {
                    width: 7,
                    height: 9,
                },
                Image {
                    width: 12,
                    height: 15,
                },
            ]);
        }
        if flags & 2 != 0 {
            images.icons = Some(
                [(
                    40,
                    vec![
                        Image {
                            width: 11,
                            height: 40,
                        },
                        Image {
                            width: 19,
                            height: 5,
                        },
                    ],
                )]
                .into(),
            );
        }
        if flags & 4 != 0 {
            images.baselines = Some(vec![3, 18]);
        }
        let mut style = Style::new(p[4], p[5]);
        let mut draws = Vec::new();
        let mut lines = Vec::new();
        let mut value = 0;
        let shared = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut random = font_layout::Random::default();
        random.set_seed(910);
        let mut bounds = vec![71; if flags & 256 != 0 { 2 } else { 4 }];
        let mut attempts = 0;
        let mut run = || -> anyhow::Result<()> {
            match op {
                0 => {
                    style.justify(&m, &texts[t], p[2], &images)?;
                    font_layout::line(
                        &m,
                        font_layout::LineAt {
                            text: &texts[t],
                            x: p[0],
                            baseline: p[1],
                        },
                        &mut style,
                        &images,
                        flags & 8 != 0,
                        &mut draws,
                    )?;
                }
                1 => {
                    value = font_layout::paragraph(
                        &m,
                        &texts[t],
                        Paragraph {
                            x: p[0],
                            y: p[1],
                            width: p[2],
                            height: p[3],
                            colour: p[4],
                            shadow: p[5],
                            halign: p[6],
                            valign: p[7],
                            line_height: p[8],
                            max_lines: p[9],
                            masked: flags & 8 != 0,
                        },
                        &images,
                        &mut style,
                        &mut draws,
                    )?;
                }
                2 => {
                    let (s, c) = font_layout::split(
                        &m,
                        &texts[t],
                        Some(&[p[2], p[3]]),
                        &images,
                        flags & 16 == 0,
                        p[10] as usize,
                    )?;
                    lines = s;
                    value = c as i32;
                }
                3 => {
                    lines.push(font_layout::truncate(&m, &texts[t], p[2], &images)?);
                }
                4 | 5 => {
                    images.images = None;
                    images.baselines = None;
                    let width = images.width(&m, &texts[t])?;
                    font_layout::line(
                        &m,
                        font_layout::LineAt {
                            text: &texts[t],
                            x: p[0] - if op == 4 { width } else { width / 2 },
                            baseline: p[1],
                        },
                        &mut style,
                        &images,
                        false,
                        &mut draws,
                    )?;
                }
                6..=9 => {
                    let provider = Provider {
                        images: &images,
                        draws: shared.clone(),
                        trace: trace.clone(),
                        fail: flags & 64 != 0,
                    };
                    let provider =
                        (flags & 2 != 0).then_some(&provider as &dyn font_layout::IconProvider);
                    let mut sink = Sink {
                        draws: shared.clone(),
                        attempt: 0,
                        fail: flags & 512 != 0,
                    };
                    let para = Paragraph {
                        x: p[0],
                        y: p[1],
                        width: p[2],
                        height: p[3],
                        colour: p[4],
                        shadow: p[5],
                        halign: p[6],
                        valign: p[7],
                        line_height: p[8],
                        max_lines: p[9],
                        masked: flags & 8 != 0,
                    };
                    let text = (flags & 128 == 0).then_some(texts[t].as_slice());
                    let seed = [i32::MIN, -1, 0, 910, i32::MAX][n % 5];
                    let r = (|| -> anyhow::Result<()> {
                        if op == 6 {
                            value = font_layout::antimacro_to(
                                &m,
                                text,
                                para,
                                font_layout::TextResources {
                                    images: &images,
                                    provider,
                                },
                                &mut style,
                                font_layout::Antimacro {
                                    random: &mut random,
                                    seed,
                                    bounds: Some(&mut bounds),
                                },
                                &mut sink,
                            )?;
                        } else if op == 7 {
                            let mut dx: Vec<_> =
                                (0..texts[t].len()).map(|i| i as i32 % 7 - 3).collect();
                            let mut dy: Vec<_> =
                                (0..texts[t].len()).map(|i| i as i32 % 5 - 2).collect();
                            if flags & 256 != 0 {
                                dx.truncate(1);
                                dy.truncate(1);
                            }
                            font_layout::alpha_line_to(
                                &m,
                                font_layout::LineAt {
                                    text: text.ok_or_else(|| anyhow::anyhow!("null text"))?,
                                    x: p[0],
                                    baseline: p[1],
                                },
                                &mut style,
                                font_layout::TextResources {
                                    images: &images,
                                    provider,
                                },
                                font_layout::Offsets {
                                    x: Some(&dx),
                                    y: Some(&dy),
                                },
                                &mut sink,
                            )?;
                        } else if op == 8 {
                            value = font_layout::paragraph_to(
                                &m, text, para, &images, provider, &mut style, &mut sink,
                            )?;
                        } else {
                            font_layout::line_to(
                                &m,
                                font_layout::LineAt {
                                    text: text.ok_or_else(|| anyhow::anyhow!("null text"))?,
                                    x: p[0],
                                    baseline: p[1],
                                },
                                &mut style,
                                font_layout::TextResources {
                                    images: &images,
                                    provider,
                                },
                                flags & 8 != 0,
                                &mut sink,
                            )?;
                        }
                        Ok(())
                    })();
                    attempts = sink.attempt;
                    r?;
                }
                _ => unreachable!(),
            }
            Ok(())
        };
        let failed = run().is_err();
        if op >= 6 {
            draws = std::mem::take(&mut *shared.borrow_mut());
        }
        errors += failed as usize;
        draw_count += draws.len();
        result.write_all(&[failed as u8])?;
        int(&mut result, value)?;
        int(&mut result, lines.len() as i32)?;
        for line in lines {
            string(&mut result, &line)?;
        }
        for v in [
            style.strike,
            style.underline,
            style.original,
            style.colour,
            style.original_shadow,
            style.shadow,
            style.spacing,
            style.remainder,
        ] {
            int(&mut result, v)?;
        }
        int(&mut result, random.next_int())?;
        int(&mut result, bounds.len() as i32)?;
        for v in &bounds {
            int(&mut result, *v)?;
        }
        int(&mut result, attempts)?;
        int(&mut result, trace.borrow().len() as i32)?;
        for row in &*trace.borrow() {
            for v in row {
                int(&mut result, *v)?;
            }
        }
        int(&mut result, draws.len() as i32)?;
        for d in draws {
            let row = match d {
                Draw::Glyph {
                    code,
                    x,
                    y,
                    colour,
                    shadow,
                    masked,
                } => [
                    0,
                    code as i32,
                    x,
                    y,
                    colour,
                    shadow as i32,
                    masked as i32,
                    0,
                ],
                Draw::Line {
                    x,
                    y,
                    width,
                    colour,
                } => [1, x, y, width, colour, 0, 0, 0],
                Draw::Image {
                    icon,
                    id,
                    frame,
                    x,
                    y,
                    mode,
                    colour,
                } => [if icon { 3 } else { 2 }, id, frame, x, y, mode, colour, 0],
            };
            for v in row {
                int(&mut result, v)?;
            }
        }
    }
    input.flush()?;
    result.flush()?;
    std::fs::write(
        out.join("counts.json"),
        format!(
            "{{\"fonts\":{},\"cases\":{},\"draws\":{draw_count},\"errors\":{errors}}}\n",
            fonts.len(),
            cases.len()
        ),
    )?;
    println!(
        "Rust text layout: {} cases, {draw_count} draws, {errors} errors",
        cases.len()
    );
    scratch.finish("font-layout", &[("rust.bin", "recording")]);
    Ok(())
}
