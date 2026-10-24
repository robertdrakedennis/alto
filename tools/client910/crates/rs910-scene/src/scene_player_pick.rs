//! Screen-space picking of players, NPCs, locs and objects: draw-time capsules and model boxes
//! tested against the mouse.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerPickId {
    pub pid: i32,
    pub generation: u32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenBounds {
    pub a: [i32; 2],
    pub b: [i32; 2],
    pub radius: i32,
    pub enabled: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelBounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub struct PickablePlayer {
    pub id: PlayerPickId,
    pub bounds: ModelBounds,
    pub screen_bounds: Option<ScreenBounds>,
    pub projected_depth: i32,
    pub matrix: [f32; 16],
    pub active: bool,
}

fn projected(m: &[f32; 16], p: [i32; 3], v: [f32; 4]) -> Option<[f32; 3]> {
    let (x, y, z) = (p[0] as f32, p[1] as f32, p[2] as f32);
    let tx = m[8] * z + m[0] * x + m[4] * y + m[12];
    let ty = m[9] * z + m[1] * x + m[5] * y + m[13];
    let tz = m[10] * z + m[2] * x + m[6] * y + m[14];
    let w = m[11] * z + m[3] * x + m[7] * y + m[15];
    if !w.is_finite() || tz < -w {
        return None;
    }
    Some([v[0] + v[2] * tx / w, v[1] + v[3] * ty / w, tz / w])
}
fn clip(m: &[f32; 16], p: [i32; 3]) -> Option<[f32; 4]> {
    let (x, y, z) = (p[0] as f32, p[1] as f32, p[2] as f32);
    let a = m[8] * z + m[0] * x + m[4] * y + m[12];
    let b = m[9] * z + m[1] * x + m[5] * y + m[13];
    let c = m[10] * z + m[2] * x + m[6] * y + m[14];
    let d = m[11] * z + m[3] * x + m[7] * y + m[15];
    if !d.is_finite() {
        None
    } else {
        Some([a, b, c, d])
    }
}
pub fn capsule_hit(b: ScreenBounds, p: [i32; 2], o: [i32; 2]) -> bool {
    if !b.enabled {
        return false;
    }
    let a = b.a;
    let c = b.b;
    let px = p[0].wrapping_add(o[0]);
    let py = p[1].wrapping_add(o[1]);
    let dx = c[0].wrapping_sub(a[0]);
    let dy = c[1].wrapping_sub(a[1]);
    let l = dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy));
    let r = b.radius.wrapping_mul(b.radius);
    let d = px
        .wrapping_sub(a[0])
        .wrapping_mul(dx)
        .wrapping_add(py.wrapping_sub(a[1]).wrapping_mul(dy));
    if d <= 0 {
        let x = a[0].wrapping_sub(px);
        let y = a[1].wrapping_sub(py);
        return x.wrapping_mul(x).wrapping_add(y.wrapping_mul(y)) < r;
    }
    if d > l {
        let x = c[0].wrapping_sub(px);
        let y = c[1].wrapping_sub(py);
        return x.wrapping_mul(x).wrapping_add(y.wrapping_mul(y)) < r;
    }
    let s = if (d & 0x1f_ff_ff) == d { 10 } else { 5 };
    let t = d.wrapping_shl(s).wrapping_div(l);
    let x = (dx.wrapping_mul(t) >> s)
        .wrapping_add(a[0])
        .wrapping_sub(px);
    let y = (dy.wrapping_mul(t) >> s)
        .wrapping_add(a[1])
        .wrapping_sub(py);
    x.wrapping_mul(x).wrapping_add(y.wrapping_mul(y)) < r
}

/// Reconstructs the eight pick corners of a model box. Each half extent is truncated before
/// the box is rebuilt, which matters for odd bounds.
pub fn pick_bounds(raw: ModelBounds, pick_size_shift: u32) -> ModelBounds {
    let half = |min: i32, max: i32| max.wrapping_sub(min) >> 1;
    let center = |min: i32, h: i32| min.wrapping_add(h);
    let (hx, hy, hz) = (
        half(raw.min[0], raw.max[0]).wrapping_shl(pick_size_shift),
        half(raw.min[1], raw.max[1]).wrapping_shl(pick_size_shift),
        half(raw.min[2], raw.max[2]).wrapping_shl(pick_size_shift),
    );
    let (cx, cy, cz) = (
        center(raw.min[0], half(raw.min[0], raw.max[0])),
        center(raw.min[1], half(raw.min[1], raw.max[1])),
        center(raw.min[2], half(raw.min[2], raw.max[2])),
    );
    ModelBounds {
        min: [
            cx.wrapping_sub(hx),
            cy.wrapping_sub(hy),
            cz.wrapping_sub(hz),
        ],
        max: [
            cx.wrapping_add(hx),
            cy.wrapping_add(hy),
            cz.wrapping_add(hz),
        ],
    }
}

