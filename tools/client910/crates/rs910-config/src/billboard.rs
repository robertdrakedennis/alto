//! Billboard types (archive `billboards`, id 29, group 0, file = billboard id)
//! and the toolkit-neutral model billboard table. The GPU model consults a
//! type's `remove_face` (drop the face) and `material` when a model carries
//! billboards.
//!
//! The toolkit-neutral model billboard table lives here too, so the GPU
//! renderers (`billboard_render`, the modern renderer) read the same data:
//!
//! - [`BillboardFace`]: the static half, built once per model.
//! - [`BillboardState`]: the animated half plus the float scales of the
//!   software placement, mutated by recolour/tint/animation ops 5, 7, 8, 9
//!   and 10.
//! - [`ModelBillboards`]: both tables and the label groups that ops 8/9/10
//!   address; it is the GPU model's billboard table.
//! - [`centroid`] + [`place`]: the per-billboard view placement, with
//!   [`quad_matrix`] building the quad matrix; [`QUAD_CORNERS`] is
//!   the unit quad the GPU toolkit uploads.
//! - [`BillboardInstance`]: one billboard resolved against the model's
//!   current vertices (face centroid + both halves), for owners that keep
//!   only an uploaded mesh.

use std::collections::BTreeMap;

use crate::cache::Pack;
use crate::opcode_table::{
    at, decode_record, Entry, Field, Input, Record, Rule, Slot, Table, Unknown,
};

/// Billboard config archive name in the pack.
pub const BILLBOARD_ARCHIVE: &str = "billboards";

/// One billboard type: material, size and sprite draw parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BillboardType {
    /// Material id (`-1` none).
    pub material: i32,
    /// Size (`g2 + 1`).
    pub width: i32,
    pub height: i32,
    /// Sprite blend parameter of the software draw (default 2).
    pub sprite_blend: i32,
    /// Sprite mode parameter of the software draw (default 1).
    pub sprite_mode: i32,
    /// Skipped while bloom is on.
    pub hidden_under_bloom: bool,
    /// The face is removed from the model.
    pub remove_face: bool,
}

impl Default for BillboardType {
    fn default() -> Self {
        Self {
            material: -1,
            width: 64,
            height: 64,
            sprite_blend: 2,
            sprite_mode: 1,
            hidden_under_bloom: false,
            remove_face: false,
        }
    }
}

/// Billboard type opcodes of this revision.
static BILLBOARD_OPCODES: Table<BillboardType, anyhow::Error> = Table::new(
    &[
        Entry::new(
            at(1),
            Rule::Short(|t, _, v| {
                t.material = if v == 65535 { -1 } else { i32::from(v) };
            }),
        ),
        Entry::new(at(2), Rule::Custom(read_size)),
        // Stored but not used.
        Entry::new(at(3), Rule::Skip(&[Field::Byte])),
        Entry::new(at(4), Rule::Byte(|t, _, v| t.sprite_blend = i32::from(v))),
        Entry::new(at(5), Rule::Byte(|t, _, v| t.sprite_mode = i32::from(v))),
        Entry::new(at(6), Rule::Flag(|t, _| t.hidden_under_bloom = true)),
        Entry::new(at(7), Rule::Flag(|t, _| t.remove_face = true)),
    ],
    Unknown::Reject,
);

/// Width and height, each stored as a short minus one.
fn read_size(source: Input<anyhow::Error>, t: &mut BillboardType, _: Slot) -> anyhow::Result<()> {
    t.width = i32::from(source.short()?) + 1;
    t.height = i32::from(source.short()?) + 1;
    Ok(())
}

impl BillboardType {
    /// Decode one billboard type: an opcode stream ended by opcode 0.
    fn decode(id: u32, data: &[u8]) -> anyhow::Result<Self> {
        let record = Record {
            kind: "billboard",
            id: i64::from(id),
        };
        decode_record(
            &BILLBOARD_OPCODES,
            "billboard",
            record,
            data,
            Self::default(),
        )
    }
}

