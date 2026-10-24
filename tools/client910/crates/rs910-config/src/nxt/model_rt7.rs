//! RT7 models (archive 47, `client.modelsrt7.js5`): NXT's GPU-ready copy of
//! every 910 model (archive 7, same group ids), decoded as NXT 865 reads
//! it (`jag::graphics::Model::LoadDataFromModelRawRT7`, L1152805-1155278),
//! with the 910 format's changes (renderer plan M10, `nxt-data-formats.md`
//! §9).
//!
//! Layout (little-endian except the UVs; every group of the 910 pack is
//! consumed exactly, `nxt::tests::rt7_models_consume_every_group`):
//!
//! - header: `u8` 2, `u8` 3 (skipped by 865, L1153267), `u8` the 910
//!   model's version, `u16` mesh count (L1153268), `u8` billboard, emitter
//!   and effector counts (L1153276-1153282);
//! - per mesh (865's `GeometryBatchInstance`): `u32` flags (L1153351: bit
//!   0 colours, 1 alphas, 2 face labels, 3 vertex labels, 4 hidden),
//!   `u8` priority, `u16` material + 1 (0 none, L1154157), `u16` face
//!   count (L1153475), then per face the `u16` HSL colour (flag 0x1,
//!   L1153500), `u8` alpha (0x2, L1153666), `i16` label (0x4, L1153683);
//!   `u8` LOD count (L1153689) and per LOD a `u16` index count with its
//!   `u16` indices (L1153702; see [`Rt7Mesh::lods`] for the one wrapped
//!   count); `u16` vertex count (L1153901) and per vertex `i16 x3`
//!   position (L1153933), **`i8 x3` normal** (865 `i16 x3 / 32767`,
//!   L1153938), **`i8 x4` tangent** (not in 865), **big-endian `f16 x2`
//!   UV** (865 `f32 x2`, L1153944), `i16` label (0x8, L1153966);
//! - billboards (L1154275): `u8` priority, `u16` material + 1, `u16`
//!   second material + 1 (unknown), `u16` item count, items of `f32 x3`
//!   position, `f32 x2` size, `u16` colour, `u8` alpha, `i16 x3` labels,
//!   `u8` depth offset;
//! - emitters (L1154935): `u16` type, then three of (`f32 x3`, `u16`
//!   label); effectors (L1155121): `u16` type, `f32 x3`, `u16` label.
//!
//! Proven against the legacy (910) model of the same id (lane Q-M10 research
//! over the whole archive): positions are the legacy ones with y negated and
//! times 4 for a legacy version below 13 (the loc-type scale; 512 units per
//! tile, y up); face colours (legacy HSL), alphas (`255 - trans`),
//! materials, face and vertex labels and priorities equal the legacy ones;
//! every non-degenerate legacy face is in LOD 0, wound `(v1, v3, v2)`.

use anyhow::{bail, ensure, Result};

/// The RT7 model archive (`Js5Archive.MODELSRT7`, id 47).
pub const MODEL_RT7_ARCHIVE: &str = "modelsrt7";

/// Mesh flag bits (865 L1153351).
pub const MESH_COLOURS: u32 = 0x1;
pub const MESH_ALPHAS: u32 = 0x2;
pub const MESH_FACE_LABELS: u32 = 0x4;
pub const MESH_VERTEX_LABELS: u32 = 0x8;
/// Hidden: 865 pushes no draw section (L1154146); in the 910 data these
/// hold only the faces the 910 client's billboards and emitters reference.
pub const MESH_HIDDEN: u32 = 0x10;

/// One mesh (one material and priority).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rt7Mesh {
    pub flags: u32,
    pub priority: u8,
    /// Material id (archive 26), `None` untextured.
    pub material: Option<u16>,
    pub face_colours: Vec<u16>,
    pub face_alphas: Vec<u8>,
    pub face_labels: Vec<i16>,
    /// Index lists, LOD 0 first (`3 * faces` indices). A count of 0 would
    /// repeat the previous list (L1153706; not in the 910 data). One group
    /// (104781) has more than 65,535 LOD-0 indices and stores the count
    /// wrapped to 16 bits; the true count is the multiple of three below
    /// `3 * 65536` it came from.
    pub lods: Vec<Vec<u16>>,
    /// Positions, RT7 axes (y up).
    pub positions: Vec<[i16; 3]>,
    /// Normals, `i8` components, not normalised.
    pub normals: Vec<[i8; 3]>,
    /// Tangents: `i8` xyz and the handedness in `w` (127 or -128).
    pub tangents: Vec<[i8; 4]>,
    pub uvs: Vec<[f32; 2]>,
    pub vertex_labels: Vec<i16>,
}