/// Builds the draw-time screen capsule of a drawn model. `draw_mvp` is the actor matrix multiplied by the projection;
/// `projection` is the separate projection matrix used for the horizontal
/// radius. Viewport is `[center_x, center_y, half_width, half_height]`.
pub fn screen_bounds(
    raw: ModelBounds,
    radius: i32,
    draw_mvp: [f32; 16],
    projection: [f32; 16],
    viewport: [f32; 4],
) -> ScreenBounds {
    let cx = (raw.min[0].wrapping_add(raw.max[0])) >> 1;
    let cz = (raw.min[2].wrapping_add(raw.max[2])) >> 1;
    let lower = clip(&draw_mvp, [cx, raw.min[1], cz]);
    let upper = clip(&draw_mvp, [cx, raw.max[1], cz]);
    let mut out = ScreenBounds {
        a: [0; 2],
        b: [0; 2],
        radius: 0,
        enabled: true,
    };
    let project = |p: [f32; 4]| {
        [
            viewport[0] + viewport[2] * p[0] / p[3],
            viewport[1] + viewport[3] * p[1] / p[3],
        ]
    };
    let Some(original_lo) = lower else {
        out.enabled = false;
        return out;
    };
    let Some(original_hi) = upper else {
        out.enabled = false;
        return out;
    };
    let mut lo = original_lo;
    let mut hi = original_hi;
    let lo_behind = lo[2] < -lo[3];
    let hi_behind = hi[2] < -hi[3];
    if lo_behind && hi_behind {
        out.enabled = false;
        return out;
    }
    if lo_behind {
        let t = (lo[2] + lo[3]) / (hi[2] + hi[3]) - 1.0;
        lo = [
            (hi[0] - lo[0]) * t + lo[0],
            (hi[1] - lo[1]) * t + lo[1],
            (hi[2] - lo[2]) * t + lo[2],
            (hi[3] - lo[3]) * t + lo[3],
        ];
    } else if hi_behind {
        let t = (hi[2] + hi[3]) / (lo[2] + lo[3]) - 1.0;
        hi = [
            (lo[0] - hi[0]) * t + hi[0],
            (lo[1] - hi[1]) * t + hi[1],
            (lo[2] - hi[2]) * t + hi[2],
            (lo[3] - hi[3]) * t + hi[3],
        ];
    }
    let pa = project(lo);
    let pb = project(hi);
    out.a = [pa[0] as i32, pa[1] as i32];
    out.b = [pb[0] as i32, pb[1] as i32];
    let choose_lower = lo[2] / lo[3] > hi[2] / hi[3];
    let chosen = if choose_lower {
        original_lo
    } else {
        original_hi
    };
    let rx = projection[0] * radius as f32 + chosen[0] + projection[12];
    let rw = projection[3] * radius as f32 + chosen[3] + projection[15];
    let chosen_screen = if choose_lower { pa[0] } else { pb[0] };
    out.radius = (viewport[2] * rx / rw + (viewport[0] - chosen_screen)) as i32;
    out
}