/// All billboard types by id; missing ids read as the default type.
#[derive(Clone, Debug, Default)]
pub struct BillboardStore {
    entries: BTreeMap<u32, BillboardType>,
}

impl BillboardStore {
    /// Load group 0 of the `billboards` archive.
    pub fn load(pack: &Pack) -> anyhow::Result<Self> {
        rs910_core::profile::scope!("load billboards");
        let files = pack.read_group(BILLBOARD_ARCHIVE, 0)?;
        let mut entries = BTreeMap::new();
        for (id, bytes) in files {
            entries.insert(id, BillboardType::decode(id, &bytes)?);
        }
        Ok(Self { entries })
    }

    /// The type of `id`, or the default type when absent.
    #[must_use]
    pub fn get(&self, id: i32) -> BillboardType {
        u32::try_from(id)
            .ok()
            .and_then(|id| self.entries.get(&id).copied())
            .unwrap_or_default()
    }

    /// A store with one extra type (tests).
    #[cfg(any(test, feature = "test-hooks"))]
    #[must_use]
    pub fn with_type(mut self, id: u32, t: BillboardType) -> Self {
        self.entries.insert(id, t);
        self
    }
}

/// The static half of one model billboard. The software toolkit carries the
/// same values keyed by the source face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BillboardFace {
    /// The billboard face's slot among the GPU model's sorted faces
    /// (`face_colour`/`face_alpha` index).
    pub face: usize,
    /// The source model face.
    pub source_face: i32,
    /// The face's three vertices.
    pub vertices: [usize; 3],
    /// Half width (the type width as a short).
    pub width: i32,
    /// Half height (the type height as a short).
    pub height: i32,
    /// Material id as a short (`-1` = no material).
    pub material: i32,
    /// The type's `hidden_under_bloom`: skipped while bloom is on.
    pub bloom_hidden: bool,
    /// The pull towards the eye.
    pub depth_offset: i32,
    /// The type's `sprite_mode` / `sprite_blend` / `remove_face` (unused by
    /// the GPU draw; the software billboard keeps them).
    pub sprite_mode: i32,
    pub sprite_blend: i32,
    pub remove_face: bool,
}

/// The animated half of one billboard. `scale_xf/scale_yf` are the software
/// placement's float scales, which compound op 10 in floats where the GPU
/// model shifts integers. The software placement's colour is `colour`'s alpha
/// with the RGB looked up from `hsl` (the toolkit palette resolves it at draw
/// time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BillboardState {
    /// ARGB (`255 - alpha << 24 | rgb(hsl)`).
    pub colour: i32,
    /// Scale, 128 = 1.
    pub scale_x: i32,
    pub scale_y: i32,
    /// Float scales of the software placement.
    pub scale_xf: f32,
    pub scale_yf: f32,
    /// View-space offset.
    pub offset_x: i32,
    pub offset_y: i32,
    /// Roll about the view axis (`& 0x3FFF`).
    pub rotation: i32,
    /// The software placement's palette index: the face's colour, renormalised
    /// after a recolour/tint/op 7.
    pub hsl: u16,
}

impl BillboardState {
    /// A fresh state: unit scale, no offset or roll.
    #[must_use]
    pub fn new(colour: i32) -> Self {
        Self {
            colour,
            scale_x: 128,
            scale_y: 128,
            scale_xf: 1.0,
            scale_yf: 1.0,
            offset_x: 0,
            offset_y: 0,
            rotation: 0,
            hsl: 0,
        }
    }

    /// Animation op 8: translate.
    pub fn translate(&mut self, x: i32, y: i32) {
        self.offset_x = self.offset_x.wrapping_add(x);
        self.offset_y = self.offset_y.wrapping_add(y);
    }

    /// Animation op 9: rotate.
    pub fn rotate(&mut self, angle: i32) {
        self.rotation = self.rotation.wrapping_add(angle) & 0x3FFF;
    }

