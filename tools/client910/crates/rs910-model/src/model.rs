//! Wire decoding of the stored model into [`RawModel`] plus its texture
//! companion [`TexData`], read by the interface icon model owner
//! (`ui_icon_model`).
//!
//! # Format
//! The decoder reads the model's footer and its run of streams with several
//! cursors: face-index opcodes 1 to 4 place each face's corners relative to
//! the previous face, texture triangles carry a kind (0 to 3), and every
//! width keys off the per-model `version` byte (the texture scale stride is
//! 6, 7 or 9 bytes, and mappings are a plain byte or a smart value from
//! version 16).
//!
//! # Deliberate deviations from the stored format's reference behaviour
//! 1. An unknown model tag (first byte not 1) is a hard [`ModelError::Invalid`];
//!    an empty model would only move the failure into the consumer.
//! 2. Face-index opcodes outside 1 to 4 and texture-triangle types outside 0 to
//!    3 are rejected. The reference reader silently reuses the previous face or
//!    skips the triangle; real encoders only emit 1 to 4 and 0 to 3, so
//!    anything else is corrupt input.
//! 3. Vertex labels, face labels, emitters, effectors and billboards are
//!    parsed and dropped (the cursor movement is exact); the live model path
//!    (`modelunlit.rs`) keeps them.
//! 4. Texture triangles keep `kind` and three vertex indices ([`TexTri`]) and
//!    the full procedural payload; [`TexData::corner_uvs`] resolves exact
//!    per-corner coordinates (direct, default, kind 0, and the cylindrical,
//!    planar and spherical kinds 1 to 3).
//! 5. `version < 13` models are not auto-scaled: the loc loader scales them
//!    by four at bind time, outside the decoder.

use crate::cache::CacheError;
use rs910_core::reader::{Eof, Reader as CoreReader};
use thiserror::Error;

/// Failures model decoding can produce. Public functions surface these as
/// `anyhow::Error`; only a CLI boundary converts further.
#[derive(Debug, Error)]
pub enum ModelError {
    /// A read ran past the end of the buffer.
    #[error("unexpected end of model data reading {what}")]
    Truncated {
        /// What was being read (e.g. `"footer"`, `"face colour"`).
        what: &'static str,
    },
    /// The bytes are shaped wrong for the `ModelUnlit` layout.
    #[error("invalid model data: {0}")]
    Invalid(String),
    /// Pack IO / decompression failure.
    #[error(transparent)]
    Cache(#[from] CacheError),
}

// ---------------------------------------------------------------------------
// Byte reader (the reads live in `rs910_core::reader`; this cursor keeps the
// model errors)
// ---------------------------------------------------------------------------

/// Byte cursor over a model blob.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn at(data: &'a [u8], pos: usize) -> Result<Self, ModelError> {
        if pos > data.len() {
            return Err(ModelError::Truncated {
                what: "stream start",
            });
        }
        Ok(Self { data, pos })
    }

    /// One `rs910_core::reader` read at `pos`; `pos` follows it, a failure is
    /// `Truncated`.
    fn read<T>(
        &mut self,
        what: &'static str,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> Result<T, ModelError> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|_| ModelError::Truncated { what })
    }

    /// Unsigned byte.
    fn g1(&mut self, what: &'static str) -> Result<u8, ModelError> {
        self.read(what, CoreReader::g1)
    }

    /// Signed byte (callers needing the raw wire pattern keep the `u8` from
    /// [`Cursor::g1`]).
    fn g1b(&mut self, what: &'static str) -> Result<i8, ModelError> {
        self.read(what, CoreReader::g1b)
    }

    /// Big-endian unsigned short.
    fn g2(&mut self, what: &'static str) -> Result<u32, ModelError> {
        self.read(what, CoreReader::g2).map(u32::from)
    }

    /// Big-endian signed short.
    fn g2s(&mut self, what: &'static str) -> Result<i32, ModelError> {
        self.read(what, CoreReader::g2s).map(i32::from)
    }

    /// Big-endian unsigned 3-byte int.
    fn g3(&mut self, what: &'static str) -> Result<i32, ModelError> {
        self.read(what, CoreReader::g3).map(|v| v as i32)
    }

    /// Signed delta smart.
    fn gsmart1or2s(&mut self, what: &'static str) -> Result<i32, ModelError> {
        self.read(what, CoreReader::gsmart1or2s)
    }

    /// Unsigned smart.
    fn gsmart1or2(&mut self, what: &'static str) -> Result<i32, ModelError> {
        self.read(what, CoreReader::gsmart1or2)
    }

    /// Nullable smart, `-1` == none.
    fn gsmart1or2null(&mut self, what: &'static str) -> Result<i32, ModelError> {
        self.read(what, CoreReader::gsmart1or2null)
    }
}

// ---------------------------------------------------------------------------
// RawModel: wire-exact decode
// ---------------------------------------------------------------------------

/// One texture triangle: `kind` is the texture triangle type byte (0-3),
/// `verts` the three referenced vertex indices. Kind 0 is plain UV (corners
/// sample the [`TexData`] coordinates directly); kinds 1-3 are procedural
/// (the render track transforms corners by [`scale`](Self::scale) spun along
/// [`direction`](Self::direction) and scrolled by [`speed`](Self::speed)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexTri {
    /// Triangle kind (0 = plain UV, 1-3 = procedural).
    pub kind: u8,
    /// Referenced vertex indices.
    pub verts: [u32; 3],
    /// Procedural scale triple as stored (widths key off the model version).
    /// `None` for kind 0 (no scale stream).
    pub scale: Option<[i32; 3]>,
    /// Spin about Y in 1/256 turns (`g1b`; kinds 1-3 only, `0` for kind 0).
    pub rotation: i8,
    /// Procedural spin direction (`g1b`; kinds 1-3 only, `0` for kind 0).
    pub direction: i8,
    /// Procedural scroll speed (`g1b`; kinds 1-3 only, `0` for kind 0).
    pub speed: i8,
    /// Type-2 extra translations (`(U, V)` as `g1b` pair). `Some` only for
    /// kind 2, which carries two more bytes.
    pub trans: Option<[i8; 2]>,
}

/// A decoded model: every footer field is accounted for; unsupported
/// sections (emitters/effectors, billboards, UV payloads) are parsed and
/// dropped so stream alignment stays wire-exact.
#[derive(Clone, Debug)]
pub struct RawModel {
    /// Model format version (default 12). Widths for tex-scale and
    /// face-mapping reads key off this.
    pub version: u8,
    /// Vertex positions as stored (wire units, `+Y`-down: up is `-Y`; floors
    /// and models upload wire Y directly with an up normal of `(0,-1,0)`).
    /// NOT auto-scaled: see the module docs (x4 when `version < 13`). UV
    /// projection uses these wire coords directly.
    pub verts: Vec<[f32; 3]>,
    /// Faces as vertex-index triples (winding as stored).
    pub faces: Vec<[u32; 3]>,
    /// Per-face colour as stored HSL components `[hue6, sat3, light7]`
    /// (`faceColour` short split as `(c >> 10) & 63`, `(c >> 7) & 7`,
    /// `c & 127`); an index into the HSL palette.
    pub colors: Vec<[u8; 3]>,
    /// Per-face material (`g2() - 1`; `-1` == none).
    pub materials: Vec<i32>,
    /// Per-face priority as the raw wire byte (the model's default priority
    /// when the footer says there are no per-face priorities).
    pub priorities: Vec<u8>,
    /// Per-face translucency as the raw wire byte (default 0).
    pub alphas: Vec<u8>,
    /// Per-face render type as the raw wire byte (present only when footer
    /// flag `0x1` is set).
    /// `0` = smooth-shaded, `1` = flat (see normals deviation in module docs).
    pub face_types: Vec<u8>,
    /// Texture triangles (`kind` + indices only; payload dropped).
    pub tex_tris: Vec<TexTri>,
}

