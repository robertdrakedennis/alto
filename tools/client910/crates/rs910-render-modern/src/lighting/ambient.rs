//! The per-square ambient irradiance block: the maths ([`Irradiance`], its
//! packing, evaluation, default and flat values, the projection of a
//! six-face capture onto it). The capture scheduler, cache and cross-fade
//! are [`crate::lighting::ambient_schedule`], the GPU half
//! `crate::frame::gpu::ambient`.
//!
//! # The model
//!
//! The lit surfaces take `ambient colour * SH(normal) * SSAO`. `SH` is one
//! second-order irradiance block per map square, seven four-component
//! vectors: the cosine convolution of the square's surroundings divided by
//! pi, so a uniform white surround evaluates to 1.0 for every normal. The
//! block comes from a capture of the world: six cube faces seen from above
//! the square (see [`crate::lighting::ambient_schedule`]), projected onto
//! the second-order real basis with solid-angle weights.
//!
//! # Frames
//!
//! The block lives in a y-up frame ([`sh_direction`]: the renderer's classic
//! axes have y down, so the direction's y is negated); the projection and the
//! evaluation use the same mapping, so the capture's up is the block's up.
//!
//! # Stand-ins
//!
//! Nothing here is a stand-in: every constant is either a basis constant of
//! the real spherical harmonics or a value the reference fixes (the default
//! block, the fade time). The capture's face size, tone mapping and eye
//! offset are labelled where they are chosen ([`FACE_RES`], [`CAPTURE_OFFSET`],
//! [`CAPTURE_EXPOSURE`]).

use crate::lighting::light_probes::{face_direction, texel_weight};

/// The capture face's size in texels (stand-in: the reference's face is at
/// most 256 and its exact size is not known; 128 keeps one face's readback
/// at 128 KiB).
pub const FACE_RES: u32 = 128;
/// The capture camera's near and far planes (classic fine units).
pub const FACE_NEAR: f32 = 512.0;
pub const FACE_FAR: f32 = 65536.0;
/// The capture eye's height above the top of the square's bounds (fine
/// units; upwards is negative y in the classic axes).
pub const EYE_CLEARANCE: f32 = 5000.0;
/// The environment record's capture-offset vector, added to the eye
/// (stand-in: the 910 record's field is not decoded, and the reference's
/// value is normally zero).
pub const CAPTURE_OFFSET: [f32; 3] = [0.0; 3];
/// The environment record's capture exposure, added to the sky colour
/// during a capture: the exposure offset of the sky drawn into each face
/// (`frame::gpu::sky_box`; stand-in: not decoded, normally zero).
pub const CAPTURE_EXPOSURE: f32 = 0.0;
/// A block cross-fades over this long (milliseconds).
pub const FADE_MS: i64 = 750;

/// The real spherical-harmonic basis constants (bands 0 to 2).
const Y0: f64 = 0.282_094_8;
const Y1: f64 = 0.488_602_52;
const Y2_CROSS: f64 = 1.092_548_5;
const Y2_ZZ: f64 = 0.315_391_57;
const Y2_DIFF: f64 = 0.546_274_24;

/// The irradiance convolution's band factors over pi: band 0 one, band 1
/// two thirds, band 2 one quarter.
const BAND1: f64 = 2.0 / 3.0;
const BAND2: f64 = 0.25;

/// Nine second-order coefficients per colour channel, in the order of
/// [`basis`].
pub type Coefficients = [[f32; 9]; 3];

/// One square's irradiance block: seven four-component vectors, the
/// linear terms of red, green and blue, their quadratic terms, and the
/// shared `x^2 - y^2` term (`[6]`, rgb, the last component 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Irradiance(pub [[f32; 4]; 7]);

