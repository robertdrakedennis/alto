//! Axis-aligned bounding boxes of the frame's model draws and the clip
//! volume test the water reflection culls with (lane P4-GPU,
//! `frame::gpu::water`).
//!
//! A draw whose box lies wholly outside one plane of a pass's clip volume
//! produces no fragment in that pass, so leaving it out leaves the pass's
//! image unchanged, whatever the draw's blending: the test is conservative
//! (a box that straddles the volume's corner is kept).

use crate::models::mesh::Vertex;

/// A box: its minimum and maximum corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    /// The longest side of the box (fine units).
    #[must_use]
    pub fn extent(&self) -> f32 {
        (0..3)
            .map(|k| self.max[k] - self.min[k])
            .fold(0.0, f32::max)
    }

    /// The box of `vertices` (`None`: none).
    #[must_use]
    pub fn of(vertices: &[Vertex]) -> Option<Self> {
        let (first, rest) = vertices.split_first()?;
        let mut b = Self {
            min: first.pos,
            max: first.pos,
        };
        for v in rest {
            for k in 0..3 {
                b.min[k] = b.min[k].min(v.pos[k]);
                b.max[k] = b.max[k].max(v.pos[k]);
            }
        }
        Some(b)
    }

    fn corners(&self) -> [glam::Vec3; 8] {
        let (a, b) = (self.min, self.max);
        std::array::from_fn(|i| {
            glam::Vec3::new(
                if i & 1 == 0 { a[0] } else { b[0] },
                if i & 2 == 0 { a[1] } else { b[1] },
                if i & 4 == 0 { a[2] } else { b[2] },
            )
        })
    }

    /// The box under `m` (column-major, the instance record's model
    /// matrix): the box of the transformed corners.
    #[must_use]
    pub fn transformed(&self, m: &[f32; 16]) -> Self {
        let m = glam::Mat4::from_cols_array(m);
        let mut out = Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        };
        for c in self.corners() {
            let p = m.transform_point3(c);
            for k in 0..3 {
                out.min[k] = out.min[k].min(p[k]);
                out.max[k] = out.max[k].max(p[k]);
            }
        }
        out
    }

    /// Whether the box lies wholly outside one plane of the clip volume of
    /// `clip` (a wgpu clip space: `-w <= x, y <= w`, `0 <= z <= w`). The
    /// test is on the homogeneous corners, so a box behind the eye or cut
    /// by an oblique near plane is judged as the rasteriser clips it; a
    /// corner within rounding of a plane counts as inside.
    #[must_use]
    pub fn outside(&self, clip: &glam::Mat4) -> bool {
        outside_clip(&self.clip_corners(clip))
    }

    /// The corners under `clip` (bit 0: x, bit 1: y, bit 2: z at the
    /// maximum).
    #[must_use]
    pub fn clip_corners(&self, clip: &glam::Mat4) -> [glam::Vec4; 8] {
        self.corners().map(|c| *clip * c.extend(1.0))
    }
}

/// Whether clip-space `corners` (of a box, [`Bounds::clip_corners`]) lie
/// wholly outside one plane of the clip volume ([`Bounds::outside`]).
#[must_use]
pub fn outside_clip(corners: &[glam::Vec4; 8]) -> bool {
    let mut inside = [false; 6];
    for &p in corners {
        let e = 1e-4 * p.w.abs() + 1e-4;
        inside[0] |= p.x >= -p.w - e;
        inside[1] |= p.x <= p.w + e;
        inside[2] |= p.y >= -p.w - e;
        inside[3] |= p.y <= p.w + e;
        inside[4] |= p.z >= -e;
        inside[5] |= p.z <= p.w + e;
    }
    inside.contains(&false)
}