/// Texture payloads of one model, decoded by the same pass as
/// [`RawModel`] (see [`decode_tex`]).
///
/// Wire sources: the textured-vertex counts + UV streams
/// (present when footer flags `& 0x80 != 0`), the face mapping stream
/// (present only with materials AND texture tris / textured verts) and the
/// per-face texture-vertex offsets (three bytes on faces whose opcode has
/// `0x8` set, only when textured verts exist).
///
/// Layout rule for [`uvs`](Self::uvs): vertex `i` owns the next
/// [`vert_counts`](Self::vert_counts)`[i]` consecutive pairs (pack order).
/// Lumbridge census (2026-09-09, all referenced loc models):
/// `sum(counts) == uvs.len()` holds for every textured model, confirming the
/// rule empirically.
#[derive(Clone, Debug, Default)]
pub struct TexData {
    /// Per-vertex texture-pair counts (`vertex_count` bytes).
    /// Empty when the model is not textured.
    pub vert_counts: Vec<u8>,
    /// Raw texture-coordinate pairs (`(U, V)` as stored `g2s`; normalize with
    /// [`TEX_UV_SCALE`]).
    pub uvs: Vec<[i32; 2]>,
    /// Per-face mapping index (tex-tri selector), exact (`wire - 1`; `-1` when
    /// the face has no material — consuming no stream byte — or maps to no
    /// triangle; `32766` is the direct-UV sentinel the lit-model build
    /// resolves through the vertex's texture offset plus the face's
    /// texture-vertex offset). `Some` iff mappings
    /// exist (`has_material && (tex_tris || textured verts)`); `None`
    /// otherwise.
    pub face_mappings: Option<Vec<i32>>,
    /// Per-face texture-vertex offsets, parallel to [`RawModel::faces`];
    /// `Some([u, v, w])` on faces whose opcode carries `0x8` (textured
    /// models only), `None` on the rest.
    pub face_tex_offsets: Vec<Option<[u8; 3]>>,
}

/// Scale from raw `g2s` texture coordinates to upload UVs: exactly the
/// lit-model input scale (`(float) g2s() / 4096.0F`).
///
/// Values are NOT clamped to `0..1`: Lumbridge census over every textured loc
/// model referenced by level-0 locs shows normalized UVs spanning about
/// `±8` (texture tiling through a wrapping sampler), so out-of-range values
/// are expected input, not corrupt data. Procedural kinds 1-3 transform these
/// further at upload (with `speed / 256`, `scale(Z) / 1024`,
/// `64 / scale(XYZ)`).
pub const TEX_UV_SCALE: f32 = 1.0 / 4096.0;

impl TexData {
    /// Exact per-corner UVs for one face.
    ///
    /// Needs `raw` alongside `self`: only [`TexData`] holds the UV streams and
    /// mapping selectors, but kinds 0-3 all read [`RawModel`] state too (face +
    /// tri vertex positions, tri kind/payloads, model version). A
    /// `(&self, face)`-only shape cannot express that, so the version/scale
    /// context arrives via `raw`.
    ///
    /// Resolution per face (`material >= 0` required, else `None` — untextured
    /// faces take the colour branch):
    /// - mapping `32766`: direct UVs from [`uvs`](Self::uvs) at
    ///   `prefix_sum(vert_counts[vert]) + offset`, scaled by
    ///   [`TEX_UV_SCALE`]. Missing offsets behave as `[0, 0, 0]` (the offset
    ///   arrays are zero-initialised when textured verts exist); out-of-range
    ///   bases/UVs yield `None`.
    /// - mapping `-1`: the default triangle `(0,1),(1,1),(0,0)`.
    /// - tri `t`, kind 0: barycentric projection of the face verts onto the tri.
    /// - tri `t`, kinds 1-3: cylindrical / planar (cube) / spherical projection
    ///   over the bounds centre and the rotation matrix of the triangle, with
    ///   `speed / 256`, `trans / 256`, `scale(Z) / 1024`, `64 / scale`.
    ///
    /// `version < 13`: wire coords are evaluated x4 and tri scales `<< 2`
    /// (X/Y always, Z except kind 1), reproducing the bind-time
    /// power-of-two rescale (by 2) that the loc type
    /// applies per part before upload. Animate-able params (speed scroll,
    /// kind-2 translations) are evaluated at t = 0: the stored bytes feed the
    /// formulas directly with no frame counter accumulated (the live path
    /// advances them per frame for animated models; the static bind pose is
    /// t = 0).
    ///
    /// Returns `None` for genuinely unportable faces (no mapping stream, bad
    /// tri index, missing kind 1-3 scale payload, tri with no contributing
    /// face, or a non-finite result from degenerate input). Never panics.
    #[allow(dead_code)]
    #[must_use]
    pub fn corner_uvs(&self, raw: &RawModel, face: usize) -> Option<[[f32; 2]; 3]> {
        let table = build_proc_table(self, raw);
        corner_uvs_with(self, raw, face, &table)
    }

    /// Exact per-corner UVs for every face in [`RawModel::faces`], batched over
    /// one shared procedural table (see [`build_proc_table`]).
    ///
    /// Element `face` is [`corner_uvs`](Self::corner_uvs)`(raw, face)` — `Some`
    /// for directly/default/kind-0/kinds-1-3 exact faces (see its docs), `None`
    /// for untextured faces (`materials[face] < 0`), missing mapping streams,
    /// corrupt payloads or degenerate geometry. Pure, never panics (one table
    /// for all faces).
    #[must_use]
    pub fn corner_uvs_batch(&self, raw: &RawModel) -> Vec<Option<[[f32; 2]; 3]>> {
        let table = build_proc_table(self, raw);
        let mut out = Vec::with_capacity(raw.faces.len());
        for face in 0..raw.faces.len() {
            out.push(corner_uvs_with(self, raw, face, &table));
        }
        out
    }
}

/// Decode one model blob (the payload of file 0 of a models group).
pub fn decode(data: &[u8]) -> anyhow::Result<RawModel> {
    Ok(decode_raw(data)?.0)
}

/// Decode one `ModelUnlit` blob's texture payloads (companion to [`decode`],
/// same single pass — see [`TexData`]).
pub fn decode_tex(data: &[u8]) -> anyhow::Result<TexData> {
    Ok(decode_raw(data)?.1)
}

