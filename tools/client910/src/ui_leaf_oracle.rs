//! Type-4 content selection and default-sprite font-image construction.
use crate::{
    ui_component_fields::Fields,
    ui_leaf::{self, ObjectText, Services},
    ui_text_compare::Language,
};
use anyhow::Result;
use std::{io::Write, rc::Rc};
fn int(w: &mut impl Write, n: i32) -> Result<()> {
    w.write_all(&n.to_be_bytes())?;
    Ok(())
}
fn text(w: &mut impl Write, v: Option<&[u16]>) -> Result<()> {
    int(w, v.map_or(-1, |v| v.len() as i32))?;
    if let Some(v) = v {
        for c in v {
            w.write_all(&c.to_be_bytes())?;
        }
    }
    Ok(())
}
struct Objects {
    name: Option<Vec<u16>>,
    stackable: i32,
    fail: bool,
    reads: Vec<i32>,
}
impl Services for Objects {
    fn object_text(&mut self, id: i32) -> Result<ObjectText> {
        self.reads.push(id);
        anyhow::ensure!(!self.fail, "object lookup failure");
        Ok(ObjectText {
            name: self.name.clone(),
            stackable: self.stackable,
        })
    }
    fn object_sprite(&mut self, _: &Fields) -> Result<Option<Rc<crate::ui_sprites::Sprite>>> {
        unreachable!()
    }
    fn http_sprite(&mut self, _: i32) -> Result<Option<Rc<crate::ui_sprites::Sprite>>> {
        unreachable!()
    }
    fn skybox(&mut self, _: &mut crate::ui_paint::Painter, _: &Fields, _: [i32; 2]) -> Result<()> {
        unreachable!()
    }
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-leaf");
    let out = scratch.dir().to_path_buf();
    let mut input = std::io::BufWriter::new(std::fs::File::create(out.join("input.bin"))?);
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    let counts = [
        i32::MIN,
        -1_000_000,
        -1000,
        -2,
        -1,
        0,
        1,
        2,
        999,
        1000,
        9999,
        10000,
        99999,
        100000,
        999999,
        1000000,
        9999999,
        10000000,
        i32::MAX,
    ];
    let strings = [
        None,
        Some(vec![]),
        Some("Sword".encode_utf16().collect()),
        Some("<u>Item😀</u>\0".encode_utf16().collect()),
        Some(vec![0xd800, 0, 0xdc00]),
    ];
    let cases = counts.len() * 8 * 3 * 3 * 2;
    int(&mut input, cases as i32)?;
    let mut n = 0;
    for count in counts {
        for lang in -1..7 {
            for stackable in [0, 1, 2] {
                for id in [-1, 0, 40] {
                    for pressed in [false, true] {
                        let name = strings[n % strings.len()].clone();
                        let original = strings[(n / 5) % strings.len()].clone();
                        let fail = n % 13 == 0;
                        for v in [id, count, stackable, pressed as i32, lang, fail as i32] {
                            int(&mut input, v)?;
                        }
                        text(&mut input, original.as_deref())?;
                        text(&mut input, name.as_deref())?;
                        let mut services = Objects {
                            name,
                            stackable,
                            fail,
                            reads: vec![],
                        };
                        let f = Fields {
                            invobject: id,
                            invcount: count,
                            text: original,
                            ..Default::default()
                        };
                        let r = ui_leaf::text_content(
                            &f,
                            pressed,
                            Language::from_id(lang),
                            &mut services,
                        );
                        int(&mut result, r.is_ok() as i32)?;
                        if let Ok(v) = r {
                            text(&mut result, v.as_deref())?;
                        }
                        int(&mut result, services.reads.len() as i32)?;
                        for id in services.reads {
                            int(&mut result, id)?;
                        }
                        n += 1;
                    }
                }
            }
        }
    }
    let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
    let defaults = crate::protocol910::defaults::Graphics::decode(
        &crate::js5_fetch::fetch_file(&pack, "defaults", 3)?.unwrap(),
    )
    .map_err(|e| anyhow::anyhow!("defaults {e:?}"))?
    .value
    .scalars;
    let raw = crate::js5_fetch::fetch_file(&pack, "sprites", defaults.font_icons as u32)?.unwrap();
    int(&mut input, raw.len() as i32)?;
    input.write_all(&raw)?;
    let frames = crate::sprite_data::Data::decode(&raw)?.len();
    int(&mut input, 16)?;
    let mut pixels = 0;
    for variant in 0..16 {
        let samples: Vec<f64> = (0..frames * 3)
            .map(|i| match variant {
                0 => 0.,
                1 => 0.5,
                2 => 0.9999999999999999,
                _ => ((i * 47 + variant * 89) % 1001) as f64 / 1001.,
            })
            .collect();
        int(&mut input, samples.len() as i32)?;
        for v in &samples {
            input.write_all(&v.to_be_bytes())?;
        }
        let mut fonts = crate::ui_fonts::Fonts::from_pack(pack.clone(), Some(0))?;
        let mut at = 0;
        fonts.load_inline_sprites(&mut || {
            let v = samples[at];
            at += 1;
            v
        })?;
        anyhow::ensure!(at == samples.len(), "random sample consumption");
        let sprites = fonts.inline_sprites.as_ref().unwrap();
        int(&mut result, sprites.len() as i32)?;
        for (i, s) in sprites.iter().enumerate() {
            for n in s.size.into_iter().chain(s.padding) {
                int(&mut result, n)?;
            }
            int(&mut result, s.argb.len() as i32)?;
            for p in &s.argb {
                int(&mut result, *p)?;
            }
            pixels += s.argb.len();
            let dims = fonts.images.images.as_ref().unwrap()[i];
            anyhow::ensure!(
                s.full_size() == [dims.width, dims.height],
                "image dimensions share pixels"
            );
        }
        int(&mut result, at as i32)?;
    }
    input.flush()?;
    result.flush()?;
    std::fs::write(
        out.join("counts.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"content_cases":cases,"inline_runs":16,"inline_frames":frames*16,"inline_pixels":pixels}),
        )?,
    )?;
    input.flush()?;
    result.flush()?;
    scratch.finish("ui-leaf", &[("rust.bin", "recording")]);
    Ok(())
}