    /// Animation op 10: scale, with the software float compounding.
    pub fn scale(&mut self, x: i32, y: i32) {
        self.scale_x = self.scale_x.wrapping_mul(x) >> 7;
        self.scale_y = self.scale_y.wrapping_mul(y) >> 7;
        self.scale_xf = x as f32 * self.scale_xf / 128.0;
        self.scale_yf = y as f32 * self.scale_yf / 128.0;
    }
}

/// The model's billboards.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelBillboards {
    pub faces: Vec<BillboardFace>,
    pub states: Vec<BillboardState>,
    /// Billboard indices per label, kept when the model flags carry `0x400`.
    pub groups: Option<Vec<Vec<usize>>>,
}

impl ModelBillboards {
    /// Refresh the colour of every billboard from its face: `rgb` is the
    /// palette colour of a face.
    pub fn refresh_colours(&mut self, rgb: impl Fn(usize) -> i32) {
        for (face, state) in self.faces.iter().zip(&mut self.states) {
            state.colour =
                (state.colour as u32 & 0xFF00_0000) as i32 | (rgb(face.face) & 0xFF_FFFF);
        }
    }

    /// Refresh the software palette index of every billboard from its face
    /// colour (renormalised): `hsl` is the face's colour.
    pub fn refresh_palette(&mut self, hsl: impl Fn(usize) -> i32) {
        for (face, state) in self.faces.iter().zip(&mut self.states) {
            state.hsl = crate::colour::renormalise_saturation(hsl(face.face) & 0xFFFF) as u16;
        }
    }

    /// Refresh the alpha of every billboard from its face.
    pub fn refresh_alphas(&mut self, alpha: impl Fn(usize) -> i32) {
        for (face, state) in self.faces.iter().zip(&mut self.states) {
            state.colour =
                (state.colour & 0xFF_FFFF) | (255 - (alpha(face.face) & 0xFF)).wrapping_shl(24);
        }
    }

    /// Ops 8/9/10 over the billboards of `labels` (labels past the table are
    /// skipped), or over every billboard when `labels` is `None`.
    pub fn apply(&mut self, kind: u8, labels: Option<&[i32]>, value: [i32; 3]) {
        let mut op = |state: &mut BillboardState| match kind {
            8 => state.translate(value[0], value[1]),
            9 => state.rotate(value[0]),
            10 => state.scale(value[0], value[1]),
            _ => {}
        };
        match labels {
            None => self.states.iter_mut().for_each(&mut op),
            Some(labels) => {
                let Some(groups) = &self.groups else {
                    return;
                };
                for &label in labels {
                    let Some(group) = usize::try_from(label).ok().and_then(|l| groups.get(l))
                    else {
                        continue;
                    };
                    for &index in group {
                        op(&mut self.states[index]);
                    }
                }
            }
        }
    }
}

/// Billboard indices grouped by label; negative labels are ungrouped.
#[must_use]
pub fn label_groups(labels: impl Iterator<Item = i32> + Clone) -> Vec<Vec<usize>> {
    let max = labels.clone().filter(|&l| l >= 0).max().unwrap_or(0);
    let mut out = vec![Vec::new(); max as usize + 1];
    for (index, label) in labels.enumerate() {
        if label >= 0 {
            out[label as usize].push(index);
        }
    }
    out
}

/// One billboard against its model's current vertices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BillboardInstance {
    /// Model-space face centroid ([`centroid`]).
    pub centroid: [f32; 3],
    pub face: BillboardFace,
    pub state: BillboardState,
}

/// Face centroid: `(float) (x1 + x2 + x3) * 0.3333333F` per axis.
#[must_use]
pub fn centroid(vx: &[i32], vy: &[i32], vz: &[i32], face: &BillboardFace) -> [f32; 3] {
    let [a, b, c] = face.vertices;
    let avg = |v: &[i32]| v[a].wrapping_add(v[b]).wrapping_add(v[c]) as f32 * 0.333_333_3_f32;
    [avg(vx), avg(vy), avg(vz)]
}

