//! The draw-distance levels and what they control (design §1.2):
//!
//! - the distances 16,384 / 32,768 / 49,152 / 65,536 / 65,536 fine units
//!   ([`DISTANCES`]; the classic reach was 14,844); level 2 is the default;
//! - the ring radius `max(1, ceil(dist / 32768))` map squares: 1, 1, 2, 2, 2;
//! - the far plane `dist + 1024` from the focus, plus the eye's distance
//!   from the focus ([`FarLevel::far_plane_from`]);
//! - the fog, which ends at the far plane; its start reference
//!   [`FOG_REFERENCE`] stays 14,844;
//! - the distance class of a map square ([`distance_class`]) and of a loc
//!   container ([`container_class`]), which gate the squares' locs and the
//!   containers' loc categories;
//! - model levels of detail ([`model_lod`]).

/// The draw distance of each level, fine units (512 per tile; module docs).
pub const DISTANCES: [i32; 5] = [16_384, 32_768, 49_152, 65_536, 65_536];

/// The distances in use ([`DISTANCES`]).
#[must_use]
pub fn distances() -> &'static [i32; 5] {
    &DISTANCES
}

/// The default level.
pub const DEFAULT_LEVEL: u8 = 2;

/// The far plane's margin over the draw distance.
pub const FAR_PLANE_MARGIN: i32 = 1024;

/// The view distance the fog start is scaled by (`start = end - end * (4
/// fogDepth + 1024) / 14844`): the classic far plane at a 104-tile window.
pub const FOG_REFERENCE: f32 = 14_844.0;

/// One draw-distance level (0..=4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FarLevel(u8);

impl FarLevel {
    /// Level `level`, `None` above 4.
    #[must_use]
    pub fn new(level: u8) -> Option<Self> {
        (usize::from(level) < DISTANCES.len()).then_some(Self(level))
    }

    /// The default level, 2.
    pub const DEFAULT: Self = Self(DEFAULT_LEVEL);

    #[must_use]
    pub fn index(self) -> u8 {
        self.0
    }

    /// The draw distance, fine units ([`distances`]).
    #[must_use]
    pub fn distance(self) -> i32 {
        distances()[usize::from(self.0)]
    }

    /// The far plane, `distance + 1024`.
    #[must_use]
    pub fn far_plane(self) -> i32 {
        self.distance() + FAR_PLANE_MARGIN
    }

    /// The far plane seen from an eye `reach` fine units (horizontally) from
    /// the focus: [`Self::far_plane`] plus `reach`. The ring and the
    /// container classes are measured from the focus, so the land and the
    /// locs then reach the fog's end (which is this plane) together.
    #[must_use]
    pub fn far_plane_from(self, reach: i32) -> i32 {
        self.far_plane() + reach.max(0)
    }

    /// The ring radius in map squares around the focus square:
    /// `max(1, ceil(distance / 32768))`: 3 x 3
    /// squares at levels 0-1, 5 x 5 at 2-4.
    #[must_use]
    pub fn ring_radius(self) -> i32 {
        let unit = crate::far_ring::SQUARE_UNITS;
        ((self.distance() + unit - 1) / unit).max(1)
    }
}

/// The distance class of a map square `distance` fine units from the
/// focus (0 inside its box, else the 2D distance to the box's nearest
/// point): the first level whose band exceeds it (strictly), 5 beyond every
/// band. A square's locs are built only while its class is below 5 and at most the level.
#[must_use]
pub fn distance_class(distance: i32) -> u8 {
    distance_class_in(distances(), distance)
}

/// [`distance_class`] over the level distances `table`.
#[must_use]
pub fn distance_class_in(table: &[i32; 5], distance: i32) -> u8 {
    table
        .iter()
        .position(|&d| distance < d)
        .map_or(table.len() as u8, |c| c as u8)
}

/// A loc container's class at `level`: the first band the container's 2D
/// distance is below, while that band is within the level; `None` beyond
/// (class 6: the container is not drawn).
#[must_use]
pub fn container_class(distance: i32, level: FarLevel) -> Option<u8> {
    let c = distance_class(distance);
    (c <= level.index() && usize::from(c) < DISTANCES.len()).then_some(c)
}

/// Whether a loc of `category` draws in a container of `class`: a
/// container's category group is built while the category is at least the
/// class (over the categories `4, 3, 2, 1, 0`). Class 0 draws every
/// category, 1-2 categories 2 and 4, 3-4 category 4.
#[must_use]
pub fn category_draws(category: u8, class: u8) -> bool {
    category >= class
}

/// A choice that moves with a distance (a container's class, a model's
/// LOD), with hysteresis: `previous` stays while it lies between the choices
/// a margin nearer (`near`) and a margin farther (`far`), else `now`. The
/// choices grow with the distance (`None`, nothing drawn, beyond all).
#[must_use]
pub fn hysteresis(
    previous: Option<u8>,
    near: Option<u8>,
    far: Option<u8>,
    now: Option<u8>,
) -> Option<u8> {
    let rank = |c: Option<u8>| c.map_or(u16::MAX, u16::from);
    match previous {
        Some(_) if rank(near) <= rank(previous) && rank(previous) <= rank(far) => previous,
        _ => now,
    }
}

/// One entry of the model level-of-detail table (its two entries are
/// [`MODEL_LOD`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodConfig {
    /// The view depth of the first distance step.
    pub base: f32,
    /// The factor between distance steps.
    pub step: f32,
    /// The projected size of the first size step.
    pub size: f32,
    /// The divisor between size steps.
    pub divisor: f32,
    /// Added to the step count.
    pub bias: i32,
}

