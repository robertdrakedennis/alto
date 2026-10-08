//! `scenediff <recorded.bin> <rust.bin>` — compare two scene dumps
//! (the recorded scene dump / `scene::Scene::to_dump`) and
//! print the first differences per section. Exit code 1 on any difference.
//!
//! Layout (big-endian): `"SCN1", i32 levels, i32 maxX, i32 maxZ, u8[tiles],
//! i32 count, records`. Record: `u8 layer, u8 plane, u16 x, u16 z, i32[12],
//! u8 flags, u8 hasModel, [u8 transparent, u8 animatedUvs, i32 ambient, i32
//! contrast, i32 verts, i32 unique, i32 faces, i32 draw, f32[3u] pos,
//! i32[u] colours, f32[3u] normals, f32[2u] uvs, u16[3d] indices, i32 nb,
//! i32[5 nb] batches]`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;

#[derive(Clone, Debug, PartialEq)]
struct Model {
    transparent: u8,
    animated: u8,
    ambient: i32,
    contrast: i32,
    verts: i32,
    unique: i32,
    faces: i32,
    draw: i32,
    pos: Vec<u32>,
    colours: Vec<i32>,
    normals: Vec<u32>,
    /// `(nx, ny, nz, count)` raw shorts + count byte per unique vertex.
    raw_normals: Vec<(i16, i16, i16, i8)>,
    uvs: Vec<u32>,
    indices: Vec<u16>,
    batches: Vec<[i32; 5]>,
}

#[derive(Clone, Debug, PartialEq)]
struct Record {
    layer: u8,
    plane: u8,
    x: u16,
    z: u16,
    ints: [i32; 12],
    /// rot xyzw + scale xyz of the entity transform (bit patterns).
    srt: [u32; 7],
    flags: u8,
    model: Option<Model>,
}

struct Dump {
    levels: i32,
    max_x: i32,
    max_z: i32,
    tiles: Vec<u8>,
    records: Vec<Record>,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let b = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or_else(|| format!("truncated at {}", self.pos))?;
        self.pos += n;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }
}

fn parse(data: &[u8]) -> Result<Dump, String> {
    let mut c = Cursor { data, pos: 0 };
    if c.bytes(4)? != b"SCN1" {
        return Err("bad magic".into());
    }
    let levels = c.i32()?;
    let max_x = c.i32()?;
    let max_z = c.i32()?;
    let tiles = c.bytes((levels * max_x * max_z) as usize)?.to_vec();
    let count = c.i32()? as usize;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let layer = c.u8()?;
        let plane = c.u8()?;
        let x = c.u16()?;
        let z = c.u16()?;
        let mut ints = [0; 12];
        for v in &mut ints {
            *v = c.i32()?;
        }
        let mut srt = [0; 7];
        for v in &mut srt {
            *v = c.u32()?;
        }
        let flags = c.u8()?;
        let has_model = c.u8()?;
        let model = if has_model == 1 {
            let transparent = c.u8()?;
            let animated = c.u8()?;
            let ambient = c.i32()?;
            let contrast = c.i32()?;
            let verts = c.i32()?;
            let unique = c.i32()?;
            let faces = c.i32()?;
            let draw = c.i32()?;
            let u = unique as usize;
            let mut pos = Vec::with_capacity(u * 3);
            for _ in 0..u * 3 {
                pos.push(c.u32()?);
            }
            let mut colours = Vec::with_capacity(u);
            for _ in 0..u {
                colours.push(c.i32()?);
            }
            let mut normals = Vec::with_capacity(u * 3);
            for _ in 0..u * 3 {
                normals.push(c.u32()?);
            }
            let mut raw_normals = Vec::with_capacity(u);
            for _ in 0..u {
                let nx = c.u16()? as i16;
                let ny = c.u16()? as i16;
                let nz = c.u16()? as i16;
                let nc = c.u8()? as i8;
                raw_normals.push((nx, ny, nz, nc));
            }
            let mut uvs = Vec::with_capacity(u * 2);
            for _ in 0..u * 2 {
                uvs.push(c.u32()?);
            }
            let mut indices = Vec::with_capacity(draw as usize * 3);
            for _ in 0..draw as usize * 3 {
                indices.push(c.u16()?);
            }
            let nb = c.i32()? as usize;
            let mut batches = Vec::with_capacity(nb);
            for _ in 0..nb {
                let mut b = [0; 5];
                for v in &mut b {
                    *v = c.i32()?;
                }
                batches.push(b);
            }
            Some(Model {
                transparent,
                animated,
                ambient,
                contrast,
                verts,
                unique,
                faces,
                draw,
                pos,
                colours,
                normals,
                raw_normals,
                uvs,
                indices,
                batches,
            })
        } else {
            None
        };
        records.push(Record {
            layer,
            plane,
            x,
            z,
            ints,
            srt,
            flags,
            model,
        });
    }
    if c.pos != data.len() {
        return Err(format!("{} trailing bytes", data.len() - c.pos));
    }
    Ok(Dump {
        levels,
        max_x,
        max_z,
        tiles,
        records,
    })
}