impl Rt7Mesh {
    #[must_use]
    pub fn face_count(&self) -> usize {
        self.lods.first().map_or(0, |l| l.len() / 3)
    }

    #[must_use]
    pub fn hidden(&self) -> bool {
        self.flags & MESH_HIDDEN != 0
    }
}

/// One billboard item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rt7Billboard {
    pub priority: u8,
    pub material: Option<u16>,
    pub material2: Option<u16>,
    pub position: [f32; 3],
    pub size: [f32; 2],
    pub colour: u16,
    pub alpha: u8,
    pub labels: [i16; 3],
    pub depth_offset: u8,
}

/// One particle emitter: its type and three `(position, label)` points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rt7Emitter {
    pub kind: u16,
    pub points: [([f32; 3], u16); 3],
}

/// One effector.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rt7Effector {
    pub kind: u16,
    pub position: [f32; 3],
    pub label: u16,
}

/// One RT7 model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rt7Model {
    /// Header bytes 0 and 1 (2 and 3 in every 910 model).
    pub magic: [u8; 2],
    /// The 910 model's version (header byte 2).
    pub model_version: u8,
    pub meshes: Vec<Rt7Mesh>,
    pub billboards: Vec<Rt7Billboard>,
    pub emitters: Vec<Rt7Emitter>,
    pub effectors: Vec<Rt7Effector>,
}

/// A little-endian cursor.
struct Le<'a> {
    data: &'a [u8],
    pos: usize,
    what: String,
}

impl<'a> Le<'a> {
    fn take(&mut self, n: usize, field: &str) -> Result<&'a [u8]> {
        ensure!(
            self.pos + n <= self.data.len(),
            "{}: truncated {field} at offset {} (len {})",
            self.what,
            self.pos,
            self.data.len()
        );
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    fn u8(&mut self, field: &str) -> Result<u8> {
        Ok(self.take(1, field)?[0])
    }
    fn u16(&mut self, field: &str) -> Result<u16> {
        let b = self.take(2, field)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self, field: &str) -> Result<i16> {
        Ok(self.u16(field)? as i16)
    }
    fn u32(&mut self, field: &str) -> Result<u32> {
        let b = self.take(4, field)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self, field: &str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(field)?))
    }
    fn f32x3(&mut self, field: &str) -> Result<[f32; 3]> {
        Ok([self.f32(field)?, self.f32(field)?, self.f32(field)?])
    }
    fn material(&mut self, field: &str) -> Result<Option<u16>> {
        Ok(self.u16(field)?.checked_sub(1))
    }
}