fn decode_raw(data: &[u8]) -> Result<(RawModel, TexData), ModelError> {
    // Header (3 bytes) + footer (26 bytes) are the absolute minimum.
    if data.len() < 29 {
        return Err(ModelError::Truncated {
            what: "model header+footer",
        });
    }
    // The tag must be 1.
    if data[0] != 1 {
        return Err(ModelError::Invalid(format!(
            "unsupported model tag {} (expects 1)",
            data[0]
        )));
    }
    // Byte 1 skipped, byte 2 is the version.
    let version = data[2];

    // Footer at len-26.
    let mut foot = Cursor::at(data, data.len() - 26)?;
    let vertex_count = foot.g2("footer")? as usize;
    let face_count = foot.g2("footer")? as usize;
    let tex_tri_count = foot.g2("footer")? as usize;
    let flags = foot.g1("footer")?;
    let pri_mode = foot.g1("footer")?;
    let has_trans = foot.g1("footer")?;
    let has_face_label = foot.g1("footer")?;
    let has_material = foot.g1("footer")?;
    let has_vertex_label = foot.g1("footer")?;
    let sec_x = foot.g2("footer")? as usize; // X-delta section length
    let sec_y = foot.g2("footer")? as usize; // Y-delta section length
    let sec_z = foot.g2("footer")? as usize; // Z-delta section length
    let sec_face_idx = foot.g2("footer")? as usize; // face-index section length
    let sec_mapping = foot.g2("footer")? as usize; // mapping section length
    let sec_vertex_label = foot.g2("footer")? as usize; // vertex-label section length
    let sec_face_label = foot.g2("footer")? as usize; // face-label section length

    // Cheap corruption guard: every vertex/face/tri owns at least one byte.
    for (name, count) in [
        ("vertex", vertex_count),
        ("face", face_count),
        ("texture triangle", tex_tri_count),
    ] {
        if count > data.len() {
            return Err(ModelError::Invalid(format!(
                "{name} count {count} exceeds {0}-byte blob",
                data.len()
            )));
        }
    }

    let has_face_type = flags & 0x1 != 0;
    let has_particles = flags & 0x2 != 0;
    let has_billboard = flags & 0x4 != 0;
    let vlabel_ext = flags & 0x10 != 0;
    let flabel_ext = flags & 0x20 != 0;
    let blabel_ext = flags & 0x40 != 0;
    let textured = flags & 0x80 != 0;

    // Texture-triangle type bytes at offset 3.
    let mut type_cursor = Cursor::at(data, 3)?;
    let mut tex_types = Vec::with_capacity(tex_tri_count);
    let (mut count_t0, mut count_t1, mut count_t2) = (0_usize, 0_usize, 0_usize);
    for _ in 0..tex_tri_count {
        let kind = type_cursor.g1b("texture triangle type")?;
        tex_types.push(kind);
        if kind == 0 {
            count_t0 += 1;
        }
        if (1..=3).contains(&kind) {
            count_t1 += 1;
        }
        if kind == 2 {
            count_t2 += 1;
        }
    }

    // Stream offsets. Checked addition: overflow is
    // corrupt input, never a wrap.
    let add = |base: usize, extra: usize| {
        base.checked_add(extra)
            .ok_or_else(|| ModelError::Invalid("model stream offset overflowed".to_string()))
    };
    let mul = |count: usize, width: usize| {
        count
            .checked_mul(width)
            .ok_or_else(|| ModelError::Invalid("model stream size overflowed".to_string()))
    };
    let o_vert_flags = add(3, tex_tri_count)?;
    let o_face_type = add(o_vert_flags, vertex_count)?;
    let o_opcode = add(o_face_type, if has_face_type { face_count } else { 0 })?;
    let o_face_label = add(o_opcode, face_count)?; // before the priority stream
    let o_face_label_data = add(o_face_label, if pri_mode == 255 { face_count } else { 0 })?;
    let o_vertex_label = add(o_face_label_data, sec_face_label)?;
    let o_face_deltas = add(o_vertex_label, sec_vertex_label)?; // before the trans stream
    let o_trans = o_face_deltas;
    let o_face_deltas_data = add(o_face_deltas, if has_trans == 1 { face_count } else { 0 })?;
    let o_mapping = add(o_face_deltas_data, sec_face_idx)?; // before the material stream
    let o_material = o_mapping;
    let o_mapping_data = add(
        o_mapping,
        if has_material == 1 {
            mul(face_count, 2)?
        } else {
            0
        },
    )?;
    let o_colour = add(o_mapping_data, sec_mapping)?;
    let o_dx = add(o_colour, mul(face_count, 2)?)?;
    let o_dy = add(o_dx, sec_x)?;
    let o_dz = add(o_dy, sec_y)?;
    let o_tex0 = add(o_dz, sec_z)?;
    let o_tex1 = add(o_tex0, mul(count_t0, 6)?)?;
    let o_texscale = add(o_tex1, mul(count_t1, 6)?)?;
    // Per-tri scale stride keys off the version.
    let scale_stride = if version == 14 {
        7
    } else if version >= 15 {
        9
    } else {
        6
    };
    let o_texa = add(o_texscale, mul(count_t1, scale_stride)?)?; // rotation
    let o_texb = add(o_texa, count_t1)?; // direction
    let o_texc = add(o_texb, count_t1)?; // speed + type-2 translations
    let o_particles = add(o_texc, add(count_t1, mul(count_t2, 2)?)?)?;

    // Vertices: flag byte + X/Y/Z smart deltas.
    let mut c_flags = Cursor::at(data, o_vert_flags)?;
    let mut c_dx = Cursor::at(data, o_dx)?;
    let mut c_dy = Cursor::at(data, o_dy)?;
    let mut c_dz = Cursor::at(data, o_dz)?;
    let mut c_vlabel = Cursor::at(data, o_vertex_label)?;
    let mut verts = Vec::with_capacity(vertex_count);
    // Skin labels: read to validate the section;
    // absent entirely when the section flag is off.
    let mut vert_labels = Vec::with_capacity(vertex_count);
    let (mut acc_x, mut acc_y, mut acc_z) = (0_i32, 0_i32, 0_i32);
    for _ in 0..vertex_count {
        let flag = c_flags.g1("vertex flags")?;
        if flag & 0x1 != 0 {
            acc_x = acc_x
                .checked_add(c_dx.gsmart1or2s("vertex x delta")?)
                .ok_or_else(|| ModelError::Invalid("vertex x overflowed".to_string()))?;
        }
        if flag & 0x2 != 0 {
            acc_y = acc_y
                .checked_add(c_dy.gsmart1or2s("vertex y delta")?)
                .ok_or_else(|| ModelError::Invalid("vertex y overflowed".to_string()))?;
        }
        if flag & 0x4 != 0 {
            acc_z = acc_z
                .checked_add(c_dz.gsmart1or2s("vertex z delta")?)
                .ok_or_else(|| ModelError::Invalid("vertex z overflowed".to_string()))?;
        }
        verts.push([acc_x as f32, acc_y as f32, acc_z as f32]);
        if has_vertex_label == 1 {
            // Retained (skin labels for AnimBase/AnimFrame); the cursor must
            // still advance exactly.
            let label = if vlabel_ext {
                c_vlabel.gsmart1or2null("vertex label")?
            } else {
                let raw = c_vlabel.g1("vertex label")?;
                if raw == 255 {
                    -1
                } else {
                    i32::from(raw)
                }
            };
            vert_labels.push(label);
        }
    }

    // Textured-vertex counts + UVs.
    // Retained in TexData (see the texture contract): vertex `i` owns the
    // next `counts[i]` consecutive UV pairs.
    let (tex_vert_count, o_texface, tex_vert_counts, tex_uvs) = if textured {
        let len = data.len();
        let back = data[len - 27] as i8 as i64;
        let base = (len as i64 - 26 - back) as usize;
        // The 3-g2 sub-header must sit fully before the footer.
        if back < 0 || base.checked_add(6).is_none_or(|end| end > len - 26) {
            return Err(ModelError::Invalid(
                "textured-vertex header points outside the model".to_string(),
            ));
        }
        let mut head = Cursor::at(data, base)?;
        let tcount = head.g2("textured vertex count")? as usize;
        let sec_particles = head.g2("textured vertex count")? as usize;
        let sec_texface = head.g2("textured vertex count")? as usize;
        let o74 = add(o_particles, sec_particles)?; // face tex offsets
        let o75 = add(o74, sec_texface)?; // per-vertex tex counts
        let o76 = add(o75, vertex_count)?; // U coords
        let o77 = add(o76, mul(tcount, 2)?)?; // V coords
        let mut c_cnt = Cursor::at(data, o75)?;
        let mut c_u = Cursor::at(data, o76)?;
        let mut c_v = Cursor::at(data, o77)?;
        let mut tex_vert_counts = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            tex_vert_counts.push(c_cnt.g1("textured vertex count")?);
        }
        let mut tex_uvs = Vec::with_capacity(tcount);
        for _ in 0..tcount {
            let u = c_u.g2s("texture U")?;
            let v = c_v.g2s("texture V")?;
            tex_uvs.push([u, v]);
        }
        (tcount, o74, tex_vert_counts, tex_uvs)
    } else {
        (0_usize, 0_usize, Vec::new(), Vec::new())
    };

    // Faces.
    let mut c_colour = Cursor::at(data, o_colour)?;
    let mut c_type = Cursor::at(data, o_face_type)?;
    let mut c_pri = Cursor::at(data, o_face_label)?;
    let mut c_tra = Cursor::at(data, o_trans)?;
    let mut c_flabel = Cursor::at(data, o_face_label_data)?;
    let mut c_mat = Cursor::at(data, o_material)?;
    let mut c_map = Cursor::at(data, o_mapping_data)?;
    // Mappings exist only with materials AND
    // (texture tris OR textured verts).
    let has_mapping = has_material == 1 && (tex_tri_count > 0 || tex_vert_count > 0);
    let default_pri = pri_mode;
    let mut colors = Vec::with_capacity(face_count);
    let mut face_types = Vec::with_capacity(face_count);
    let mut priorities = Vec::with_capacity(face_count);
    let mut alphas = Vec::with_capacity(face_count);
    let mut materials = Vec::with_capacity(face_count);
    // Skin face labels: read to validate the section.
    let mut face_labels = Vec::with_capacity(face_count);
    // Retained for TexData (None when mappings are absent).
    let mut face_mappings: Option<Vec<i32>> = has_mapping.then(|| Vec::with_capacity(face_count));
    for _ in 0..face_count {
        let colour = c_colour.g2("face colour")?;
        colors.push([
            ((colour >> 10) & 63) as u8,
            ((colour >> 7) & 7) as u8,
            (colour & 127) as u8,
        ]);
        if has_face_type {
            face_types.push(c_type.g1("face type")?);
        } else {
            face_types.push(0);
        }
        if pri_mode == 255 {
            priorities.push(c_pri.g1("face priority")?);
        } else {
            priorities.push(default_pri);
        }
        if has_trans == 1 {
            alphas.push(c_tra.g1("face translucency")?);
        } else {
            alphas.push(0);
        }
        if has_face_label == 1 {
            // Retained (animation face labels).
            let label = if flabel_ext {
                c_flabel.gsmart1or2null("face label")?
            } else {
                let raw = c_flabel.g1("face label")?;
                if raw == 255 {
                    -1
                } else {
                    i32::from(raw)
                }
            };
            face_labels.push(label);
        }
        if has_material == 1 {
            let material = c_mat.g2("face material")? as i32 - 1;
            materials.push(material);
        } else {
            materials.push(-1);
        }
        if has_mapping {
            // Exact: a face with no material
            // (`-1`) consumes NO mapping byte and maps to `-1`; otherwise the
            // stored value is `wire - 1`. `32766` is the direct-UV sentinel
            // (only reachable as wire `32767` on the `version >= 16` smart
            // path); `-1` takes the default UV triangle in `GpuModel`.
            // Unconditional reads here would desync the mapping cursor on
            // mixed-material models, so the consume is conditional too.
            let material = materials[materials.len() - 1];
            let mapping = if material == -1 {
                -1
            } else if version >= 16 {
                c_map.gsmart1or2("face mapping")? - 1
            } else {
                i32::from(c_map.g1("face mapping")?) - 1
            };
            if let Some(slot) = face_mappings.as_mut() {
                slot.push(mapping);
            }
        }
    }

    // Face indices.
    let mut c_op = Cursor::at(data, o_opcode)?;
    let mut c_di = Cursor::at(data, o_face_deltas_data)?;
    let mut c_tx = Cursor::at(data, o_texface)?;
    let (mut v0, mut v1, mut v2, mut last) = (0_i32, 0_i32, 0_i32, 0_i32);
    let mut faces = Vec::with_capacity(face_count);
    let mut face_tex_offsets = Vec::with_capacity(face_count);
    for _ in 0..face_count {
        let opcode = c_op.g1("face opcode")?;
        match opcode & 0x7 {
            // Three fresh deltas.
            1 => {
                v0 = c_di
                    .gsmart1or2s("face index")?
                    .checked_add(last)
                    .ok_or_else(|| ModelError::Invalid("face index overflowed".to_string()))?;
                v1 = c_di
                    .gsmart1or2s("face index")?
                    .checked_add(v0)
                    .ok_or_else(|| ModelError::Invalid("face index overflowed".to_string()))?;
                v2 = c_di
                    .gsmart1or2s("face index")?
                    .checked_add(v1)
                    .ok_or_else(|| ModelError::Invalid("face index overflowed".to_string()))?;
                last = v2;
            }
            // Reuse (v0, v2), one fresh index.
            2 => {
                v1 = v2;
                v2 = c_di
                    .gsmart1or2s("face index")?
                    .checked_add(last)
                    .ok_or_else(|| ModelError::Invalid("face index overflowed".to_string()))?;
                last = v2;
            }
            // Reuse (v2, v1)→(v1 := v2), one fresh.
            3 => {
                v0 = v2;
                v2 = c_di
                    .gsmart1or2s("face index")?
                    .checked_add(last)
                    .ok_or_else(|| ModelError::Invalid("face index overflowed".to_string()))?;
                last = v2;
            }
            // Swap v0/v1, one fresh index.
            4 => {
                std::mem::swap(&mut v0, &mut v1);
                v2 = c_di
                    .gsmart1or2s("face index")?
                    .checked_add(last)
                    .ok_or_else(|| ModelError::Invalid("face index overflowed".to_string()))?;
                last = v2;
            }
            other => {
                return Err(ModelError::Invalid(format!(
                    "bad face-index opcode {other} (only 1-4 exist)"
                )));
            }
        }
        for index in [v0, v1, v2] {
            if index < 0 || index as usize >= vertex_count {
                return Err(ModelError::Invalid(format!(
                    "face index {index} outside 0..{vertex_count}"
                )));
            }
        }
        faces.push([v0 as u32, v1 as u32, v2 as u32]);
        if tex_vert_count > 0 && opcode & 0x8 != 0 {
            // Retained in TexData.
            let a = c_tx.g1("face texture offset")?;
            let b = c_tx.g1("face texture offset")?;
            let c = c_tx.g1("face texture offset")?;
            face_tex_offsets.push(Some([a, b, c]));
        } else {
            face_tex_offsets.push(None);
        }
    }

    // Texture triangles: keep kind + verts,
    // parse-and-drop the payload (untextured grey-box).
    let mut c_t0 = Cursor::at(data, o_tex0)?;
    let mut c_t1 = Cursor::at(data, o_tex1)?;
    let mut c_ts = Cursor::at(data, o_texscale)?;
    let mut c_ta = Cursor::at(data, o_texa)?;
    let mut c_tb = Cursor::at(data, o_texb)?;
    let mut c_tc = Cursor::at(data, o_texc)?;
    let mut tex_tris = Vec::with_capacity(tex_tri_count);
    for kind in tex_types {
        match kind {
            0 => {
                let a = c_t0.g2("texture triangle vertex")?;
                let b = c_t0.g2("texture triangle vertex")?;
                let c = c_t0.g2("texture triangle vertex")?;
                tex_tris.push(TexTri {
                    kind: 0,
                    verts: [a, b, c],
                    scale: None,
                    rotation: 0,
                    direction: 0,
                    speed: 0,
                    trans: None,
                });
            }
            1..=3 => {
                let a = c_t1.g2("texture triangle vertex")?;
                let b = c_t1.g2("texture triangle vertex")?;
                let c = c_t1.g2("texture triangle vertex")?;
                // Scale widths key off the version.
                let scale = if version < 15 {
                    let s0 = c_ts.g2("texture scale")? as i32;
                    let s1 = if version < 14 {
                        c_ts.g2("texture scale")? as i32
                    } else {
                        c_ts.g3("texture scale")?
                    };
                    let s2 = c_ts.g2("texture scale")? as i32;
                    [s0, s1, s2]
                } else {
                    [
                        c_ts.g3("texture scale")?,
                        c_ts.g3("texture scale")?,
                        c_ts.g3("texture scale")?,
                    ]
                };
                let rotation = c_ta.g1b("texture rotation")?;
                let direction = c_tb.g1b("texture direction")?; // direction
                let speed = c_tc.g1b("texture speed")?; // speed
                let trans = if kind == 2 {
                    Some([
                        c_tc.g1b("texture translation U")?,
                        c_tc.g1b("texture translation V")?,
                    ])
                } else {
                    None
                };
                tex_tris.push(TexTri {
                    kind: kind as u8,
                    verts: [a, b, c],
                    scale: Some(scale),
                    rotation,
                    direction,
                    speed,
                    trans,
                });
            }
            other => {
                return Err(ModelError::Invalid(format!(
                    "bad texture-triangle type {other} (only 0-3 exist)"
                )));
            }
        }
    }

    // Emitters / effectors and billboards
    // Parsed-and-dropped. Counts prefix each run so skipping is
    // exact; billboard labels are variable-width and must be read, not jumped.
    let mut c_part = Cursor::at(data, o_particles)?;
    if has_particles {
        let emitter_count = c_part.g1("emitter count")? as usize;
        for _ in 0..emitter_count {
            c_part.g2("emitter priority")?;
            c_part.g2("emitter face")?;
        }
        let effector_count = c_part.g1("effector count")? as usize;
        for _ in 0..effector_count {
            c_part.g2("effector vertex")?;
            c_part.g2("effector face")?;
        }
    }
    if has_billboard {
        let board_count = c_part.g1("billboard count")? as usize;
        for _ in 0..board_count {
            c_part.g2("billboard vertex")?;
            c_part.g2("billboard face")?;
            if blabel_ext {
                c_part.gsmart1or2null("billboard label")?;
            } else {
                c_part.g1("billboard label")?;
            }
            c_part.g1b("billboard render type")?;
        }
    }

    Ok((
        RawModel {
            version,
            verts,
            faces,
            colors,
            materials,
            priorities,
            alphas,
            face_types,
            tex_tris,
        },
        TexData {
            vert_counts: tex_vert_counts,
            uvs: tex_uvs,
            face_mappings,
            face_tex_offsets,
        },
    ))
}