fn key(r: &Record) -> (u8, u16, u16, u8, i32) {
    (r.plane, r.x, r.z, r.layer, r.ints[0])
}

fn describe(r: &Record) -> String {
    format!(
        "plane {} tile {},{} layer {} loc {} shape {} angle {}",
        r.plane, r.x, r.z, r.layer, r.ints[0], r.ints[1], r.ints[2]
    )
}

fn diff_models(a: &Model, b: &Model, out: &mut String) -> usize {
    let mut d = 0;
    macro_rules! field {
        ($f:ident) => {
            if a.$f != b.$f {
                d += 1;
                let _ = writeln!(out, "    {}: A {:?} B {:?}", stringify!($f), a.$f, b.$f);
            }
        };
    }
    field!(transparent);
    field!(animated);
    field!(ambient);
    field!(contrast);
    field!(verts);
    field!(unique);
    field!(faces);
    field!(draw);
    // NaN payloads differ between JVM and Rust float ops; a NaN is a NaN.
    let same =
        |p: u32, q: u32| p == q || (f32::from_bits(p).is_nan() && f32::from_bits(q).is_nan());
    let mut vec_diff = |name: &str, x: &[u32], y: &[u32], per: usize| {
        let n = x.len().min(y.len());
        let bad: Vec<usize> = (0..n).filter(|&i| !same(x[i], y[i])).collect();
        if !bad.is_empty() || x.len() != y.len() {
            d += bad.len().max(1);
            let i = bad.first().copied().unwrap_or(0);
            let _ = writeln!(
                out,
                "    {name}: {} diffs (len A {} B {}), first at {} (vertex {}): A {} B {}",
                bad.len(),
                x.len(),
                y.len(),
                i,
                i / per,
                f32::from_bits(x.get(i).copied().unwrap_or(0)),
                f32::from_bits(y.get(i).copied().unwrap_or(0))
            );
        }
    };
    vec_diff("positions", &a.pos, &b.pos, 3);
    vec_diff("normals", &a.normals, &b.normals, 3);
    vec_diff("uvs", &a.uvs, &b.uvs, 2);
    {
        let n = a.raw_normals.len().min(b.raw_normals.len());
        let bad: Vec<usize> = (0..n)
            .filter(|&i| a.raw_normals[i] != b.raw_normals[i])
            .collect();
        if !bad.is_empty() {
            d += bad.len();
            for &i in bad.iter().take(4) {
                let _ = writeln!(
                    out,
                    "    raw normal vertex {i}: A {:?} B {:?}",
                    a.raw_normals[i], b.raw_normals[i]
                );
            }
        }
    }
    {
        let n = a.colours.len().min(b.colours.len());
        let bad: Vec<usize> = (0..n).filter(|&i| a.colours[i] != b.colours[i]).collect();
        if !bad.is_empty() || a.colours.len() != b.colours.len() {
            d += bad.len().max(1);
            let i = bad.first().copied().unwrap_or(0);
            let _ = writeln!(
                out,
                "    colours: {} diffs, first vertex {}: A {:#010x} B {:#010x}",
                bad.len(),
                i,
                a.colours.get(i).copied().unwrap_or(0),
                b.colours.get(i).copied().unwrap_or(0)
            );
        }
    }
    if a.indices != b.indices {
        d += 1;
        let i = (0..a.indices.len().min(b.indices.len()))
            .find(|&i| a.indices[i] != b.indices[i])
            .unwrap_or(0);
        let _ = writeln!(
            out,
            "    indices differ (len A {} B {}), first at {}: A {:?} B {:?}",
            a.indices.len(),
            b.indices.len(),
            i,
            a.indices.get(i),
            b.indices.get(i)
        );
    }
    if a.batches != b.batches {
        d += 1;
        let _ = writeln!(out, "    batches: A {:?} B {:?}", a.batches, b.batches);
    }
    d
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: scenediff <a.bin> <b.bin>");
        return ExitCode::from(2);
    }
    let load = |p: &str| -> Result<Dump, String> {
        let bytes = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
        parse(&bytes).map_err(|e| format!("{p}: {e}"))
    };
    let (a, b) = match (load(&args[1]), load(&args[2])) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut report = String::new();
    let mut diffs = 0_usize;
    let _ = writeln!(
        report,
        "A: levels {} {}x{} records {}\nB: levels {} {}x{} records {}",
        a.levels,
        a.max_x,
        a.max_z,
        a.records.len(),
        b.levels,
        b.max_x,
        b.max_z,
        b.records.len()
    );
    if a.levels != b.levels || a.max_x != b.max_x || a.max_z != b.max_z {
        diffs += 1;
        let _ = writeln!(report, "header differs");
    }
    let tile_diffs: Vec<usize> = (0..a.tiles.len().min(b.tiles.len()))
        .filter(|&i| a.tiles[i] != b.tiles[i])
        .collect();
    if !tile_diffs.is_empty() {
        diffs += tile_diffs.len();
        let per = (a.max_x * a.max_z) as usize;
        for &i in tile_diffs.iter().take(5) {
            let plane = i / per;
            let x = (i % per) / a.max_z as usize;
            let z = i % a.max_z as usize;
            let _ = writeln!(
                report,
                "tile plane {plane} {x},{z}: A {:#04x} B {:#04x}",
                a.tiles[i], b.tiles[i]
            );
        }
        let _ = writeln!(report, "{} tile diffs", tile_diffs.len());
    }
    // entity matching by (plane, x, z, layer, loc id); order matters too.
    let mut only_a: BTreeMap<(u8, u16, u16, u8, i32), usize> = BTreeMap::new();
    let mut by_key_b: BTreeMap<(u8, u16, u16, u8, i32), Vec<&Record>> = BTreeMap::new();
    for r in &b.records {
        by_key_b.entry(key(r)).or_default().push(r);
    }
    let mut model_diffs = 0;
    let mut shown = 0;
    let mut missing = 0;
    let mut int_diffs = 0;
    for r in &a.records {
        let Some(cands) = by_key_b.get_mut(&key(r)) else {
            missing += 1;
            if missing <= 10 {
                let _ = writeln!(report, "only in A: {}", describe(r));
            }
            *only_a.entry(key(r)).or_default() += 1;
            continue;
        };
        if cands.is_empty() {
            missing += 1;
            if missing <= 10 {
                let _ = writeln!(report, "only in A (extra copy): {}", describe(r));
            }
            continue;
        }
        let o = cands.remove(0);
        if r.ints != o.ints || r.flags != o.flags || r.srt != o.srt {
            int_diffs += 1;
            if int_diffs <= 10 {
                let f = |s: &[u32; 7]| s.iter().map(|b| f32::from_bits(*b)).collect::<Vec<f32>>();
                let _ = writeln!(
                    report,
                    "header diff {}: A {:?}/{:#x}/{:?} B {:?}/{:#x}/{:?}",
                    describe(r),
                    r.ints,
                    r.flags,
                    f(&r.srt),
                    o.ints,
                    o.flags,
                    f(&o.srt)
                );
            }
        }
        match (&r.model, &o.model) {
            (None, None) => {}
            (Some(ma), Some(mb)) => {
                let mut detail = String::new();
                let d = diff_models(ma, mb, &mut detail);
                if d > 0 {
                    model_diffs += 1;
                    if shown < 12 {
                        let _ = writeln!(report, "model diff {}:\n{detail}", describe(r));
                        shown += 1;
                    }
                }
            }
            (ma, mb) => {
                model_diffs += 1;
                if shown < 12 {
                    let _ = writeln!(
                        report,
                        "model presence {}: A {} B {}",
                        describe(r),
                        ma.is_some(),
                        mb.is_some()
                    );
                    shown += 1;
                }
            }
        }
    }
    let mut extra_b = 0;
    for (k, v) in &by_key_b {
        for r in v {
            extra_b += 1;
            if extra_b <= 10 {
                let _ = writeln!(report, "only in B: {} (key {:?})", describe(r), k);
            }
        }
    }
    diffs += missing + extra_b + int_diffs + model_diffs;
    let _ = writeln!(report, "missing in B {missing}, extra in B {extra_b}, header diffs {int_diffs}, model diffs {model_diffs}");
    print!("{report}");
    if diffs == 0 {
        println!("IDENTICAL");
        ExitCode::SUCCESS
    } else {
        println!("{diffs} differences");
        ExitCode::from(1)
    }
}