/// The model LOD configs: entry 0 gives the LOD the scene passes draw with;
/// entry 1 gives the LOD of one other pass kind. Which of our passes is
/// which kind is inferred: the scene passes take entry 0.
pub const MODEL_LOD: [LodConfig; 2] = [
    LodConfig {
        base: 10_000.0,
        step: 1.32,
        size: 0.032,
        divisor: 1.538,
        bias: -2,
    },
    LodConfig {
        base: 23_000.0,
        step: 1.0,
        size: 0.061,
        divisor: 1.58,
        bias: 2,
    },
];

/// A model's level of detail, 0 to 4: `depth` is the smallest view depth over its
/// bounding box, `radius` the length of its box's half extent, and `size`
/// its projected size ([`projected_size`], only asked for when the camera
/// is outside the radius).
///
/// Distance steps: 0 up to `base`, else the first `k` in 1..=10 whose
/// `base` times `step` to the `k` reaches `depth`. Size steps: 0 at or
/// above `size`, else `k + 1` for the first `k` in 1..=8 with the size at
/// or above `size` over `divisor` to the `k`, else 10. The level is the
/// larger count plus the bias (the distance count alone inside the radius),
/// clamped to 0..=4.
#[must_use]
pub fn model_lod(cfg: &LodConfig, depth: f32, radius: f32, size: impl FnOnce() -> f32) -> u8 {
    let mut n = 0_i32;
    if depth > cfg.base {
        let mut m = 1.0_f32;
        loop {
            n += 1;
            m *= cfg.step;
            if m * cfg.base >= depth || n == 10 {
                break;
            }
        }
    }
    if depth > radius {
        let s = size();
        let steps = if s >= cfg.size {
            0
        } else {
            let mut threshold = cfg.size;
            let mut k = 1;
            loop {
                threshold /= cfg.divisor;
                if s >= threshold {
                    break k + 1;
                }
                if k == 8 {
                    break 10;
                }
                k += 1;
            }
        };
        n = n.max(steps);
    }
    (n + cfg.bias).clamp(0, 4) as u8
}

/// The projected size of a sphere: the view-space centre
/// `centre` (depth along +z) and the same point `radius` deeper, both
/// through `projection` (column-major, `w` from row 3) and divided by the
/// deeper point's `w`; the length of their difference.
#[must_use]
pub fn projected_size(projection: &[f32; 16], centre: [f32; 3], radius: f32) -> f32 {
    let p = |v: [f32; 3]| -> [f32; 4] {
        std::array::from_fn(|r| {
            projection[r] * v[0]
                + projection[4 + r] * v[1]
                + projection[8 + r] * v[2]
                + projection[12 + r]
        })
    };
    let a = p(centre);
    let b = p([centre[0], centre[1], centre[2] + radius]);
    let w = b[3];
    if w.abs() < f32::EPSILON {
        return f32::MAX;
    }
    let d = [(b[0] - a[0]) / w, (b[1] - a[1]) / w, (b[2] - a[2]) / w];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The distance fog at far plane `far` (`end = far`, `start = max(0, end -
/// end * (4 fogDepth + 1024) / 14844)`) for the environment whose classic fog
/// range is `classic` (`end - start` is the classic fog depth
/// `(fogDepth + 256) << 2`, the same `4 fogDepth + 1024`). At the classic
/// far plane (14,844) it is the classic range. `None` (fog off) stays off.
#[must_use]
pub fn fog_range(far: f32, classic: Option<(f32, f32)>) -> Option<(f32, f32)> {
    let (start, end) = classic?;
    if end <= start {
        return None;
    }
    let depth = end - start;
    Some(((far - far * depth / FOG_REFERENCE).max(0.0), far))
}

/// `projection` (row-major entries with GL depth, the
/// form `SceneCamera::projection` returns: a perspective frustum with `w =
/// z`, or an orthographic one) with its far plane moved out to `far`. The
/// near plane and every other entry stay; a projection whose far plane is
/// already at or beyond `far` is returned unchanged (the far plane never
/// cuts the classic near scene).
#[must_use]
pub fn projection_with_far(projection: &[f32; 16], far: f32) -> [f32; 16] {
    let mut m = *projection;
    let (a, b) = (f64::from(m[10]), f64::from(m[14]));
    let far = f64::from(far);
    if m[11] != 0.0 && m[15] == 0.0 {
        // Perspective: m10 = (n + f) / (f - n), m14 = -2 f n / (f - n).
        let (near, old) = (-b / (a + 1.0), -b / (a - 1.0));
        if !(near > 0.0 && old > near) || old >= far {
            return m;
        }
        m[10] = ((near + far) / (far - near)) as f32;
        m[14] = (-2.0 * far * near / (far - near)) as f32;
    } else if m[11] == 0.0 && m[15] == 1.0 && a != 0.0 {
        // Orthographic: m10 = 2 / (f - n), m14 = -(f + n) / (f - n).
        let span = 2.0 / a;
        let sum = -b * span;
        let (near, old) = ((sum - span) / 2.0, (sum + span) / 2.0);
        if old >= far {
            return m;
        }
        m[10] = (2.0 / (far - near)) as f32;
        m[14] = (-(far + near) / (far - near)) as f32;
    }
    m
}
