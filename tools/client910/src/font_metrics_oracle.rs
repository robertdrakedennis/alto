use crate::{cache::Pack, font_metrics::Metrics};
use std::io::Write;
fn int(w: &mut impl Write, v: i32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

fn synthetic(seed: u8, kern: bool, scale: u8) -> Vec<u8> {
    let mut b = vec![0, kern as u8];
    let advances: Vec<u8> = (0..256)
        .map(|i| (i as u8).wrapping_mul(13).wrapping_add(seed))
        .collect();
    let heights: Vec<u8> = (0..256)
        .map(|i| if kern { (i % 7) as u8 } else { i as u8 })
        .collect();
    b.extend(&advances);
    b.extend(&heights);
    b.extend((0..256).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)));
    b.extend(511u16.to_be_bytes());
    b.extend(257u16.to_be_bytes());
    for axis in 0..2 {
        for i in 0..256 {
            b.extend(((i * 197 + axis * 32768) as u16).to_be_bytes());
        }
    }
    if kern {
        for plane in 0..2 {
            for (i, &h) in heights.iter().enumerate() {
                for y in 0..h {
                    b.push(
                        (i as u8)
                            .wrapping_mul(29)
                            .wrapping_add(y)
                            .wrapping_add(plane * 53),
                    );
                }
            }
        }
    } else {
        b.push(201);
    }
    b.extend([231, 173, 243, 17, scale]);
    b
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn metrics_and_widths() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("font-metrics");
    let out = scratch.dir().to_path_buf();
    let pack = Pack::open(root.join("server/data/pack"));
    let mut cases = Vec::new();
    for id in pack.read_archive_index("fontmetrics")?.group_id {
        for (_, bytes) in pack.read_group("fontmetrics", id)? {
            cases.push(bytes);
        }
    }
    let cache_count = cases.len();
    for seed in [0, 127, 255] {
        for kern in [false, true] {
            for scale in [1, 2, 3, 255] {
                cases.push(synthetic(seed, kern, scale));
            }
        }
    }
    cases.push(synthetic(0, false, 0));
    let mut invalid = synthetic(0, true, 1);
    invalid[258] = 128;
    cases.push(invalid);
    let base = synthetic(0, true, 1);
    for n in [0, 1, 100, 769, 773, 1796, 1797, 1800, base.len() - 1] {
        cases.push(base[..n].to_vec());
    }
    let mut invalid = synthetic(0, false, 1);
    invalid[0] = 1;
    cases.push(invalid);
    let mut texts: Vec<Vec<u16>> = [
        "",
        "AVATAR To fi",
        "<col=ff0000>A</col>V",
        "<lt><gt><nbsp><shy><times><euro><copy><reg>",
        "A<br>V",
        "<",
        "abc<xyz",
        "<col<lt>>",
        "A<img=0>V<img=1>!",
        "A<img=+1>V<img=-1>x",
        "A<img=2147483648>V",
        "A<img= 0>V",
        "A<sprite=123,0>V",
        "€",
        "éÿ",
        "😀",
        "\0",
        "A中",
        "中A",
        "A€",
        "€A",
    ]
    .iter()
    .map(|s| s.encode_utf16().collect())
    .collect();
    for c in 0..256u16 {
        texts.push(vec![65, c, 86]);
    }
    for c in [256, 338, 8364, 0xd800, 0xdc00, 0xffff] {
        texts.push(vec![c]);
        texts.push(vec![65, c]);
        texts.push(vec![c, 86]);
    }
    let mut input = std::fs::File::create(out.join("input.bin"))?;
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    int(&mut input, texts.len() as i32)?;
    for t in &texts {
        int(&mut input, t.len() as i32)?;
        for c in t {
            input.write_all(&c.to_be_bytes())?;
        }
    }
    int(&mut input, cases.len() as i32)?;
    let (mut valid, mut rejected, mut width_errors) = (0, 0, 0);
    for data in &cases {
        int(&mut input, data.len() as i32)?;
        input.write_all(data)?;
        let m = match Metrics::decode(data) {
            Ok(m) => m,
            Err(_) => {
                result.write_all(&[1])?;
                rejected += 1;
                continue;
            }
        };
        valid += 1;
        result.write_all(&[0])?;
        for v in [
            m.atlas_width as i32,
            m.atlas_height as i32,
            m.scale as i32,
            m.space_width,
            m.ascent,
            m.descent,
            m.metric_a,
            m.metric_b,
        ] {
            int(&mut result, v)?;
        }
        for a in [&m.advances, &m.widths, &m.bearings] {
            result.write_all(a)?;
        }
        for r in &m.rects {
            for v in r {
                result.write_all(&v.to_be_bytes())?;
            }
        }
        result.write_all(&[m.kerning.is_some() as u8])?;
        if let Some(table) = &m.kerning {
            for row in table {
                for v in row {
                    result.write_all(&[*v as u8])?;
                }
            }
        }
        for i in 0..256 {
            for vertex in m.glyph_vertices(i) {
                for v in vertex {
                    int(&mut result, v.to_bits() as i32)?;
                }
            }
        }
        for t in &texts {
            for images in [None, Some(&[3, 7][..])] {
                match m.width_utf16(t, images) {
                    Ok(v) => {
                        result.write_all(&[0])?;
                        int(&mut result, v)?;
                    }
                    Err(_) => {
                        result.write_all(&[1])?;
                        width_errors += 1;
                    }
                }
            }
        }
    }
    result.flush()?;
    std::fs::write(out.join("counts.json"),format!("{{\"cached_fonts\":{cache_count},\"cases\":{},\"decoded\":{valid},\"rejected\":{rejected},\"strings\":{},\"width_queries\":{},\"width_errors\":{width_errors}}}\n",cases.len(),texts.len(),valid*texts.len()*2))?;
    println!(
        "Font metrics: {cache_count} cached, {valid} decoded, {rejected} rejected, {} widths",
        valid * texts.len() * 2
    );
    scratch.finish("font-metrics", &[("rust.bin", "recording")]);
    Ok(())
}