/// The capsule a loc type's clickbox cuboid contributes instead of the drawn model's.
/// `draw_mvp` is the entity matrix times projection; `projection` is the separate projection
/// matrix and `v` is the viewport `[center_x, center_y, half_width, half_height]`.
pub fn cuboid_screen_bounds(
    c: [f32; 6],
    draw_mvp: [f32; 16],
    projection: [f32; 16],
    v: [f32; 4],
) -> ScreenBounds {
    let [min_x, min_y, min_z, max_x, max_y, max_z] = c;
    let m = draw_mvp;
    let mut out = ScreenBounds {
        a: [0; 2],
        b: [0; 2],
        radius: 0,
        enabled: true,
    };
    let mut clipped = false;
    // The centre column truncates each float sum to int first.
    let x = ((min_x + max_x) as i32 >> 1) as f32;
    let z = ((max_z + min_z) as i32 >> 1) as f32;
    let row = |y: f32| {
        [
            m[8] * z + m[0] * x + m[4] * y + m[12],
            m[9] * z + m[1] * x + m[5] * y + m[13],
            m[10] * z + m[2] * x + m[6] * y + m[14],
            m[11] * z + m[3] * x + m[7] * y + m[15],
        ]
    };
    let [x14, x15, x16, x17] = row(min_y as i32 as f32);
    if x16 >= -x17 {
        out.a = [
            (v[2] * x14 / x17 + v[0]) as i32,
            (v[3] * x15 / x17 + v[1]) as i32,
        ];
    } else {
        clipped = true;
    }
    let [x21, x22, x23, x24] = row(max_y as i32 as f32);
    if x23 >= -x24 {
        out.b = [
            (v[2] * x21 / x24 + v[0]) as i32,
            (v[3] * x22 / x24 + v[1]) as i32,
        ];
    } else {
        clipped = true;
    }
    if clipped {
        if x16 < -x17 && x23 < -x24 {
            out.enabled = false;
        } else if x16 < -x17 {
            let t = (x16 + x17) / (x23 + x24) - 1.0;
            let px = (x21 - x14) * t + x14;
            let py = (x22 - x15) * t + x15;
            let pw = (x24 - x17) * t + x17;
            out.a = [
                (v[2] * px / pw + v[0]) as i32,
                (v[3] * py / pw + v[1]) as i32,
            ];
        } else if x23 < -x24 {
            let t = (x23 + x24) / (x16 + x17) - 1.0;
            let px = (x14 - x21) * t + x21;
            let py = (x15 - x22) * t + x22;
            let pw = (x17 - x24) * t + x24;
            out.b = [
                (v[2] * px / pw + v[0]) as i32,
                (v[3] * py / pw + v[1]) as i32,
            ];
        }
    }
    if !out.enabled {
        return out;
    }
    // Half the XZ diagonal, projected through `projection`.
    let half =
        (f64::from(max_x - min_x).powi(2) + f64::from(max_z - min_z).powi(2)).sqrt() as f32 / 2.0;
    out.radius = if x16 / x17 > x23 / x24 {
        let px = projection[0] * half + x14 + projection[12];
        let pw = projection[3] * half + x17 + projection[15];
        (v[2] * px / pw + (v[0] - out.a[0] as f32)) as i32
    } else {
        let px = projection[0] * half + x21 + projection[12];
        let pw = projection[3] * half + x24 + projection[15];
        (v[2] * px / pw + (v[0] - out.b[0] as f32)) as i32
    };
    out
}

/// Projected screen rectangle test shared by the cuboid pick and the box half of the model
/// pick: project the corners that pass the near test and compare the mouse against the float
/// bounds.
fn corners_hit(corners: &[[f32; 3]; 8], m: &[f32; 16], v: [f32; 4], mouse: [i32; 2]) -> bool {
    let mut visible = false;
    let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) =
        (f32::MAX, -3.402_823_5e38_f32, f32::MAX, -3.402_823_5e38_f32);
    for &[x, y, z] in corners {
        let tz = m[10] * z + m[2] * x + m[6] * y + m[14];
        let tw = m[11] * z + m[3] * x + m[7] * y + m[15];
        if tz >= -tw {
            let tx = m[8] * z + m[0] * x + m[4] * y + m[12];
            let ty = m[9] * z + m[1] * x + m[5] * y + m[13];
            let sx = v[2] * tx / tw + v[0];
            let sy = v[3] * ty / tw + v[1];
            lo_x = lo_x.min(sx);
            hi_x = hi_x.max(sx);
            lo_y = lo_y.min(sy);
            hi_y = hi_y.max(sy);
            visible = true;
        }
    }
    // The pick position plus the pick extent, with zero pick extents.
    visible
        && (mouse[0] as f32) > lo_x
        && (mouse[0] as f32) < hi_x
        && (mouse[1] as f32) > lo_y
        && (mouse[1] as f32) < hi_y
}

/// Cuboid pick: the clickbox corners through the entity matrix.
pub fn cuboid_pick(c: [f32; 6], mvp: &[f32; 16], v: [f32; 4], mouse: [i32; 2]) -> bool {
    let [x0, y0, z0, x1, y1, z1] = c;
    let corners = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x0, y1, z0],
        [x1, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x0, y1, z1],
        [x1, y1, z1],
    ];
    corners_hit(&corners, mvp, v, mouse)
}

/// Model pick with zero pick extents. The model's half extents are truncated and shifted before the box test; unless
/// `box_only`, every kept face whose three projected vertices survive the
/// near test is then compared as a screen bounding box.
pub fn model_pick(
    model: &mut crate::gpumodel::GpuModel,
    mvp: &[f32; 16],
    v: [f32; 4],
    mouse: [i32; 2],
    box_only: bool,
    shift: u32,
) -> bool {
    let raw = ModelBounds {
        min: [model.min_x(), model.min_y(), model.min_z()],
        max: [model.max_x(), model.max_y(), model.max_z()],
    };
    model_pick_within(model, raw, mvp, v, mouse, box_only, shift)
}