/// One billboard's view-space placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewPlacement {
    /// The centroid through the model-view matrix (the last component is the
    /// depth the fog reads).
    pub view: [f32; 3],
    /// The unit quad -> view space, as
    /// `Matrix4x3` entries `[00, 01, 02, 10, 11, 12, 20, 21, 22, 30, 31, 32]`
    /// (`actor_matrix::Matrix` layout).
    pub quad: [f32; 12],
}

/// One billboard's placement: `mv` is the model matrix times the camera view
/// as 4x4 matrix entries.
#[must_use]
pub fn place(instance: &BillboardInstance, mv: &[f32; 16]) -> ViewPlacement {
    let [cx, cy, cz] = instance.centroid;
    let e = mv;
    let vx = e[8] * cz + e[0] * cx + e[4] * cy + e[12];
    let vy = e[9] * cz + e[1] * cx + e[5] * cy + e[13];
    let vz = e[10] * cz + e[2] * cx + e[6] * cy + e[14];
    let pull = (1.0_f64 / f64::from(vz * vz + vx * vx + vy * vy).sqrt()) as f32
        * instance.face.depth_offset as f32;
    let s = &instance.state;
    ViewPlacement {
        view: [vx, vy, vz],
        quad: quad_matrix(
            s.rotation,
            instance.face.width.wrapping_mul(s.scale_x) >> 7,
            instance.face.height.wrapping_mul(s.scale_y) >> 7,
            s.offset_x as f32 + vx - vx * pull,
            s.offset_y as f32 + vy - vy * pull,
            vz - vz * pull,
        ),
    }
}

/// Cosine and sine of a 14-bit angle.
fn trig_matrix(angle: i32) -> (f32, f32) {
    let a = f64::from(angle & 0x3FFF) * crate::trig::STEP;
    (a.cos() as f32, a.sin() as f32)
}

/// The unit quad scaled to `2 * half_w` x `2 * half_h`, rolled by
/// `roll` and centred on `(x, y, z)`, as 4x3 matrix entries.
#[must_use]
pub fn quad_matrix(roll: i32, half_w: i32, half_h: i32, x: f32, y: f32, z: f32) -> [f32; 12] {
    if roll == 0 {
        return [
            half_w.wrapping_mul(2) as f32,
            0.0,
            0.0,
            0.0,
            half_h.wrapping_mul(2) as f32,
            0.0,
            0.0,
            0.0,
            1.0,
            x - half_w as f32,
            y - half_h as f32,
            z,
        ];
    }
    let (cos, sin) = trig_matrix(roll);
    [
        cos * 2.0 * half_w as f32,
        sin * 2.0 * half_w as f32,
        0.0,
        sin * -2.0 * half_h as f32,
        cos * 2.0 * half_h as f32,
        0.0,
        0.0,
        0.0,
        1.0,
        (sin * 0.5 - cos * 0.5) * half_w.wrapping_mul(2) as f32 + x,
        (sin * -0.5 - cos * 0.5) * half_h.wrapping_mul(2) as f32 + y,
        z,
    ]
}

/// The four fan corners of the billboard quad (a triangle fan, two
/// primitives), position `(x, y, 0)` with texture coordinate `(x, y)`.
pub const QUAD_CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];

