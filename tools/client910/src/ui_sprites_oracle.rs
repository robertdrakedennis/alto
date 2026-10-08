use crate::{
    sprite_data::{Data, Pixels},
    ui_component_fields::Fields,
    ui_components_oracle::int,
    ui_sprites::{Factory, Mask, Resources, Source, Sprite},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
fn ints(out: &mut Vec<u8>, values: &[i32]) {
    int(out, values.len() as i32);
    for v in values {
        int(out, *v);
    }
}
fn bytes(out: &mut Vec<u8>, v: Option<&[u8]>) {
    int(out, v.map_or(-1, |v| v.len() as i32));
    if let Some(v) = v {
        out.extend(v);
    }
}
fn data(out: &mut Vec<u8>, v: &Data, pids: &mut Vec<Rc<RefCell<Vec<i32>>>>) -> anyhow::Result<()> {
    ints(
        out,
        &[
            v.width,
            v.height,
            v.padding[0],
            v.padding[1],
            v.padding[2],
            v.padding[3],
            v.translucent() as i32,
        ],
    );
    match &v.pixels {
        Pixels::Paletted {
            palette,
            indices,
            alpha,
        } => {
            int(out, 0);
            int(out, identity(palette, pids));
            ints(out, &palette.borrow());
            bytes(out, Some(indices));
            bytes(out, alpha.as_deref());
        }
        Pixels::Full { argb, .. } => {
            int(out, 1);
            ints(out, argb);
        }
    }
    for padded in [false, true] {
        let p = v.argb(padded);
        out.push(p.is_ok() as u8);
        if let Ok(p) = p {
            ints(out, &p);
        }
    }
    Ok(())
}
fn identity<T>(v: &Rc<T>, ids: &mut Vec<Rc<T>>) -> i32 {
    if let Some(i) = ids.iter().position(|x| Rc::ptr_eq(x, v)) {
        i as i32
    } else {
        ids.push(v.clone());
        ids.len() as i32 - 1
    }
}
fn sprite(out: &mut Vec<u8>, s: &Sprite) {
    ints(out, &s.size);
    ints(out, &s.padding);
}
fn mask(out: &mut Vec<u8>, m: &Mask) {
    ints(out, &m.size);
    int(out, m.graphic);
    ints(out, &m.starts);
    ints(out, &m.lengths);
    for y in -1..=m.size[1] {
        for x in [-1, 0, 1, m.size[0] - 1, m.size[0], m.size[0] + 1] {
            out.push(m.contains(x, y) as u8);
        }
    }
}
#[derive(Default)]
struct Memory {
    files: BTreeMap<i32, Vec<u8>>,
    trace: Vec<u8>,
    fail: i32,
    mask_mode: i32,
    sprite_fail: bool,
}
struct Input(Rc<RefCell<Memory>>);
impl Source for Input {
    fn file(&mut self, id: i32) -> anyhow::Result<Option<Vec<u8>>> {
        let mut m = self.0.borrow_mut();
        int(&mut m.trace, 0);
        int(&mut m.trace, id);
        anyhow::ensure!(id != m.fail, "source failure");
        Ok(m.files.get(&id).cloned())
    }
}
struct Capture(Rc<RefCell<Memory>>);
impl Factory for Capture {
    fn sprite(&mut self, d: &Data) -> anyhow::Result<Rc<Sprite>> {
        let s = Sprite::new(d)?;
        let mut m = self.0.borrow_mut();
        int(&mut m.trace, 1);
        ints(&mut m.trace, &s.size);
        ints(&mut m.trace, &s.argb);
        anyhow::ensure!(!m.sprite_fail, "sprite allocation failure");
        Ok(Rc::new(s))
    }
    fn mask(&mut self, v: Mask) -> anyhow::Result<Option<Rc<Mask>>> {
        let mut m = self.0.borrow_mut();
        int(&mut m.trace, 2);
        ints(&mut m.trace, &v.size);
        ints(&mut m.trace, &v.starts);
        ints(&mut m.trace, &v.lengths);
        anyhow::ensure!(m.mask_mode != 2, "mask allocation failure");
        Ok((m.mask_mode == 0).then(|| Rc::new(v)))
    }
}
fn palette(size: usize, alpha: i32, w: u16, h: u16, frames: usize) -> Vec<u8> {
    let mut b = vec![];
    for frame in 0..frames {
        b.push(if alpha < 0 { 0 } else { 2 });
        for y in 0..h {
            for x in 0..w {
                b.push(
                    if x == 0 || y == 0 || (x as usize + y as usize + frame).is_multiple_of(5) {
                        0
                    } else {
                        ((x as usize * 3 + y as usize) % (size - 1) + 1) as u8
                    },
                );
            }
        }
        if alpha >= 0 {
            for i in 0..w as usize * h as usize {
                b.push(if alpha == 1 {
                    255
                } else {
                    [0, 1, 127, 128, 254, 255][i % 6]
                });
            }
        }
    }
    for i in 1..size {
        let c = if i % 5 == 0 {
            0
        } else {
            ((i * 23517) as u32) & 0xffffff
        };
        b.extend([(c >> 16) as u8, (c >> 8) as u8, c as u8]);
    }
    b.extend((w + 7).to_be_bytes());
    b.extend((h + 9).to_be_bytes());
    b.push((size - 1) as u8);
    for axis in 0..4 {
        for _ in 0..frames {
            b.extend(
                match axis {
                    0 => 2u16,
                    1 => 3,
                    2 => w,
                    _ => h,
                }
                .to_be_bytes(),
            );
        }
    }
    b.extend((frames as u16).to_be_bytes());
    b
}
fn full(alpha: u8) -> Vec<u8> {
    let mut b = vec![0, alpha, 0, 4, 0, 3];
    for p in [
        0xff00ffu32,
        0,
        1,
        0xffffff,
        0xff00ff,
        0x123456,
        0xff0000,
        0x00ff00,
        0x0000ff,
        0x444444,
        0xff00ff,
        0x123,
    ] {
        b.extend([(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    if alpha == 1 {
        b.extend([0, 1, 127, 128, 254, 255, 1, 0, 255, 128, 254, 255]);
    }
    b.extend([128, 1]);
    b
}
fn props(out: &mut Vec<u8>, f: &Fields) {
    for v in [
        f.parentlayer,
        f.id,
        f.graphic,
        f.outline,
        f.graphicshadow,
        f.alpha as i32,
        f.vflip as i32,
        f.hflip as i32,
    ] {
        int(out, v);
    }
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-sprites");
    let out = scratch.dir().to_path_buf();
    let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
    // The sprite components a recorded login exchange creates: parent, child,
    // graphic, outline, shadow, alpha, vflip, hflip (tab separated).
    let mut requests = vec![];
    let mut raw = BTreeMap::new();
    for line in rs910_core::test_support::frozen::text("ui-sprites/requests.tsv").lines() {
        let n: Vec<i32> = line.split('\t').map(|v| v.parse().unwrap()).collect();
        requests.push(Fields {
            parentlayer: n[0],
            id: n[1],
            graphic: n[2],
            outline: n[3],
            graphicshadow: n[4],
            alpha: n[5] != 0,
            vflip: n[6] != 0,
            hflip: n[7] != 0,
            ..Default::default()
        });
    }
    for f in &requests {
        if let std::collections::btree_map::Entry::Vacant(slot) = raw.entry(f.graphic) {
            if let Some(b) = crate::js5_fetch::fetch_file(&pack, "sprites", f.graphic as u32)? {
                slot.insert(b);
            }
        }
    }
    let cache_count = raw.len();
    let mut synthetic = vec![];
    let mut next = -100;
    for colours in [2, 3, 254, 255, 256] {
        for alpha in [-1, 0, 1] {
            raw.insert(next, palette(colours, alpha, 5, 4, 2));
            synthetic.push(next);
            next -= 1;
        }
    }
    for alpha in [0, 1, 2, 255] {
        raw.insert(next, full(alpha));
        synthetic.push(next);
        next -= 1;
    }
    for (w, h) in [(0, 0), (0, 3), (3, 0)] {
        raw.insert(next, palette(2, 0, w, h, 1));
        synthetic.push(next);
        next -= 1;
    }
    for b in [vec![], vec![0], vec![0; 7], vec![2, 0, 0, 1, 0, 1, 128, 0]] {
        raw.insert(next, b);
        synthetic.push(next);
        next -= 1;
    }
    let mut input = vec![];
    let mut output = vec![];
    let mut offsets = String::new();
    int(&mut input, raw.len() as i32);
    for (id, b) in &raw {
        int(&mut input, *id);
        bytes(&mut input, Some(b));
    }
    // Raw decoder and transform states; shared palette identities are retained.
    int(&mut input, raw.len() as i32);
    for (id, b) in &raw {
        int(&mut input, *id);
        let v = Data::decode(b);
        output.push(v.is_ok() as u8);
        let mut decoded = v.ok();
        let mut pids = vec![];
        if let Some(v) = &decoded {
            int(&mut output, v.len() as i32);
            for d in v {
                data(&mut output, d, &mut pids)?;
            }
        }
        let actions = if synthetic.contains(id) {
            vec![
                [6, 0, 14, -5, 12],
                [3, 0, 1, 0, 0],
                [6, 1, -4, 18, 2],
                [2, 0, 2, 0, 0],
                [0, 0, 0, 0, 0],
                [1, 0, 0, 0, 0],
                [3, 0, 0xffffff, 0, 0],
                [4, 0, 0xff123456u32 as i32, 0, 0],
                [5, 0, 0, 0, 0],
                [7, 0, 0, 0, 0],
                [2, 0, -1, 0, 0],
            ]
        } else {
            vec![]
        };
        int(&mut input, actions.len() as i32);
        for a in actions {
            for v in a {
                int(&mut input, v);
            }
            offsets.push_str(&format!("{} raw {id} {a:?}\n", output.len()));
            let result = (|| -> anyhow::Result<()> {
                let d = decoded
                    .as_mut()
                    .and_then(|v| v.get_mut(a[1] as usize))
                    .ok_or_else(|| anyhow::anyhow!("missing sprite"))?;
                match a[0] {
                    0 => d.flip(false),
                    1 => d.flip(true),
                    2 => d.expand(a[2])?,
                    3 => d.outline(a[2])?,
                    4 => d.shadow(a[2])?,
                    5 => d.rotate(),
                    6 => d.recolour([a[2], a[3], a[4]]),
                    7 => d.clear_padding(),
                    _ => unreachable!(),
                }
                Ok(())
            })();
            output.push(result.is_ok() as u8);
            if let Some(v) = &decoded {
                int(&mut output, v.len() as i32);
                for d in v {
                    data(&mut output, d, &mut pids)?;
                }
            } else {
                int(&mut output, -1);
            }
        }
    }
    let memory = Rc::new(RefCell::new(Memory {
        files: raw.clone(),
        fail: i32::MIN,
        ..Default::default()
    }));
    let mut state = crate::ui_properties::State {
        sprites: Some(Resources::new(Box::new(Input(memory.clone())))),
        ..Default::default()
    };
    let mut factory = Capture(memory.clone());
    let mut sids = vec![];
    let mut mids = vec![];
    let mut actions = vec![];
    for f in requests.iter() {
        actions.push((0, f.clone(), 0));
        actions.push((1, f.clone(), 0));
    }
    for id in &synthetic {
        for flags in 0..16 {
            let f = Fields {
                parentlayer: 65536 + flags,
                id: -1,
                graphic: *id,
                outline: flags % 4,
                graphicshadow: if flags & 4 != 0 { 0x123456 } else { 0 },
                vflip: flags & 1 != 0,
                hflip: flags & 2 != 0,
                alpha: flags & 8 != 0,
                ..Default::default()
            };
            actions.push((0, f.clone(), 0));
            actions.push((1, f, 0));
        }
    }
    let mut f = Fields {
        graphic: synthetic[0],
        parentlayer: -1,
        id: -1,
        ..Default::default()
    };
    for mode in [1, 2, 0] {
        actions.extend([
            (2, f.clone(), 0),
            (7, f.clone(), mode),
            (1, f.clone(), 0),
            (1, f.clone(), 0),
        ]);
    }
    actions.extend([
        (2, f.clone(), 0),
        (8, f.clone(), 1),
        (0, f.clone(), 0),
        (8, f.clone(), 0),
        (0, f.clone(), 0),
        (3, f.clone(), 0),
        (5, f.clone(), 0),
        (0, f.clone(), 0),
        (4, f.clone(), 0),
        (6, f.clone(), f.graphic),
        (0, f.clone(), 0),
        (1, f.clone(), 0),
        (6, f.clone(), i32::MIN),
    ]);
    f.graphicshadow = 1;
    actions.push((0, f.clone(), 0));
    f.graphicshadow = 0;
    f.outline = 16;
    actions.push((0, f.clone(), 0));
    let mut missing = f.clone();
    missing.graphic = i32::MAX;
    missing.outline = 0;
    f.outline = 0;
    actions.extend([
        (0, missing.clone(), 0),
        (1, missing.clone(), 0),
        (1, f.clone(), 0),
        (0, f.clone(), 0),
        (9, f.clone(), 0),
        (0, f.clone(), 0),
        (2, f.clone(), 0),
        (0, f.clone(), 0),
        (1, f.clone(), 0),
        (10, f.clone(), 0),
        (0, f.clone(), 0),
        (1, f.clone(), 0),
    ]);
    int(&mut input, actions.len() as i32);
    for (n, (op, f, a)) in actions.iter().enumerate() {
        int(&mut input, *op);
        props(&mut input, f);
        int(&mut input, *a);
        let mut component = crate::ui_components::Component::default();
        component.f = f.clone();
        let c = Rc::new(RefCell::new(component));
        let mut spr = None;
        let mut msk = None;
        let result = (|| -> anyhow::Result<()> {
            match op {
                0 => spr = state.component_sprite(&c, &mut factory)?,
                1 => msk = state.component_graphic(&c, &mut factory)?,
                2 => state.sprites.as_mut().unwrap().reset(),
                3 => state.sprites.as_mut().unwrap().clean(*a),
                4 => state.sprites.as_mut().unwrap().clear_soft(),
                5 => state.sprites.as_mut().unwrap().clear_soft_referents(),
                6 => memory.borrow_mut().fail = *a,
                7 => memory.borrow_mut().mask_mode = *a,
                8 => memory.borrow_mut().sprite_fail = *a != 0,
                9 => {
                    memory.borrow_mut().files.remove(&f.graphic);
                }
                10 => {
                    memory
                        .borrow_mut()
                        .files
                        .insert(f.graphic, raw[&f.graphic].clone());
                }
                _ => unreachable!(),
            }
            Ok(())
        })();
        offsets.push_str(&format!(
            "{} resource {n} {op} {}\n",
            output.len(),
            f.graphic
        ));
        output.push(result.is_ok() as u8);
        output.push(state.resource_missing as u8);
        int(
            &mut output,
            spr.as_ref().map_or(-1, |v| identity(v, &mut sids)),
        );
        if let Some(v) = spr {
            sprite(&mut output, &v);
        }
        int(
            &mut output,
            msk.as_ref().map_or(-1, |v| identity(v, &mut mids)),
        );
        if let Some(v) = msk {
            mask(&mut output, &v);
        }
        for (available, entries) in state.sprites.as_ref().unwrap().snapshot() {
            int(&mut output, available);
            int(&mut output, entries.len() as i32);
            for (key, soft, age, present, weight) in entries {
                output.extend(key.to_be_bytes());
                output.push(soft as u8);
                output.extend(age.to_be_bytes());
                output.push(present as u8);
                int(&mut output, weight);
            }
        }
        bytes(
            &mut output,
            Some(&std::mem::take(&mut memory.borrow_mut().trace)),
        );
    }
    let weights: Vec<(i32, i64, i32)> = vec![
        (0, 0, 4_000_000),
        (0, 1, 2_000_000),
        (1, 0, 0),
        (0, 2, 4_000_000),
        (1, 1, 0),
        (1, 0, 0),
        (0, 2, 6_000_001),
        (1, 2, 0),
        (0, 3, 0),
        (0, 4, -1),
        (0, 5, 1),
        (2, 0, 0),
        (1, 3, 0),
        (3, 0, 0),
        (1, 3, 0),
        (2, 0, 0),
        (4, 0, 0),
        (1, 3, 0),
        (5, 0, 0),
        (0, i64::MIN, 6_000_000),
        (0, i64::MAX, 1),
        (0, 10, i32::MIN),
        (1, 10, 0),
        (5, 0, 0),
    ];
    int(&mut input, weights.len() as i32);
    let mut cache = crate::ui_cache::Cache::new(6_000_000);
    for (n, &(op, key, weight)) in weights.iter().enumerate() {
        int(&mut input, op);
        input.extend(key.to_be_bytes());
        int(&mut input, weight);
        let mut value = None;
        let result = match op {
            0 => cache.insert(key, Rc::new(n as i32), weight),
            1 => {
                value = cache.get(key);
                Ok(())
            }
            2 => {
                cache.clean(weight);
                Ok(())
            }
            3 => {
                cache.clear_soft();
                Ok(())
            }
            4 => {
                cache.clear_soft_referents();
                Ok(())
            }
            5 => {
                cache.reset();
                Ok(())
            }
            _ => unreachable!(),
        };
        offsets.push_str(&format!(
            "{} weight {n} {op} {key} {weight}\n",
            output.len()
        ));
        output.push(result.is_ok() as u8);
        int(&mut output, value.map_or(-1, |v| *v));
        int(&mut output, cache.available());
        let entries = cache.weighted_snapshot();
        int(&mut output, entries.len() as i32);
        for (k, soft, age, present, w) in entries {
            output.extend(k.to_be_bytes());
            output.push(soft as u8);
            output.extend(age.to_be_bytes());
            output.push(present as u8);
            int(&mut output, w);
        }
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(out.join("rust-offsets.txt"), offsets)?;
    std::fs::write(
        out.join("counts.txt"),
        format!(
            "{cache_count} {} {} {} {}\n",
            synthetic.len(),
            requests.len(),
            actions.len(),
            weights.len()
        ),
    )?;
    scratch.finish("ui-sprites", &[("rust.bin", "recording")]);
    Ok(())
}