// ---------------------------------------------------------------------------
// Procedural UVs: the cylindrical, planar and spherical projections
// ---------------------------------------------------------------------------

/// Default UV triangle for mapping `-1`.
const DEFAULT_UV_TRI: [[f32; 2]; 3] = [[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]];

/// One precomputed kinds-1-3 triangle: bounds centre + rotation matrix + kind
/// params.
struct ProcTri {
    /// Triangle kind (1 = cylindrical, 2 = planar, 3 = spherical).
    kind: u8,
    /// Bounds centre (`(min + max) / 2` integer division).
    centre: [f32; 3],
    /// Rotation matrix, rows pre-scaled by kind.
    mat: [f32; 9],
    /// Spin direction in quarter turns.
    dir: i8,
    /// Scroll speed `/ 256` (t = 0 evaluation).
    speed: f32,
    /// Kind-2 extra translations `/ 256`.
    trans: [f32; 2],
    /// Kind-1 cylinder wrap scale (scale Z `/ 1024`).
    cyl_scale: f32,
}

/// Effective tri scales with the `version < 13` bind-time shift applied
/// (X/Y always `<< 2`, Z except kind 1). `None` when the part carries no scale payload (decoder always
/// sets it for kinds 1-3; hand-built models may lack it → fallback).
fn effective_scales(version: u8, tri: &TexTri) -> Option<[i32; 3]> {
    let mut scales = tri.scale?;
    if version < 13 {
        scales[0] = scales[0].wrapping_shl(2);
        scales[1] = scales[1].wrapping_shl(2);
        if tri.kind != 1 {
            scales[2] = scales[2].wrapping_shl(2);
        }
    }
    Some(scales)
}

