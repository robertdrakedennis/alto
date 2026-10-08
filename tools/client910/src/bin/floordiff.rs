//! `floordiff <a.bin> <b.bin>` — compare two floor dumps (Rust
//! `FloorGeometry::to_dump` / the recorded floor dump) section
//! by section and print the first differences. Exit code 1 on any difference.
//!
//! Format (big-endian): `u32 vertex_count, u32 stride_floats, u32 flags,
//! f32[vertex_count * stride_floats] stream0, i32[vertex_count] base_colours,
//! u32 batch_count, per batch: i32 material, f32 scale, i32[7] fog, i64
//! node_id, i32[vertex_count] colours`, then (optional, absent in old
//! dumps) the hard-shadow mask: `u8 present, [i32 width, i32 height,
//! u8[width * height] mask]` (index
//! `width * z + x`), then (optional) the floor-sweep-only share of that
//! mask: `u8 present, [u8[width * height]]`.
//!
//! The full masks (floor sweep + loc entity shadows, both ported) are
//! compared strictly. The recorded sweep-only replay section is only reported:
//! the Rust dump has no sweep-only mask any more (it writes its full mask in
//! that slot), so that line is informational.

use std::fmt::Write as _;
use std::process::ExitCode;

struct Dump {
    vertex_count: usize,
    stride: usize,
    flags: u32,
    stream0: Vec<f32>,
    base: Vec<i32>,
    batches: Vec<Batch>,
    /// Hard-shadow mask `(width, height, bytes)`; `None` when absent or
    /// the file predates the section.
    mask: Option<(usize, usize, Vec<u8>)>,
    /// Floor-sweep-only share of `mask` (same shape); `None` when absent.
    sweep: Option<Vec<u8>>,
}

struct Batch {
    material: i32,
    scale: f32,
    fog: [i32; 7],
    node_id: i64,
    colours: Vec<i32>,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.data.get(self.pos..self.pos + 4).ok_or("truncated")?;
        self.pos += 4;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn i64(&mut self) -> Result<i64, String> {
        let hi = i64::from(self.u32()?);
        let lo = i64::from(self.u32()?);
        Ok((hi << 32) | lo)
    }
}

fn parse(data: &[u8]) -> Result<Dump, String> {
    let mut c = Cursor { data, pos: 0 };
    let vertex_count = c.u32()? as usize;
    let stride = c.u32()? as usize;
    let flags = c.u32()?;
    let mut stream0 = Vec::with_capacity(vertex_count * stride);
    for _ in 0..vertex_count * stride {
        stream0.push(c.f32()?);
    }
    let mut base = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        base.push(c.i32()?);
    }
    let batch_count = c.u32()? as usize;
    let mut batches = Vec::with_capacity(batch_count);
    for _ in 0..batch_count {
        let material = c.i32()?;
        let scale = c.f32()?;
        let mut fog = [0; 7];
        for f in &mut fog {
            *f = c.i32()?;
        }
        let node_id = c.i64()?;
        let mut colours = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            colours.push(c.i32()?);
        }
        batches.push(Batch {
            material,
            scale,
            fog,
            node_id,
            colours,
        });
    }
    let mut mask = None;
    let mut sweep = None;
    if c.pos != data.len() {
        let present = data[c.pos];
        c.pos += 1;
        if present != 0 {
            let width = c.i32()? as usize;
            let height = c.i32()? as usize;
            let bytes = data
                .get(c.pos..c.pos + width * height)
                .ok_or("truncated mask")?
                .to_vec();
            c.pos += width * height;
            mask = Some((width, height, bytes));
        }
        if c.pos != data.len() {
            let present = data[c.pos];
            c.pos += 1;
            if present != 0 {
                let (width, height, _) = mask.as_ref().ok_or("sweep mask without full mask")?;
                let n = width * height;
                sweep = Some(
                    data.get(c.pos..c.pos + n)
                        .ok_or("truncated sweep mask")?
                        .to_vec(),
                );
                c.pos += n;
            }
        }
    }
    if c.pos != data.len() {
        return Err(format!("{} trailing bytes", data.len() - c.pos));
    }
    Ok(Dump {
        vertex_count,
        stride,
        flags,
        stream0,
        base,
        batches,
        mask,
        sweep,
    })
}

/// Compare two optional `width x height` byte masks; returns the number of
/// differing texels (1 for a shape / presence mismatch) and appends one
/// line labelled `what` to `report`.
fn diff_bytes(
    what: &str,
    a: Option<(usize, usize, &[u8])>,
    b: Option<(usize, usize, &[u8])>,
    report: &mut String,
) -> usize {
    let set = |m: &[u8]| m.iter().filter(|&&v| v != 0).count();
    match (a, b) {
        (None, None) => {
            let _ = writeln!(report, "{what}: none in either");
            0
        }
        (Some((w, h, m)), None) => {
            let _ = writeln!(report, "{what}: only in A ({w}x{h}, {} set)", set(m));
            1
        }
        (None, Some((w, h, m))) => {
            let _ = writeln!(report, "{what}: only in B ({w}x{h}, {} set)", set(m));
            1
        }
        (Some((aw, ah, am)), Some((bw, bh, bm))) => {
            let (a_set, b_set) = (set(am), set(bm));
            if aw != bw || ah != bh {
                let _ = writeln!(
                    report,
                    "{what}: A {aw}x{ah} ({a_set} set) B {bw}x{bh} ({b_set} set): size differs"
                );
                return 1;
            }
            let diffs = am.iter().zip(bm).filter(|(x, y)| x != y).count();
            if diffs == 0 {
                let _ = writeln!(report, "{what}: IDENTICAL ({aw}x{ah}, {a_set} set)");
            } else {
                let i = am.iter().zip(bm).position(|(x, y)| x != y).unwrap_or(0);
                let _ = writeln!(
                    report,
                    "{what}: {aw}x{ah}, A {a_set} set B {b_set} set, {diffs} differing bytes, first at texel ({}, {}): A {} B {}",
                    i % aw,
                    i / aw,
                    am[i],
                    bm[i]
                );
            }
            diffs
        }
    }
}

