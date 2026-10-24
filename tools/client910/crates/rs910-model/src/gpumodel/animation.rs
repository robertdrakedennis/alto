//! Animation of a lit model. A pose (a classic frame, optionally tweened with
//! the next, or a skeletal keyframe set) becomes a list of [`Transform`]s over
//! skin labels, and [`GpuModel::apply_animation`] applies them to the label
//! groups. Transform order, signed fixed-point rounding, label duplication and
//! partial source masks are part of the result and are kept.
use super::GpuModel;
use crate::anim::{AnimBase, AnimFrame, FrameOp};

#[derive(Clone, Debug)]
pub struct Transform {
    pub kind: u8,
    pub labels: Vec<i32>,
    pub value: [i32; 3],
    pub mask: i32,
    pub angle: i32,
    pub normals: bool,
    /// A skeletal type-zero transform sets the pivot to its value directly.
    pub direct_pivot: bool,
}

impl Transform {
    pub fn new(
        base: &AnimBase,
        skin: usize,
        mut value: [i32; 3],
        angle: i32,
        normals: bool,
        direct_pivot: bool,
    ) -> Self {
        let kind = base.op_types[skin];
        let [x, y, z] = value;
        value = match (angle, kind) {
            (1, 0 | 1) => [z, y, x.wrapping_neg()],
            (1, 3) => [z, y, x],
            (1, 2) => [z.wrapping_neg() & 16383, y, x & 16383],
            (2, 0 | 1) => [x.wrapping_neg(), y, z.wrapping_neg()],
            (2, 2) => [x.wrapping_neg() & 16383, y, z.wrapping_neg() & 16383],
            (3, 0 | 1) => [z.wrapping_neg(), y, x],
            (3, 3) => [z, y, x],
            (3, 2) => [z & 16383, y, x.wrapping_neg() & 16383],
            _ => value,
        };
        Self {
            kind,
            labels: base.skins[skin].clone(),
            value,
            mask: base.priorities[skin] & 65535,
            angle,
            normals,
            direct_pivot,
        }
    }
}

/// A classic pose to sample: `frame` of `base`, tweened `tick / duration` of
/// the way towards `next`.
#[derive(Clone, Copy)]
pub struct ClassicPose<'a> {
    pub base: &'a AnimBase,
    pub frame: &'a AnimFrame,
    pub next: Option<&'a AnimFrame>,
    pub tick: i32,
    pub duration: i32,
}

/// What a pose is applied to: the turn of the model (0 to 3 quarter turns),
/// whether normals follow rotations, an optional blend-group filter
/// (`(mask, selected)`: only ops whose mask entry equals `selected`), and the
/// part mask intersected with every op's source-model mask.
#[derive(Clone, Copy)]
pub struct PoseTarget<'a> {
    pub angle: i32,
    pub normals: bool,
    pub blend: Option<(&'a [bool], bool)>,
    pub part_mask: i32,
}

impl Default for PoseTarget<'_> {
    fn default() -> Self {
        Self {
            angle: 0,
            normals: false,
            blend: None,
            part_mask: 65535,
        }
    }
}

/// The transforms of a classic pose with no blend filter and no part mask,
/// including the implicit pivot insertion and the per-frame tween locks.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the unselected form; production uses classic_transforms_selected"
    )
)]
pub fn classic_transforms(pose: ClassicPose<'_>, angle: i32, normals: bool) -> Vec<Transform> {
    classic_transforms_masked(
        pose,
        PoseTarget {
            angle,
            normals,
            ..PoseTarget::default()
        },
    )
}

/// The transforms of a classic pose: blend-group filtering precedes implicit
/// pivot insertion; the wear-slot mask intersects each base operation's
/// source-model mask. The next frame is used only when it belongs to the same
/// skeleton.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the unselected form; production uses classic_transforms_selected"
    )
)]
pub fn classic_transforms_masked(pose: ClassicPose<'_>, target: PoseTarget<'_>) -> Vec<Transform> {
    classic_transforms_selected(
        ClassicPose {
            next: pose.next.filter(|f| f.base_id == pose.base.id),
            ..pose
        },
        target,
    )
}