/// Bounds centre of one tri over faces selecting it.
///
/// Only faces whose stored mapping EQUALS `tri_index` contribute (a mapping
/// in `0..32766` — the caller resolves one face, so its own tri always
/// contributes at least itself). Integer `(min + max) / 2` per axis over the
/// version-scaled wire coords (scaling first matters: the client divides the
/// SCALED ints, and odd sums truncate differently). Bounds arrive
/// precomputed single-pass (see [`build_proc_table`]); `None` when no face
/// contributes (unreachable from [`TexData::corner_uvs` on a face selecting
/// the tri; corrupt-input guard, never panic).
fn tri_centre_from(bounds: Option<([i64; 3], [i64; 3])>) -> Option<[f32; 3]> {
    let (min, max) = bounds?;
    // Integer division truncates toward zero — Rust i64 `/` matches.
    Some([
        ((min[0] + max[0]) / 2) as f32,
        ((min[1] + max[1]) / 2) as f32,
        ((min[2] + max[2]) / 2) as f32,
    ])
}

/// Rotation matrix of a texture triangle.
///
/// `(d0, d1, d2)` are the tri's vertex params (a direction, NOT vertex
/// indices for kinds 1-3), `angle` is the rotation byte (Y-spin,
/// `cos/sin(angle * 0.024543693)`), `(s0, s1, s2)` are the kind-dependent
/// row scales (see [`build_proc_table`]). `f32` trig (the live path computes
/// in double then narrows — an ulp-level deviation, documented).
fn proc_matrix(d0: i32, d1: i32, d2: i32, angle: u8, s0: f32, s1: f32, s2: f32) -> [f32; 9] {
    let theta = f32::from(angle) * 0.024543693;
    let (sin, cos) = theta.sin_cos();
    // Y-rotation by the spin.
    let rot = [
        cos, 0.0, sin, //
        0.0, 1.0, 0.0, //
        -sin, 0.0, cos,
    ];
    let f1 = d1 as f32 / 32767.0;
    let len = (d0 as f32 * d0 as f32 + d2 as f32 * d2 as f32).sqrt();
    let mut mat = [0.0_f32; 9];
    if len == 0.0 && f1 == 0.0 {
        mat = rot;
    } else {
        let (r13, r14) = if len != 0.0 {
            (-(d2 as f32) / len, d0 as f32 / len)
        } else {
            (1.0, 0.0)
        };
        let tilt = -(1.0 - f1 * f1).sqrt();
        let e = 1.0 - f1;
        // Alignment rotation.
        let align = [
            r13 * r13 * e + f1,
            r14 * tilt,
            r13 * r14 * e,
            -r14 * tilt,
            f1,
            r13 * tilt,
            r13 * r14 * e,
            -r13 * tilt,
            r14 * r14 * e + f1,
        ];
        // Spin times alignment, in the client's exact index order.
        for row in 0..3 {
            for col in 0..3 {
                mat[row * 3 + col] = rot[row * 3 + 2] * align[6 + col]
                    + rot[row * 3] * align[col]
                    + rot[row * 3 + 1] * align[3 + col];
            }
        }
    }
    // Row scales.
    for col in 0..3 {
        mat[col] *= s0;
        mat[3 + col] *= s1;
        mat[6 + col] *= s2;
    }
    mat
}