/// Compare the hard-shadow masks: the full mask strictly; the sweep-only
/// replay is reported only (see module doc). Returns the counted differences.
fn diff_mask(a: &Dump, b: &Dump, report: &mut String) -> usize {
    fn full(d: &Dump) -> Option<(usize, usize, &[u8])> {
        d.mask.as_ref().map(|(w, h, m)| (*w, *h, m.as_slice()))
    }
    fn only(d: &Dump) -> Option<(usize, usize, &[u8])> {
        match (&d.mask, &d.sweep) {
            (Some((w, h, _)), Some(s)) => Some((*w, *h, s.as_slice())),
            _ => None,
        }
    }
    let sweep_diffs = diff_bytes(
        "hard shadow mask (recorded floor-sweep replay vs Rust full, informational)",
        only(a),
        only(b),
        report,
    );
    if sweep_diffs > 0 {
        let _ = writeln!(
            report,
            "  (not counted: the Rust dump carries its full mask in the sweep slot)"
        );
    }
    diff_bytes("hard shadow mask (full)", full(a), full(b), report)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: floordiff <a.bin> <b.bin>");
        return ExitCode::from(2);
    }
    let load = |p: &str| -> Result<Dump, String> {
        let bytes = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
        parse(&bytes).map_err(|e| format!("{p}: {e}"))
    };
    // A floor with no vertices is never dumped by the original client, so
    // no recorded dump exists for it; the Rust
    // side still writes an empty dump. Missing A + empty B is identical.
    if !std::path::Path::new(&args[1]).exists() {
        return match load(&args[2]) {
            Ok(b) if b.vertex_count == 0 && b.batches.is_empty() => {
                // The mask of an empty floor cannot be checked either
                // (no recorded dump); report what the Rust side has.
                let set = b
                    .sweep
                    .as_ref()
                    .map_or(0, |m| m.iter().filter(|&&v| v != 0).count());
                println!("A: no dump (empty floor)\nB: verts 0 batches 0 sweep mask set {set}\nIDENTICAL");
                ExitCode::SUCCESS
            }
            Ok(b) => {
                println!(
                    "A: no dump\nB: verts {} batches {}\n1 differences",
                    b.vertex_count,
                    b.batches.len()
                );
                ExitCode::from(1)
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        };
    }
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
        "A: verts {} stride {} flags {:#x} batches {}\nB: verts {} stride {} flags {:#x} batches {}",
        a.vertex_count, a.stride, a.flags, a.batches.len(), b.vertex_count, b.stride, b.flags, b.batches.len()
    );
    if a.vertex_count != b.vertex_count || a.stride != b.stride || a.flags != b.flags {
        diffs += 1;
        let _ = writeln!(report, "header differs");
    }
    let n = a.vertex_count.min(b.vertex_count);
    let stride = a.stride.min(b.stride);
    let mut shown = 0;
    let mut stream_diffs = 0;
    for v in 0..n {
        for k in 0..stride {
            let (x, y) = (a.stream0[v * a.stride + k], b.stream0[v * b.stride + k]);
            if x.to_bits() != y.to_bits() {
                stream_diffs += 1;
                if shown < 10 {
                    let _ = writeln!(report, "stream0 vertex {v} float {k}: A {x} B {y}");
                    shown += 1;
                }
            }
        }
    }
    let base_diffs = (0..n).filter(|&v| a.base[v] != b.base[v]).count();
    if let Some(v) = (0..n).find(|&v| a.base[v] != b.base[v]) {
        let _ = writeln!(
            report,
            "base colour vertex {v}: A {:#010x} B {:#010x}",
            a.base[v], b.base[v]
        );
    }
    let _ = writeln!(
        report,
        "stream0 diffs {stream_diffs}, base colour diffs {base_diffs}"
    );
    diffs += stream_diffs + base_diffs;
    let nb = a.batches.len().min(b.batches.len());
    for i in 0..nb {
        let (x, y) = (&a.batches[i], &b.batches[i]);
        if x.material != y.material
            || x.scale != y.scale
            || x.fog != y.fog
            || x.node_id != y.node_id
        {
            diffs += 1;
            let _ = writeln!(
                report,
                "batch {i}: A mat {} scale {} node {} fog {:?} | B mat {} scale {} node {} fog {:?}",
                x.material, x.scale, x.node_id, x.fog, y.material, y.scale, y.node_id, y.fog
            );
        }
        let cd = (0..n).filter(|&v| x.colours[v] != y.colours[v]).count();
        if cd > 0 {
            diffs += cd;
            let v = (0..n).find(|&v| x.colours[v] != y.colours[v]).unwrap_or(0);
            let _ = writeln!(
                report,
                "batch {i} (mat {}): {cd} colour diffs, first vertex {v}: A {:#010x} B {:#010x}",
                x.material, x.colours[v], y.colours[v]
            );
        }
    }
    if a.batches.len() != b.batches.len() {
        diffs += 1;
    }
    diffs += diff_mask(&a, &b, &mut report);
    print!("{report}");
    if diffs == 0 {
        println!("IDENTICAL");
        ExitCode::SUCCESS
    } else {
        println!("{diffs} differences");
        ExitCode::from(1)
    }
}