/// The transforms of a classic pose after the caller has applied its own
/// check of the next frame's skeleton. Blended secondary frames are sometimes
/// checked against the primary skeleton.
pub fn classic_transforms_selected(
    pose: ClassicPose<'_>,
    target: PoseTarget<'_>,
) -> Vec<Transform> {
    let ClassicPose {
        base,
        frame,
        next,
        tick,
        duration,
    } = pose;
    let PoseTarget {
        angle,
        normals,
        blend,
        part_mask,
    } = target;
    let mut out = Vec::new();
    let mut emit = |skin: usize, v: [i32; 3], pivot: i32| {
        if base.op_types[skin] != 0 && blend.is_some_and(|(mask, selected)| mask[skin] != selected)
        {
            return;
        }

        if pivot >= 0 {
            out.push(Transform::new(
                base,
                pivot as usize,
                [0; 3],
                angle,
                normals,
                false,
            ));
        }
        out.push(Transform::new(base, skin, v, angle, normals, false));
    };
    let next = next.filter(|_| tick != 0);
    if let Some(next) = next {
        let (mut a, mut b) = (0, 0);
        for skin in 0..base.op_types.len() {
            let kind = base.op_types[skin];
            let default = if matches!(kind, 3 | 10) { 128 } else { 0 };
            let empty = FrameOp {
                skin,
                x: default,
                y: default,
                z: default,
                pivot: -1,
                blend: 0,
            };
            let ca = frame.ops.get(a).filter(|v| v.skin == skin);
            let cb = next.ops.get(b).filter(|v| v.skin == skin);
            if ca.is_none() && cb.is_none() {
                continue;
            }
            let va = ca.unwrap_or(&empty);
            let vb = cb.unwrap_or(&empty);
            let mut value = [va.x, va.y, va.z];
            if va.blend & 2 == 0 && vb.blend & 1 == 0 {
                for (i, (start, end)) in [va.x, va.y, va.z]
                    .into_iter()
                    .zip([vb.x, vb.y, vb.z])
                    .enumerate()
                {
                    let wrap = if kind == 2 || (kind == 9 && i == 0) {
                        16384
                    } else if kind == 7 && i == 0 {
                        64
                    } else {
                        0
                    };
                    let mut delta = end.wrapping_sub(start);
                    if wrap != 0 {
                        delta &= wrap - 1;
                        if delta >= wrap / 2 {
                            delta -= wrap;
                        }
                    }
                    value[i] = delta
                        .wrapping_mul(tick)
                        .wrapping_div(duration)
                        .wrapping_add(start);
                    if wrap != 0 {
                        value[i] &= wrap - 1;
                    }
                }
                if kind == 9 {
                    value[1] = 0;
                    value[2] = 0;
                }
            }
            emit(skin, value, if va.pivot >= 0 { va.pivot } else { vb.pivot });
            a += usize::from(ca.is_some());
            b += usize::from(cb.is_some());
        }
    } else {
        for op in &frame.ops {
            emit(op.skin, [op.x, op.y, op.z], op.pivot);
        }
    }
    for op in &mut out {
        op.mask &= part_mask;
    }
    out
}

/// Rotations about Z, X and Y in 14-bit fixed point; the rounding adds 16383,
/// negative values included.
fn rotate(mut p: [i32; 3], a: [i32; 3], odd: bool, short: bool) -> [i32; 3] {
    let order = if odd { [0, 2, 1] } else { [2, 0, 1] };
    for axis in order {
        if a[axis] == 0 {
            continue;
        }
        let s = crate::trig::sin(a[axis]);
        let c = crate::trig::cos(a[axis]);
        let (i, j) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        let x = p[i];
        let y = p[j];
        if axis == 2 {
            p[i] = x
                .wrapping_mul(c)
                .wrapping_add(y.wrapping_mul(s))
                .wrapping_add(16383)
                >> 14;
            p[j] = y
                .wrapping_mul(c)
                .wrapping_sub(x.wrapping_mul(s))
                .wrapping_add(16383)
                >> 14;
        } else {
            p[i] = x
                .wrapping_mul(c)
                .wrapping_sub(y.wrapping_mul(s))
                .wrapping_add(16383)
                >> 14;
            p[j] = x
                .wrapping_mul(s)
                .wrapping_add(y.wrapping_mul(c))
                .wrapping_add(16383)
                >> 14;
        }
        if short {
            p[i] = p[i] as i16 as i32;
            p[j] = p[j] as i16 as i32;
        }
    }
    p
}