/// Plane selector: dominant axis of the rotated
/// face normal, 0-5.
fn proc_plane(nx: f32, ny: f32, nz: f32) -> i32 {
    let (ax, ay, az) = (nx.abs(), ny.abs(), nz.abs());
    if ay > ax && ay > az {
        if ny > 0.0 {
            0
        } else {
            1
        }
    } else if az > ax && az > ay {
        if nz > 0.0 {
            2
        } else {
            3
        }
    } else if nx > 0.0 {
        4
    } else {
        5
    }
}

/// Cylindrical projection of one corner.
fn proc_cylindrical(
    point: [f32; 3],
    centre: [f32; 3],
    mat: &[f32; 9],
    cyl_scale: f32,
    dir: i8,
    speed: f32,
) -> [f32; 2] {
    let dx = point[0] - centre[0];
    let dy = point[1] - centre[1];
    let dz = point[2] - centre[2];
    let tx = mat[2] * dz + mat[0] * dx + mat[1] * dy;
    let ty = mat[5] * dz + mat[3] * dx + mat[4] * dy;
    let tz = mat[8] * dz + mat[6] * dx + mat[7] * dy;
    let mut u = tx.atan2(tz) / 6.2831855 + 0.5;
    if cyl_scale != 1.0 {
        u *= cyl_scale;
    }
    let v = ty + 0.5 + speed;
    // Direction swizzle: 1 -> (-v, u), 2 -> (-u, -v),
    // 3 -> (v, -u).
    if dir == 1 {
        return [-v, u];
    } else if dir == 2 {
        return [-u, -v];
    } else if dir == 3 {
        return [v, -u];
    }
    [u, v]
}

/// Planar projection of one corner.
fn proc_planar(
    point: [f32; 3],
    centre: [f32; 3],
    plane: i32,
    mat: &[f32; 9],
    dir: i8,
    speed: f32,
    [trans_u, trans_v]: [f32; 2],
) -> [f32; 2] {
    let dx = point[0] - centre[0];
    let dy = point[1] - centre[1];
    let dz = point[2] - centre[2];
    let tx = mat[2] * dz + mat[0] * dx + mat[1] * dy;
    let ty = mat[5] * dz + mat[3] * dx + mat[4] * dy;
    let tz = mat[8] * dz + mat[6] * dx + mat[7] * dy;
    let (mut u, mut v) = match plane {
        0 => (speed + tx + 0.5, -tz + trans_v + 0.5),
        1 => (speed + tx + 0.5, trans_v + tz + 0.5),
        2 => (-tx + speed + 0.5, -ty + trans_u + 0.5),
        3 => (speed + tx + 0.5, -ty + trans_u + 0.5),
        4 => (trans_v + tz + 0.5, -ty + trans_u + 0.5),
        _ => (-tz + trans_v + 0.5, -ty + trans_u + 0.5),
    };
    if dir == 1 {
        (u, v) = (-v, u);
    } else if dir == 2 {
        (u, v) = (-u, -v);
    } else if dir == 3 {
        (u, v) = (v, -u);
    }
    [u, v]
}

/// Spherical projection of one corner.
fn proc_spherical(
    point: [f32; 3],
    centre: [f32; 3],
    mat: &[f32; 9],
    dir: i8,
    speed: f32,
) -> [f32; 2] {
    let dx = point[0] - centre[0];
    let dy = point[1] - centre[1];
    let dz = point[2] - centre[2];
    let tx = mat[2] * dz + mat[0] * dx + mat[1] * dy;
    let ty = mat[5] * dz + mat[3] * dx + mat[4] * dy;
    let tz = mat[8] * dz + mat[6] * dx + mat[7] * dy;
    let r = (tz * tz + tx * tx + ty * ty).sqrt();
    let mut u = tx.atan2(tz) / 6.2831855 + 0.5;
    // The single-precision pi is bit-identical to `f32::consts::PI`.
    let mut v = (ty / r).asin() / std::f32::consts::PI + 0.5 + speed;
    if dir == 1 {
        (u, v) = (-v, u);
    } else if dir == 2 {
        (u, v) = (-u, -v);
    } else if dir == 3 {
        (u, v) = (v, -u);
    }
    [u, v]
}

/// Precomputed per-model UV context: kinds-1-3 triangles plus the direct-UV
/// prefix bases. Built once per model ([`build_proc_table`]), consulted O(1)
/// per face.
struct ProcTable {
    /// Per-tri procedural payload (`Some` iff kind 1-3, usable scales and a
    /// contributing face).
    tris: Vec<Option<ProcTri>>,
    /// `vertexTextureVertex` prefix bases: `bases[vert]` is the [`TexData::uvs`]
    /// index of the vert's first owned pair.
    bases: Vec<usize>,
}