/// [`model_pick`] over a model whose bounds were taken beforehand, so the model
/// can be shared and read without being touched.
pub fn model_pick_within(
    model: &crate::gpumodel::GpuModel,
    raw: ModelBounds,
    mvp: &[f32; 16],
    v: [f32; 4],
    mouse: [i32; 2],
    box_only: bool,
    shift: u32,
) -> bool {
    let b = pick_bounds(raw, shift);
    let (x0, y0, z0) = (b.min[0] as f32, b.min[1] as f32, b.min[2] as f32);
    let (x1, y1, z1) = (b.max[0] as f32, b.max[1] as f32, b.max[2] as f32);
    let corners = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x0, y1, z0],
        [x1, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x0, y1, z1],
        [x1, y1, z1],
    ];
    if !corners_hit(&corners, mvp, v, mouse) {
        return false;
    }
    if box_only {
        return true;
    }
    let m = mvp;
    let unique = model.unique_count.max(0) as usize;
    let mut sx = vec![0i32; unique];
    let mut sy = vec![0i32; unique];
    for vertex in 0..model.vertex_count.max(0) as usize {
        let (x, y, z) = (
            model.vx[vertex] as f32,
            model.vy[vertex] as f32,
            model.vz[vertex] as f32,
        );
        let tz = m[10] * z + m[2] * x + m[6] * y + m[14];
        let tw = m[11] * z + m[3] * x + m[7] * y + m[15];
        let screen = (tz >= -tw).then(|| {
            let tx = m[8] * z + m[0] * x + m[4] * y + m[12];
            let ty = m[9] * z + m[1] * x + m[5] * y + m[13];
            [
                (v[2] * tx / tw + v[0]) as i32,
                (v[3] * ty / tw + v[1]) as i32,
            ]
        });
        let start = model.vertex_offsets[vertex].max(0) as usize;
        let end = model.vertex_offsets[vertex + 1].max(0) as usize;
        for slot in start..end {
            let packed = model.vertex_slots[slot];
            if packed == 0 {
                break;
            }
            let index = ((packed as i32 & 0xffff) - 1) as usize;
            if index >= unique {
                continue;
            }
            match screen {
                Some([x, y]) => {
                    sx[index] = x;
                    sy[index] = y;
                }
                None => sx[index] = -999_999,
            }
        }
    }
    let [mx, my] = mouse;
    for face in 0..model.face_count.max(0) as usize {
        let a = (model.idx1[face] as i32 & 0xffff) as usize;
        let b = (model.idx2[face] as i32 & 0xffff) as usize;
        let c = (model.idx3[face] as i32 & 0xffff) as usize;
        if a >= unique || b >= unique || c >= unique {
            continue;
        }
        if sx[a] == -999_999 || sx[b] == -999_999 || sx[c] == -999_999 {
            continue;
        }
        let (ya, yb, yc) = (sy[a], sy[b], sy[c]);
        let (xa, xb, xc) = (sx[a], sx[b], sx[c]);
        // Bounding-box test of the face's projected vertices against the mouse point.
        if my < ya && my < yb && my < yc {
            continue;
        }
        if my > ya && my > yb && my > yc {
            continue;
        }
        if mx < xa && mx < xb && mx < xc {
            continue;
        }
        if mx <= xa || mx <= xb || mx <= xc {
            return true;
        }
    }
    false
}

/// One hit test over a player/NPC capsule and model box (box-only model test, no shift).
pub fn pick_one(p: &PickablePlayer, v: [f32; 4], cursor: [i32; 2], offset: [i32; 2]) -> bool {
    if v[2] <= 0.0 || v[3] <= 0.0 || !p.active {
        return false;
    }
    if let Some(b) = p.screen_bounds {
        if !capsule_hit(b, cursor, offset) {
            return false;
        }
    }
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    let mut visible = false;
    for x in [p.bounds.min[0], p.bounds.max[0]] {
        for y in [p.bounds.min[1], p.bounds.max[1]] {
            for z in [p.bounds.min[2], p.bounds.max[2]] {
                if let Some(q) = projected(&p.matrix, [x, y, z], v) {
                    visible = true;
                    lo[0] = lo[0].min(q[0]);
                    hi[0] = hi[0].max(q[0]);
                    lo[1] = lo[1].min(q[1]);
                    hi[1] = hi[1].max(q[1]);
                }
            }
        }
    }
    visible
        && (cursor[0] as f32) > lo[0]
        && (cursor[0] as f32) < hi[0]
        && (cursor[1] as f32) > lo[1]
        && (cursor[1] as f32) < hi[1]
}

/// List order: projected depth descending; equal-depth newer entries first.
#[cfg(any(test, feature = "test-hooks"))]
pub fn pick_players(
    ps: &[PickablePlayer],
    v: [f32; 4],
    cursor: [i32; 2],
    offset: [i32; 2],
) -> Vec<PlayerPickId> {
    let mut hits: Vec<_> = ps
        .iter()
        .enumerate()
        .filter(|(_, p)| pick_one(p, v, cursor, offset))
        .map(|(order, p)| (p.projected_depth, order, p.id))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    hits.into_iter().map(|(_, _, id)| id).collect()
}

#[cfg(test)]
mod tests;