impl Irradiance {
    /// The capture's own block: every normal evaluates to 1.0 (a flat
    /// ambient with no direction).
    pub const FLAT: Self = Self([
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.0; 4],
        [0.0; 4],
        [0.0; 4],
        [0.0; 4],
    ]);

    /// The block a square shows until its own capture arrives: up is sky
    /// blue, down is dark (the reference's constructor value).
    pub const DEFAULT: Self = Self([
        [-0.030_482_4, 0.145_005_8, 0.006_312_7, 0.367_450_7],
        [-0.047_049_5, 0.226_165_9, 0.002_472_5, 0.538_955_8],
        [-0.019_611_9, 0.381_174_2, 0.017_024_0, 0.704_479_3],
        [-0.008_778_6, 0.003_909_2, 0.023_335_5, -0.010_253_3],
        [-0.013_826_3, -0.003_844_6, 0.046_234_1, -0.006_764_5],
        [0.018_801_7, -0.013_616_4, 0.057_810_7, 0.003_807_7],
        [0.038_138_9, 0.071_135_0, 0.080_667_7, 1.0],
    ]);

    /// The block's light for unit normal `n` (y up): linear, quadratic and
    /// the `x^2 - y^2` term.
    #[must_use]
    pub fn evaluate(&self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        let c = &self.0;
        let n1 = [x, y, z, 1.0];
        let v = [x * y, y * z, z * z, z * x];
        let dot =
            |a: &[f32; 4], b: &[f32; 4]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
        let last = x * x - y * y;
        std::array::from_fn(|ch| dot(&c[ch], &n1) + dot(&c[3 + ch], &v) + c[6][ch] * last)
    }

    /// [`Self::evaluate`] for a normal in the renderer's classic axes.
    #[must_use]
    pub fn evaluate_classic(&self, n: [f32; 3]) -> [f32; 3] {
        self.evaluate(sh_direction(n))
    }

    /// All 28 packed floats interpolated linearly: `self` at `t = 0`, `to`
    /// at `t = 1`.
    #[must_use]
    pub fn lerp(&self, to: &Self, t: f32) -> Self {
        let mut out = *self;
        for (row, (a, b)) in out.0.iter_mut().zip(self.0.iter().zip(&to.0)) {
            for (v, (a, b)) in row.iter_mut().zip(a.iter().zip(b)) {
                *v = a + t * (b - a);
            }
        }
        out
    }

    /// The block of the second-order coefficients `sh` (the irradiance
    /// convolution divided by pi folded in).
    #[must_use]
    pub fn pack(sh: &Coefficients) -> Self {
        let f = |v: f64, c: f32| (v * f64::from(c)) as f32;
        let mut out = [[0.0_f32; 4]; 7];
        for (ch, c) in sh.iter().enumerate() {
            out[ch] = [
                f(BAND1 * Y1, c[1]),
                f(BAND1 * Y1, c[2]),
                f(BAND1 * Y1, c[3]),
                f(Y0, c[0]) - f(BAND2 * Y2_ZZ, c[7]),
            ];
            out[3 + ch] = [
                f(BAND2 * Y2_CROSS, c[4]),
                f(BAND2 * Y2_CROSS, c[5]),
                f(BAND2 * 3.0 * Y2_ZZ, c[7]),
                f(BAND2 * Y2_CROSS, c[6]),
            ];
            out[6][ch] = f(BAND2 * Y2_DIFF, c[8]);
        }
        out[6][3] = 1.0;
        Self(out)
    }

    /// The coefficients a block was packed from (the inverse of
    /// [`Self::pack`]).
    #[must_use]
    pub fn unpack(&self) -> Coefficients {
        let c = &self.0;
        std::array::from_fn(|ch| {
            let (lin, quad) = (c[ch], c[3 + ch]);
            let z2 = quad[2] / (BAND2 * 3.0 * Y2_ZZ) as f32;
            [
                (f64::from(lin[3] + (BAND2 * Y2_ZZ) as f32 * z2) / Y0) as f32,
                (f64::from(lin[0]) / (BAND1 * Y1)) as f32,
                (f64::from(lin[1]) / (BAND1 * Y1)) as f32,
                (f64::from(lin[2]) / (BAND1 * Y1)) as f32,
                (f64::from(quad[0]) / (BAND2 * Y2_CROSS)) as f32,
                (f64::from(quad[1]) / (BAND2 * Y2_CROSS)) as f32,
                (f64::from(quad[3]) / (BAND2 * Y2_CROSS)) as f32,
                z2,
                (f64::from(c[6][ch]) / (BAND2 * Y2_DIFF)) as f32,
            ]
        })
    }
}

/// The block's frame direction of a classic-axes direction: y up.
#[must_use]
pub fn sh_direction([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, -y, z]
}

/// The second-order real basis at unit direction `d` (the block's frame):
/// the constant, the three linear terms, `xy`, `yz`, `zx`, the `3 z^2 - 1`
/// term and `x^2 - y^2`.
#[must_use]
pub fn basis([x, y, z]: [f64; 3]) -> [f64; 9] {
    [
        Y0,
        Y1 * x,
        Y1 * y,
        Y1 * z,
        Y2_CROSS * x * y,
        Y2_CROSS * y * z,
        Y2_CROSS * z * x,
        Y2_ZZ * (3.0 * z * z - 1.0),
        Y2_DIFF * (x * x - y * y),
    ]
}