/// Build the per-tri procedural table for one model (shared by
/// [`TexData::corner_uvs`] and [`TexData::uv_kind_census`] so the census does
/// not rebuild it per face). Entry `t` is `Some` iff tri `t` has kind 1-3
/// with a usable scale payload and a contributing face.
///
/// Linear-time: one pass over faces groups the bounds per tri, one pass over
/// verts builds the direct-UV prefix bases — per-face resolution after this is
/// O(1).
fn build_proc_table(tex: &TexData, raw: &RawModel) -> ProcTable {
    let vscale = if raw.version < 13 { 4.0_f32 } else { 1.0_f32 };
    // Direct-UV prefix bases, one pass over verts.
    let mut bases: Vec<usize> = Vec::with_capacity(raw.verts.len());
    let mut acc = 0_usize;
    for index in 0..raw.verts.len() {
        bases.push(acc);
        acc = acc.saturating_add(tex.vert_counts.get(index).copied().unwrap_or(0) as usize);
    }
    // Bounds per tri, one pass over faces.
    let mut bounds: Vec<Option<([i64; 3], [i64; 3])>> = vec![None; raw.tex_tris.len()];
    if let Some(mappings) = tex.face_mappings.as_deref() {
        for (face_no, face) in raw.faces.iter().enumerate() {
            let mapping = mappings.get(face_no).copied().unwrap_or(-1);
            if mapping < 0 || mapping >= raw.tex_tris.len() as i32 {
                continue;
            }
            let slot = match bounds.get_mut(mapping as usize) {
                Some(slot) => slot,
                None => continue,
            };
            let (mut min, mut max) = slot.unwrap_or(([i64::MAX; 3], [i64::MIN; 3]));
            let mut ok = true;
            for corner in face {
                match raw.verts.get(*corner as usize) {
                    Some(vert) => {
                        for axis in 0..3 {
                            // Wire coords are exact ints in f32 dress; x4 exact.
                            let scaled = (vert[axis] * vscale) as i64;
                            min[axis] = min[axis].min(scaled);
                            max[axis] = max[axis].max(scaled);
                        }
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                *slot = Some((min, max));
            }
        }
    }
    let tris = raw
        .tex_tris
        .iter()
        .enumerate()
        .map(|(index, tri)| {
            if tri.kind < 1 || tri.kind > 3 {
                return None;
            }
            let scales = effective_scales(raw.version, tri)?;
            let centre = tri_centre_from(bounds.get(index).copied().flatten())?;
            let angle = tri.rotation as u8;
            // Kinds-1-3 tri verts are DIRECTION params stored as SIGNED shorts
            // (read as a signed short): the decoder keeps the raw `g2` in
            // `u32` dress, so wrap back (`0xFFFF` == -1, the common "straight
            // down" axis) before the matrix float
            // math — unwrapped values `>= 32768` push `1 - f1*f1` negative and
            // NaN the whole matrix. Kind-0 verts are vertex INDICES (unsigned,
            // rebased on merge) and never flow through here.
            let dir_vec = [
                tri.verts[0] as u16 as i16 as i32,
                tri.verts[1] as u16 as i16 as i32,
                tri.verts[2] as u16 as i16 as i32,
            ];
            let speed = f32::from(tri.speed) / 256.0;
            match tri.kind {
                1 => {
                    // Row scales from the sign of scale X, then 64 / scale Y;
                    // cylinder wrap = scale Z / 1024.
                    let (m0, m2) = if scales[0] == 0 {
                        (1.0, 1.0)
                    } else if scales[0] > 0 {
                        (1.0, scales[0] as f32 / 1024.0)
                    } else {
                        (-scales[0] as f32 / 1024.0, 1.0)
                    };
                    let m1 = 64.0 / scales[1] as f32;
                    let mat = proc_matrix(dir_vec[0], dir_vec[1], dir_vec[2], angle, m0, m1, m2);
                    Some(ProcTri {
                        kind: 1,
                        centre,
                        mat,
                        dir: tri.direction,
                        speed,
                        trans: [0.0, 0.0],
                        cyl_scale: scales[2] as f32 / 1024.0,
                    })
                }
                2 => {
                    // 64 / scale on all axes.
                    let mat = proc_matrix(
                        dir_vec[0],
                        dir_vec[1],
                        dir_vec[2],
                        angle,
                        64.0 / scales[0] as f32,
                        64.0 / scales[1] as f32,
                        64.0 / scales[2] as f32,
                    );
                    let trans = tri.trans.unwrap_or([0, 0]);
                    Some(ProcTri {
                        kind: 2,
                        centre,
                        mat,
                        dir: tri.direction,
                        speed,
                        trans: [f32::from(trans[0]) / 256.0, f32::from(trans[1]) / 256.0],
                        cyl_scale: 1.0,
                    })
                }
                _ => {
                    // Scale / 1024 on all axes.
                    let mat = proc_matrix(
                        dir_vec[0],
                        dir_vec[1],
                        dir_vec[2],
                        angle,
                        scales[0] as f32 / 1024.0,
                        scales[1] as f32 / 1024.0,
                        scales[2] as f32 / 1024.0,
                    );
                    Some(ProcTri {
                        kind: 3,
                        centre,
                        mat,
                        dir: tri.direction,
                        speed,
                        trans: [0.0, 0.0],
                        cyl_scale: 1.0,
                    })
                }
            }
        })
        .collect();
    ProcTable { tris, bases }
}

/// [`TexData::corner_uvs`] body over a prebuilt table (see [`build_proc_table`]).
fn corner_uvs_with(
    tex: &TexData,
    raw: &RawModel,
    face: usize,
    table: &ProcTable,
) -> Option<[[f32; 2]; 3]> {
    let corners = *raw.faces.get(face)?;
    if raw.materials.get(face).is_some_and(|mat| *mat >= 0) {
        // Fall through to resolution below.
    } else {
        return None;
    }
    let mapping = tex.face_mappings.as_deref()?.get(face).copied()?;
    let vscale = if raw.version < 13 { 4.0_f32 } else { 1.0_f32 };
    let out = match mapping {
        // Direct UVs at prefix bases + per-face offsets.
        32766 => {
            let offsets = tex
                .face_tex_offsets
                .get(face)
                .copied()
                .flatten()
                .unwrap_or([0, 0, 0]);
            let mut resolved = [[0.0_f32; 2]; 3];
            for (corner_no, slot) in resolved.iter_mut().enumerate() {
                let vert_usize = corners.get(corner_no).copied()? as usize;
                let base = table.bases.get(vert_usize).copied()?;
                let at = base.saturating_add(offsets.get(corner_no).copied()? as usize);
                let pair = tex.uvs.get(at).copied()?;
                *slot = [pair[0] as f32 * TEX_UV_SCALE, pair[1] as f32 * TEX_UV_SCALE];
            }
            resolved
        }
        // Default triangle.
        -1 => DEFAULT_UV_TRI,
        t if t >= 0 => {
            let tri = raw.tex_tris.get(t as usize)?;
            match tri.kind {
                // Barycentric projection.
                0 => {
                    let q = [
                        raw.verts.get(tri.verts[0] as usize)?,
                        raw.verts.get(tri.verts[1] as usize)?,
                        raw.verts.get(tri.verts[2] as usize)?,
                    ];
                    let p = [
                        raw.verts.get(corners[0] as usize)?,
                        raw.verts.get(corners[1] as usize)?,
                        raw.verts.get(corners[2] as usize)?,
                    ];
                    // Version-scaled wire coords (uniform x4 — homogeneous, but
                    // exact: upload-time coords ARE scaled).
                    let q0 = [q[0][0] * vscale, q[0][1] * vscale, q[0][2] * vscale];
                    let qu = [
                        q[1][0] * vscale - q0[0],
                        q[1][1] * vscale - q0[1],
                        q[1][2] * vscale - q0[2],
                    ];
                    let qv = [
                        q[2][0] * vscale - q0[0],
                        q[2][1] * vscale - q0[1],
                        q[2][2] * vscale - q0[2],
                    ];
                    // Cofactor rows + reciprocal determinants.
                    let n = [
                        qu[1] * qv[2] - qu[2] * qv[1],
                        qu[2] * qv[0] - qu[0] * qv[2],
                        qu[0] * qv[1] - qu[1] * qv[0],
                    ];
                    let row_u = [
                        qv[1] * n[2] - qv[2] * n[1],
                        qv[2] * n[0] - qv[0] * n[2],
                        qv[0] * n[1] - qv[1] * n[0],
                    ];
                    let det_u = qu[2] * row_u[2] + qu[0] * row_u[0] + qu[1] * row_u[1];
                    let row_v = [
                        qu[1] * n[2] - qu[2] * n[1],
                        qu[2] * n[0] - qu[0] * n[2],
                        qu[0] * n[1] - qu[1] * n[0],
                    ];
                    // The mapping is literal: n = qu x qv; row_u = qv x n,
                    // det_u = qu . row_u; row_v = qu x n, det_v = qv . row_v.
                    let det_v = qv[2] * row_v[2] + qv[0] * row_v[0] + qv[1] * row_v[1];
                    let inv_u = 1.0 / det_u;
                    let inv_v = 1.0 / det_v;
                    let mut resolved = [[0.0_f32; 2]; 3];
                    for corner_no in 0..3 {
                        let d = [
                            p[corner_no][0] * vscale - q0[0],
                            p[corner_no][1] * vscale - q0[1],
                            p[corner_no][2] * vscale - q0[2],
                        ];
                        resolved[corner_no] = [
                            (d[2] * row_u[2] + d[0] * row_u[0] + d[1] * row_u[1]) * inv_u,
                            (d[2] * row_v[2] + d[0] * row_v[0] + d[1] * row_v[1]) * inv_v,
                        ];
                    }
                    resolved
                }
                1..=3 => {
                    let proc = table.tris.get(t as usize)?.as_ref()?;
                    let p = [
                        raw.verts.get(corners[0] as usize)?,
                        raw.verts.get(corners[1] as usize)?,
                        raw.verts.get(corners[2] as usize)?,
                    ];
                    let scaled = [
                        [p[0][0] * vscale, p[0][1] * vscale, p[0][2] * vscale],
                        [p[1][0] * vscale, p[1][1] * vscale, p[1][2] * vscale],
                        [p[2][0] * vscale, p[2][1] * vscale, p[2][2] * vscale],
                    ];
                    match proc.kind {
                        1 => {
                            let mut uvs = [[0.0_f32; 2]; 3];
                            for (slot, v) in uvs.iter_mut().zip(scaled.iter()) {
                                *slot = proc_cylindrical(
                                    *v,
                                    proc.centre,
                                    &proc.mat,
                                    proc.cyl_scale,
                                    proc.dir,
                                    proc.speed,
                                );
                            }
                            // Seam fixups: U-branch when
                            // `dir & 1 == 0`, V-branch otherwise.
                            let wrap = proc.cyl_scale / 2.0;
                            if proc.dir & 0x1 == 0 {
                                if uvs[1][0] - uvs[0][0] > wrap {
                                    uvs[1][0] -= proc.cyl_scale;
                                } else if uvs[0][0] - uvs[1][0] > wrap {
                                    uvs[1][0] += proc.cyl_scale;
                                }
                                if uvs[2][0] - uvs[0][0] > wrap {
                                    uvs[2][0] -= proc.cyl_scale;
                                } else if uvs[0][0] - uvs[2][0] > wrap {
                                    uvs[2][0] += proc.cyl_scale;
                                }
                            } else {
                                if uvs[1][1] - uvs[0][1] > wrap {
                                    uvs[1][1] -= proc.cyl_scale;
                                } else if uvs[0][1] - uvs[1][1] > wrap {
                                    uvs[1][1] += proc.cyl_scale;
                                }
                                if uvs[2][1] - uvs[0][1] > wrap {
                                    uvs[2][1] -= proc.cyl_scale;
                                } else if uvs[0][1] - uvs[2][1] > wrap {
                                    uvs[2][1] += proc.cyl_scale;
                                }
                            }
                            uvs
                        }
                        2 => {
                            // Face normal, rotated and
                            // normalized by 64/scale for the plane pick.
                            let e1 = [
                                scaled[1][0] - scaled[0][0],
                                scaled[1][1] - scaled[0][1],
                                scaled[1][2] - scaled[0][2],
                            ];
                            let e2 = [
                                scaled[2][0] - scaled[0][0],
                                scaled[2][1] - scaled[0][1],
                                scaled[2][2] - scaled[0][2],
                            ];
                            let n = [
                                e1[1] * e2[2] - e1[2] * e2[1],
                                e1[2] * e2[0] - e1[0] * e2[2],
                                e1[0] * e2[1] - e1[1] * e2[0],
                            ];
                            // The plane pick rotates the face normal
                            // by the matrix, then divides each row back by its
                            // 64/scale factor. The table rows carry that factor,
                            // so multiply by its reciprocal (ulp-level order
                            // deviation from a single division, documented).
                            let scales =
                                effective_scales(raw.version, raw.tex_tris.get(t as usize)?)?;
                            let inv = [
                                scales[0] as f32 / 64.0,
                                scales[1] as f32 / 64.0,
                                scales[2] as f32 / 64.0,
                            ];
                            let rn = [
                                (proc.mat[2] * n[2] + proc.mat[0] * n[0] + proc.mat[1] * n[1])
                                    * inv[0],
                                (proc.mat[5] * n[2] + proc.mat[3] * n[0] + proc.mat[4] * n[1])
                                    * inv[1],
                                (proc.mat[8] * n[2] + proc.mat[6] * n[0] + proc.mat[7] * n[1])
                                    * inv[2],
                            ];
                            let plane = proc_plane(rn[0], rn[1], rn[2]);
                            let mut uvs = [[0.0_f32; 2]; 3];
                            for (slot, v) in uvs.iter_mut().zip(scaled.iter()) {
                                *slot = proc_planar(
                                    *v,
                                    proc.centre,
                                    plane,
                                    &proc.mat,
                                    proc.dir,
                                    proc.speed,
                                    proc.trans,
                                );
                            }
                            uvs
                        }
                        _ => {
                            let mut uvs = [[0.0_f32; 2]; 3];
                            for (slot, v) in uvs.iter_mut().zip(scaled.iter()) {
                                *slot = proc_spherical(
                                    *v,
                                    proc.centre,
                                    &proc.mat,
                                    proc.dir,
                                    proc.speed,
                                );
                            }
                            // Seam fixups (+-1.0).
                            if proc.dir & 0x1 == 0 {
                                if uvs[1][0] - uvs[0][0] > 0.5 {
                                    uvs[1][0] -= 1.0;
                                } else if uvs[0][0] - uvs[1][0] > 0.5 {
                                    uvs[1][0] += 1.0;
                                }
                                if uvs[2][0] - uvs[0][0] > 0.5 {
                                    uvs[2][0] -= 1.0;
                                } else if uvs[0][0] - uvs[2][0] > 0.5 {
                                    uvs[2][0] += 1.0;
                                }
                            } else {
                                if uvs[1][1] - uvs[0][1] > 0.5 {
                                    uvs[1][1] -= 1.0;
                                } else if uvs[0][1] - uvs[1][1] > 0.5 {
                                    uvs[1][1] += 1.0;
                                }
                                if uvs[2][1] - uvs[0][1] > 0.5 {
                                    uvs[2][1] -= 1.0;
                                } else if uvs[0][1] - uvs[2][1] > 0.5 {
                                    uvs[2][1] += 1.0;
                                }
                            }
                            uvs
                        }
                    }
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    // Degenerate-input guard: the live path would upload garbage (1/0,
    // asin(>1));
    // report fallback instead so the caller averages. Never panics.
    if out.iter().flatten().all(|c| c.is_finite()) {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