impl GpuModel {
    /// Whole-model shadow transforms.
    /// Values are already in the frame's fixed-point space; unlike label
    /// transforms, translation/pivot do not shift by four and scale divides.
    pub fn apply_shadow_animation(&mut self, base: &AnimBase, frame: &AnimFrame) {
        if self.vertex_groups.is_none() {
            return;
        }
        for v in 0..self.vertex_count_all as usize {
            self.vx[v] <<= 4;
            self.vy[v] <<= 4;
            self.vz[v] <<= 4;
        }
        let mut pivot = [0i32; 3];
        let mut ops = Vec::new();
        for op in &frame.ops {
            if base.opaque[op.skin] {
                if op.pivot != -1 {
                    ops.push((0, [0; 3]));
                }
                ops.push((base.op_types[op.skin], [op.x, op.y, op.z]));
            }
        }
        for (kind, a) in ops {
            match kind {
                0 => {
                    pivot = [0; 3];
                    for v in 0..self.vertex_count as usize {
                        pivot[0] = pivot[0].wrapping_add(self.vx[v]);
                        pivot[1] = pivot[1].wrapping_add(self.vy[v]);
                        pivot[2] = pivot[2].wrapping_add(self.vz[v]);
                    }
                    for i in 0..3 {
                        if self.vertex_count > 0 {
                            pivot[i] /= self.vertex_count;
                        }
                        pivot[i] = pivot[i].wrapping_add(a[i]);
                    }
                }
                1 => {
                    for v in 0..self.vertex_count as usize {
                        self.vx[v] = self.vx[v].wrapping_add(a[0]);
                        self.vy[v] = self.vy[v].wrapping_add(a[1]);
                        self.vz[v] = self.vz[v].wrapping_add(a[2]);
                    }
                }
                2 | 3 => {
                    for v in 0..self.vertex_count as usize {
                        let mut p = [
                            self.vx[v].wrapping_sub(pivot[0]),
                            self.vy[v].wrapping_sub(pivot[1]),
                            self.vz[v].wrapping_sub(pivot[2]),
                        ];
                        if kind == 2 {
                            p = rotate(p, a, false, false);
                        } else {
                            for i in 0..3 {
                                p[i] = p[i].wrapping_mul(a[i]) / 128;
                            }
                        }
                        self.vx[v] = p[0].wrapping_add(pivot[0]);
                        self.vy[v] = p[1].wrapping_add(pivot[1]);
                        self.vz[v] = p[2].wrapping_add(pivot[2]);
                    }
                }
                5 => {
                    for alpha in &mut self.face_alpha {
                        *alpha = (*alpha as u8 as i32)
                            .wrapping_add(a[0].wrapping_mul(8))
                            .clamp(0, 255) as i8;
                    }
                    self.refresh_billboard_alphas();
                }
                7 => {
                    for colour in &mut self.face_colour {
                        let c = *colour as u16 as i32;
                        let h = ((c >> 10 & 63).wrapping_add(a[0])) & 63;
                        let s = (c >> 7 & 7).wrapping_add(a[1] / 4).clamp(0, 7);
                        let l = (c & 127).wrapping_add(a[2]).clamp(0, 127);
                        *colour = (h << 10 | s << 7 | l) as i16;
                    }
                    self.refresh_billboard_colours();
                    self.refresh_billboard_palette();
                }
                // Every billboard.
                8..=10 => {
                    if let Some(b) = &mut self.billboards {
                        b.apply(kind, None, a);
                    }
                }
                _ => {}
            }
        }
        for v in 0..self.vertex_count_all as usize {
            self.vx[v] = self.vx[v].wrapping_add(7) >> 4;
            self.vy[v] = self.vy[v].wrapping_add(7) >> 4;
            self.vz[v] = self.vz[v].wrapping_add(7) >> 4;
        }
        self.bounds_valid = false;
    }
    /// Applies label transforms to the model's label groups. Billboard
    /// operations (8, 9, 10) move the billboard states, not the mesh streams.
    pub fn apply_animation(&mut self, ops: &[Transform]) {
        // The face and billboard ops (5, 7, 8-10) run without label vertices
        // too; vertex ops then touch nothing.
        let groups = self.vertex_groups.as_ref();
        for i in 0..self.vertex_count_all as usize {
            self.vx[i] <<= 4;
            self.vy[i] <<= 4;
            self.vz[i] <<= 4;
        }
        let mut pivot = [0i32; 3];
        // One label-vertex list reused across the ops (programme Phase 6:
        // it was collected into a new `Vec` per op).
        let mut vertices: Vec<usize> = Vec::new();
        for op in ops {
            let partial = op.mask != 65535;
            vertices.clear();
            vertices.extend(
                op.labels
                    .iter()
                    .filter_map(|&l| groups?.get(l as usize))
                    .flatten()
                    .copied()
                    .filter(|&v| {
                        !partial
                            || self
                                .vertex_source_models
                                .as_ref()
                                .is_none_or(|m| op.mask & i32::from(m[v]) != 0)
                    }),
            );
            let a = op.value;
            match op.kind {
                0 => {
                    pivot = [0; 3];
                    if !op.direct_pivot {
                        for &v in &vertices {
                            pivot[0] = pivot[0].wrapping_add(self.vx[v]);
                            pivot[1] = pivot[1].wrapping_add(self.vy[v]);
                            pivot[2] = pivot[2].wrapping_add(self.vz[v]);
                        }
                        if !vertices.is_empty() {
                            for v in &mut pivot {
                                *v /= vertices.len() as i32;
                            }
                        }
                    }
                    for i in 0..3 {
                        pivot[i] = pivot[i].wrapping_add(a[i] << 4);
                    }
                }
                1 => {
                    for &v in &vertices {
                        self.vx[v] = self.vx[v].wrapping_add(a[0] << 4);
                        self.vy[v] = self.vy[v].wrapping_add(a[1] << 4);
                        self.vz[v] = self.vz[v].wrapping_add(a[2] << 4);
                    }
                }
                2 | 3 => {
                    for &v in &vertices {
                        let mut p = [
                            self.vx[v].wrapping_sub(pivot[0]),
                            self.vy[v].wrapping_sub(pivot[1]),
                            self.vz[v].wrapping_sub(pivot[2]),
                        ];
                        if op.kind == 2 {
                            p = rotate(p, a, !partial && op.angle & 1 != 0, false);
                        } else {
                            for i in 0..3 {
                                p[i] = p[i].wrapping_mul(a[i]) >> 7;
                            }
                        }
                        self.vx[v] = p[0].wrapping_add(pivot[0]);
                        self.vy[v] = p[1].wrapping_add(pivot[1]);
                        self.vz[v] = p[2].wrapping_add(pivot[2]);
                    }
                    if op.kind == 2 && op.normals {
                        for &v in &vertices {
                            for slot in
                                self.vertex_offsets[v] as usize..self.vertex_offsets[v + 1] as usize
                            {
                                if self.vertex_slots[slot] == 0 {
                                    break;
                                }
                                let n = (self.vertex_slots[slot] as u16) as usize - 1;
                                let p = rotate(
                                    [self.nx[n] as i32, self.ny[n] as i32, self.nz[n] as i32],
                                    a,
                                    false,
                                    true,
                                );
                                self.nx[n] = p[0] as i16;
                                self.ny[n] = p[1] as i16;
                                self.nz[n] = p[2] as i16;
                            }
                        }
                    }
                }
                5 | 7 => {
                    if let Some(faces) = &self.face_groups {
                        // Any non-empty label group refreshes every billboard's
                        // colour.
                        let touched = op
                            .labels
                            .iter()
                            .filter_map(|&l| faces.get(l as usize))
                            .any(|g| !g.is_empty());
                        for &v in op
                            .labels
                            .iter()
                            .filter_map(|&l| faces.get(l as usize))
                            .flatten()
                        {
                            if partial
                                && self
                                    .face_part
                                    .as_ref()
                                    .is_some_and(|m| op.mask & i32::from(m[v]) == 0)
                            {
                                continue;
                            }
                            if op.kind == 5 {
                                self.face_alpha[v] = ((self.face_alpha[v] as u8) as i32)
                                    .wrapping_add(a[0].wrapping_mul(8))
                                    .clamp(0, 255)
                                    as i8;
                            } else {
                                let c = self.face_colour[v] as u16 as i32;
                                self.face_colour[v] = (((c >> 10).wrapping_add(a[0]) & 63) << 10
                                    | ((c >> 7 & 7).wrapping_add(a[1] / 4)).clamp(0, 7) << 7
                                    | (c & 127).wrapping_add(a[2]).clamp(0, 127))
                                    as i16;
                            }
                        }
                        if touched {
                            if let Some(b) = &mut self.billboards {
                                if op.kind == 5 {
                                    let alphas = &self.face_alpha;
                                    b.refresh_alphas(|f| i32::from(alphas[f] as u8));
                                } else {
                                    let rgb = &crate::colour::hsl_tables().rgb;
                                    let colours = &self.face_colour;
                                    b.refresh_colours(|f| rgb[(colours[f] as u16) as usize]);
                                }
                            }
                        }
                        // The billboard palette entries are re-read after every
                        // label op 7, touched or not.
                        if op.kind == 7 {
                            if let Some(b) = &mut self.billboards {
                                let colours = &self.face_colour;
                                b.refresh_palette(|f| i32::from(colours[f] as u16));
                            }
                        }
                    }
                }
                // The billboard label groups.
                8..=10 => {
                    if let Some(b) = &mut self.billboards {
                        b.apply(op.kind, Some(&op.labels), a);
                    }
                }
                _ => {}
            }
        }
        for i in 0..self.vertex_count_all as usize {
            self.vx[i] = self.vx[i].wrapping_add(7) >> 4;
            self.vy[i] = self.vy[i].wrapping_add(7) >> 4;
            self.vz[i] = self.vz[i].wrapping_add(7) >> 4;
        }
        self.bounds_valid = false;
    }
}