/// A capture's six faces (the cube order of `crate::lighting::light_probes`,
/// `+x -x +y -y +z -z` in the classic axes) of `size * size` colours each,
/// row-major, every channel in 0..1 (the byte over 256, no gamma decode).
#[derive(Clone, Debug, PartialEq)]
pub struct Faces {
    pub size: usize,
    pub texels: [Vec<[f32; 3]>; 6],
}

impl Faces {
    /// Six faces of one colour.
    #[must_use]
    pub fn uniform(size: usize, colour: [f32; 3]) -> Self {
        Self {
            size,
            texels: std::array::from_fn(|_| vec![colour; size * size]),
        }
    }

    /// The faces' colour at each texel from `colour(direction)`, the
    /// direction a unit vector in the block's frame (y up).
    #[must_use]
    pub fn from_directions(size: usize, colour: impl Fn([f64; 3]) -> [f32; 3]) -> Self {
        let mut faces = Self::uniform(size, [0.0; 3]);
        for (face, texels) in faces.texels.iter_mut().enumerate() {
            for j in 0..size {
                for i in 0..size {
                    texels[j * size + i] = colour(texel_direction(face, i, j, size));
                }
            }
        }
        faces
    }
}

/// The unit direction (the block's frame) of texel `(i, j)` of a face.
fn texel_direction(face: usize, i: usize, j: usize, size: usize) -> [f64; 3] {
    let (sc, tc) = texel_centre(i, j, size);
    let d = face_direction(face, sc, tc);
    let [x, y, z] = sh_direction(d).map(f64::from);
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

/// A texel's centre on the face plane, both coordinates in (-1, 1).
fn texel_centre(i: usize, j: usize, size: usize) -> (f32, f32) {
    let c = |k: usize| (2 * k + 1) as f32 / size as f32 - 1.0;
    (c(i), c(j))
}

/// The second-order projection of a capture: each texel's colour times its
/// solid-angle weight `4 / (1 + x^2 + y^2)^1.5` onto [`basis`], the sums
/// scaled by `4 pi / sum(weights)`.
#[must_use]
pub fn project_coefficients(faces: &Faces) -> Coefficients {
    let size = faces.size;
    let mut sums = [[0.0_f64; 9]; 3];
    let mut total = 0.0_f64;
    for (face, texels) in faces.texels.iter().enumerate() {
        for j in 0..size {
            for i in 0..size {
                let (sc, tc) = texel_centre(i, j, size);
                let w = f64::from(texel_weight(sc, tc));
                let y = basis(texel_direction(face, i, j, size));
                let colour = texels[j * size + i];
                for (ch, sum) in sums.iter_mut().enumerate() {
                    let l = f64::from(colour[ch]) * w;
                    for (s, y) in sum.iter_mut().zip(&y) {
                        *s += l * y;
                    }
                }
                total += w;
            }
        }
    }
    let scale = 4.0 * std::f64::consts::PI / total;
    sums.map(|ch| ch.map(|v| (v * scale) as f32))
}

/// A capture's irradiance block: the projection, packed.
#[must_use]
pub fn project(faces: &Faces) -> Irradiance {
    Irradiance::pack(&project_coefficients(faces))
}

/// The colour a capture texel holds: the render target's HDR value through
/// the renderer's tone map and display encode, clamped and stored as a byte,
/// then read as the byte over 256 (stand-in for the reference's target,
/// whose tone mapping is not known).
#[must_use]
pub fn texel_byte_colour(hdr: [f32; 3]) -> [f32; 3] {
    crate::post::tone_map::display_colour(hdr).map(|c| (c.clamp(0.0, 1.0) * 255.0).round() / 256.0)
}

/// One captured face from its readback: `size * size` RGBA half-float texels
/// (`bytes`, row-major, `row_bytes` per row) as [`texel_byte_colour`]s.
#[must_use]
pub fn face_from_rgba16f(bytes: &[u8], row_bytes: usize, size: usize) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(size * size);
    for j in 0..size {
        let row = &bytes[j * row_bytes..j * row_bytes + size * 8];
        for texel in row.chunks_exact(8) {
            let c = |k: usize| half::f16::from_le_bytes([texel[2 * k], texel[2 * k + 1]]).to_f32();
            out.push(texel_byte_colour([c(0), c(1), c(2)]));
        }
    }
    out
}
