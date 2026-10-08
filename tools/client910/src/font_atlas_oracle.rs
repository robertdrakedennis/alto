use crate::{cache::Pack, font_atlas::Atlas, font_metrics::Metrics, sprite_sheet::SpriteSheet};
use std::io::Write;
fn int(w: &mut impl Write, v: i32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

fn sprite(kind: u8, mode: u8) -> Vec<u8> {
    let indices: Vec<u8> = (0..64).map(|i| i % 3).collect();
    let alpha: Vec<u8> = (0..64)
        .map(|i| if mode == 1 { 255 } else { i * 4 })
        .collect();
    if kind < 2 {
        let mut b = vec![kind | if mode == 0 { 0 } else { 2 }];
        for plane in [&indices, &alpha]
            .iter()
            .take(if mode == 0 { 1 } else { 2 })
        {
            for i in 0..64 {
                b.push(plane[if kind == 0 { i } else { (i % 8) * 8 + i / 8 }]);
            }
        }
        b.extend([0x27, 0x55, 0xbb, 0, 0, 0]);
        b.extend([0, 8, 0, 8, 2, 0, 0, 0, 0, 0, 8, 0, 8, 0, 1]);
        b
    } else {
        let mut b = vec![0, if mode == 0 { 0 } else { 1 }, 0, 8, 0, 8];
        for i in 0..64 {
            let rgb = if kind == 3 && i == 0 {
                [255, 0, 255]
            } else {
                [i * 3, i * 2, i]
            };
            b.extend(rgb);
        }
        if mode != 0 {
            b.extend(alpha);
        }
        b.extend([128, 1]);
        b
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn texture_pixels() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("font-atlas");
    let out = scratch.dir().to_path_buf();
    let pack = Pack::open(root.join("server/data/pack"));
    let mut cases = Vec::new();
    for id in pack.read_archive_index("fontmetrics")?.group_id {
        let metrics = crate::js5_fetch::fetch_file(&pack, "fontmetrics", id)?.unwrap();
        let m = Metrics::decode(&metrics)?;
        let raw = crate::js5_fetch::fetch_file(&pack, "sprites", id)?
            .ok_or_else(|| anyhow::anyhow!("missing font sprite {id}"))?;
        cases.push((m.atlas_width, m.atlas_height, raw));
    }
    let cached = cases.len();
    for kind in 0..4 {
        for mode in 0..3 {
            cases.push((8, 8, sprite(kind, mode)));
        }
    }
    // Reference loop limits differ between paletted, full mono and full colour.
    for kind in 0..4 {
        for size in [7, 9] {
            cases.push((size, size, sprite(kind, 2)));
        }
    }
    let mut input = std::fs::File::create(out.join("input.bin"))?;
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    int(&mut input, cases.len() as i32)?;
    let mut errors = 0;
    for (w, h, raw) in &cases {
        int(&mut input, *w as i32)?;
        int(&mut input, *h as i32)?;
        int(&mut input, raw.len() as i32)?;
        input.write_all(raw)?;
        let m = Metrics {
            atlas_width: *w,
            atlas_height: *h,
            ..Default::default()
        };
        let sheet = SpriteSheet::decode(raw)?;
        for mono in [false, true] {
            match Atlas::new(&m, &sheet, mono) {
                Ok(a) => {
                    result.write_all(&[0])?;
                    int(&mut result, a.argb.len() as i32)?;
                    for p in a.argb {
                        int(&mut result, p as i32)?;
                    }
                }
                Err(_) => {
                    result.write_all(&[1])?;
                    errors += 1;
                }
            }
        }
    }
    result.flush()?;
    std::fs::write(
        out.join("counts.json"),
        format!(
            "{{\"cached_fonts\":{cached},\"cases\":{},\"textures\":{},\"errors\":{errors}}}\n",
            cases.len(),
            cases.len() * 2
        ),
    )?;
    println!(
        "Font atlases: {cached} cached, {} textures, {errors} expected failures",
        cases.len() * 2
    );
    result.flush()?;
    scratch.finish("font-atlas", &[("rust.bin", "recording")]);
    Ok(())
}