/// IEEE half-precision bits to `f32`.
#[must_use]
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 if frac == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// Decode one RT7 model file (group `group`, file 0 of archive 47).
pub fn decode_model_rt7(group: u32, data: &[u8]) -> Result<Rt7Model> {
    let mut r = Le {
        data,
        pos: 0,
        what: format!("modelsrt7 group {group}"),
    };
    let mut m = Rt7Model {
        magic: [r.u8("magic")?, r.u8("magic")?],
        model_version: r.u8("version")?,
        ..Rt7Model::default()
    };
    let meshes = r.u16("mesh count")?;
    let billboards = r.u8("billboard count")?;
    let emitters = r.u8("emitter count")?;
    let effectors = r.u8("effector count")?;
    for _ in 0..meshes {
        let flags = r.u32("mesh flags")?;
        let priority = r.u8("priority")?;
        let material = r.material("material")?;
        let faces = usize::from(r.u16("face count")?);
        let mut mesh = Rt7Mesh {
            flags,
            priority,
            material,
            ..Rt7Mesh::default()
        };
        if flags & MESH_COLOURS != 0 {
            mesh.face_colours = (0..faces)
                .map(|_| r.u16("face colour"))
                .collect::<Result<_>>()?;
        }
        if flags & MESH_ALPHAS != 0 {
            mesh.face_alphas = r.take(faces, "face alphas")?.to_vec();
        }
        if flags & MESH_FACE_LABELS != 0 {
            mesh.face_labels = (0..faces)
                .map(|_| r.i16("face label"))
                .collect::<Result<_>>()?;
        }
        let lods = r.u8("lod count")?;
        for _ in 0..lods {
            let mut count = usize::from(r.u16("index count")?);
            if count != 0 && 3 * faces > 0xFFFF {
                // The wrapped count: the one multiple of three it came
                // from (65536 = 1 mod 3).
                count += 65536 * ((3 - count % 3) % 3);
            }
            if count == 0 {
                let Some(prev) = mesh.lods.last().cloned() else {
                    bail!("{}: empty LOD 0", r.what);
                };
                mesh.lods.push(prev);
                continue;
            }
            ensure!(
                count % 3 == 0,
                "{}: index count {count} not a multiple of 3",
                r.what
            );
            let list = (0..count)
                .map(|_| r.u16("index"))
                .collect::<Result<Vec<_>>>()?;
            mesh.lods.push(list);
        }
        let vertices = usize::from(r.u16("vertex count")?);
        if vertices > 0 {
            mesh.positions = (0..vertices)
                .map(|_| Ok([r.i16("x")?, r.i16("y")?, r.i16("z")?]))
                .collect::<Result<_>>()?;
            let n = r.take(3 * vertices, "normals")?;
            mesh.normals = n
                .chunks_exact(3)
                .map(|c| [c[0] as i8, c[1] as i8, c[2] as i8])
                .collect();
            let t = r.take(4 * vertices, "tangents")?;
            mesh.tangents = t
                .chunks_exact(4)
                .map(|c| [c[0] as i8, c[1] as i8, c[2] as i8, c[3] as i8])
                .collect();
            let uv = r.take(4 * vertices, "uvs")?;
            mesh.uvs = uv
                .chunks_exact(4)
                .map(|c| {
                    [
                        f16_to_f32(u16::from_be_bytes([c[0], c[1]])),
                        f16_to_f32(u16::from_be_bytes([c[2], c[3]])),
                    ]
                })
                .collect();
            if flags & MESH_VERTEX_LABELS != 0 {
                mesh.vertex_labels = (0..vertices)
                    .map(|_| r.i16("vertex label"))
                    .collect::<Result<_>>()?;
            }
        }
        for lod in &mesh.lods {
            ensure!(
                lod.iter().all(|&i| usize::from(i) < vertices),
                "{}: index past {vertices} vertices",
                r.what
            );
        }
        m.meshes.push(mesh);
    }
    for _ in 0..billboards {
        let priority = r.u8("billboard priority")?;
        let material = r.material("billboard material")?;
        let material2 = r.material("billboard material 2")?;
        let items = r.u16("billboard items")?;
        for _ in 0..items {
            let position = r.f32x3("billboard position")?;
            let size = [r.f32("billboard size")?, r.f32("billboard size")?];
            m.billboards.push(Rt7Billboard {
                priority,
                material,
                material2,
                position,
                size,
                colour: r.u16("billboard colour")?,
                alpha: r.u8("billboard alpha")?,
                labels: [
                    r.i16("billboard label")?,
                    r.i16("billboard label")?,
                    r.i16("billboard label")?,
                ],
                depth_offset: r.u8("billboard depth")?,
            });
        }
    }
    for _ in 0..emitters {
        let kind = r.u16("emitter")?;
        let mut points = [([0.0; 3], 0); 3];
        for p in &mut points {
            *p = (r.f32x3("emitter point")?, r.u16("emitter label")?);
        }
        m.emitters.push(Rt7Emitter { kind, points });
    }
    for _ in 0..effectors {
        m.effectors.push(Rt7Effector {
            kind: r.u16("effector")?,
            position: r.f32x3("effector position")?,
            label: r.u16("effector label")?,
        });
    }
    ensure!(
        r.pos == data.len(),
        "{}: {} trailing bytes after offset {}",
        r.what,
        data.len() - r.pos,
        r.pos
    );
    Ok(m)
}