/// The billboard fog uniform: the clamped fog share at view depth `z` for
/// `fog_start/fog_end`.
#[must_use]
pub fn fog_amount(z: f32, fog_start: f32, fog_end: f32) -> f32 {
    (1.0 - (fog_end - z) / (fog_end - fog_start)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(v: &[u32]) -> Vec<f32> {
        v.iter().map(|&b| f32::from_bits(b)).collect()
    }

    fn instance() -> BillboardInstance {
        let face = BillboardFace {
            face: 0,
            source_face: 0,
            vertices: [0, 1, 2],
            width: 64,
            height: 48,
            material: -1,
            bloom_hidden: false,
            depth_offset: -20,
            sprite_mode: 1,
            sprite_blend: 2,
            remove_face: false,
        };
        let mut state = BillboardState::new(-1);
        state.scale_x = 160;
        state.scale_y = 96;
        state.offset_x = 5;
        state.offset_y = -7;
        BillboardInstance {
            centroid: centroid(&[100, 131, 90], &[-400, -377, -410], &[55, 80, 71], &face),
            face,
            state,
        }
    }

    const MV: [f32; 16] = [
        0.8, 0.1, -0.6, 0.0, 0.2, -0.97, 0.05, 0.0, 0.6, 0.05, 0.8, 0.0, 312.5, -40.25, 2100.75,
        1.0,
    ];

    /// Bit patterns of the reference client's placement expressions for the
    /// same inputs.
    #[test]
    fn placement_matches_reference_bits() {
        let mut i = instance();
        assert_eq!(i.centroid[0].to_bits(), 1_121_320_959);
        let p = place(&i, &MV);
        assert_eq!(
            p.view.map(f32::to_bits),
            [1_135_875_414, 1_135_793_930, 1_157_724_979]
        );
        let unrolled = bits(&[
            1_126_170_624,
            0,
            0,
            0,
            1_116_733_440,
            0,
            0,
            0,
            1_065_353_216,
            1_133_528_476,
            1_134_494_804,
            1_157_804_546,
        ]);
        assert_eq!(p.quad.to_vec(), unrolled);
        i.state.rotation = 1234;
        let rolled = bits(&[
            1_125_018_233,
            1_116_854_370,
            0,
            (-1_039_973_697_i32) as u32,
            1_115_696_288,
            0,
            0,
            0,
            1_065_353_216,
            1_135_011_338,
            1_134_086_804,
            1_157_804_546,
        ]);
        assert_eq!(place(&i, &MV).quad.to_vec(), rolled);
    }

    #[test]
    fn animation_ops_follow_gpu_and_software_state() {
        let mut b = ModelBillboards {
            faces: vec![instance().face; 3],
            states: vec![BillboardState::new(0x7F12_3456); 3],
            groups: Some(label_groups([1, -1, 1].into_iter())),
        };
        assert_eq!(b.groups, Some(vec![vec![], vec![0, 2]]));
        b.apply(8, Some(&[1, 7]), [3, -4, 0]);
        b.apply(10, Some(&[1]), [64, 192, 0]);
        b.apply(9, Some(&[1]), [16380, 0, 0]);
        b.apply(9, Some(&[1]), [10, 0, 0]);
        let s = b.states[2];
        assert_eq!(
            (s.offset_x, s.offset_y, s.scale_x, s.scale_y, s.rotation),
            (3, -4, 64, 192, 6)
        );
        assert_eq!((s.scale_xf, s.scale_yf), (0.5, 1.5));
        assert_eq!(
            b.states[1],
            BillboardState::new(0x7F12_3456),
            "ungrouped billboard untouched"
        );
        b.apply(8, None, [1, 1, 0]);
        assert_eq!(b.states[1].offset_x, 1);
        b.refresh_alphas(|_| 0x40);
        assert_eq!(b.states[0].colour as u32, 0xBF12_3456);
        b.refresh_colours(|_| 0x00AB_CDEF);
        assert_eq!(b.states[0].colour as u32, 0xBFAB_CDEF);
        b.refresh_palette(|_| 0x1234);
        assert_eq!(
            b.states[0].hsl,
            crate::colour::renormalise_saturation(0x1234) as u16
        );
        assert_eq!(
            b.states[0].colour as u32, 0xBFAB_CDEF,
            "palette refresh keeps the GPU colour"
        );
    }

    #[test]
    fn fog_amount_is_the_clamped_billboard_fog() {
        assert_eq!(fog_amount(1000.0, 1000.0, 3000.0), 0.0);
        assert_eq!(fog_amount(2000.0, 1000.0, 3000.0), 0.5);
        assert_eq!(fog_amount(9000.0, 1000.0, 3000.0), 1.0);
    }
}
