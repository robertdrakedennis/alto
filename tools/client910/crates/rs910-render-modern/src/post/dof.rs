//! The modern renderer's depth of field (renderer plan §4(s)): the near and far
//! blur of the lit frame before the tonemap. Off by default, as in modern
//! gameplay (the effect sits behind a flag that no option or call site
//! sets: plan §4(h)); `CLIENT910_MODERN_DOF=on` draws it (screenshots such as
//! the aerial reference, whose distance is soft).
//!
//! # The effect
//!
//! - Focus: with `v` = the view depth minus the focal point and `P` the four
//!   DOF parameters, the near weight is `step(0, P.y) smoothstep(P.x, P.x +
//!   P.y, -v)` and the far weight `step(0, P.w) smoothstep(P.z, P.z + P.w,
//!   v)`. The focus pass writes both.
//! - Blur, per Kawase iteration (four taps at `(iteration + 1/2)` texels
//!   diagonally): a pixel whose weight is 0 is invalid (the reference writes
//!   NaN; the near blur keeps the source where a neighbour's near weight is
//!   set). Otherwise the four taps are averaged, invalid taps dropped; for
//!   the far blur only taps whose own depth lies past `P.z + P.w / 10` count,
//!   and the far blur keeps the source when fewer than half a tap is left.
//!   The bokeh boost is `mix(mean, max * boost.zw, smoothstep(boost.xy,
//!   max))`.
//! - Spread: the focus weights spread by a Kawase step, `0.4` times the four
//!   taps' sum.
//! - Composite: the near blur mixed in by the near weight, the far blur by
//!   `smoothstep(P.z, P.z + P.w, v)` where that is at least the near weight.
//!
//! # This port
//!
//! Full size, validity in the alpha channel instead of NaN. The values the
//! reference sets from its host code are chosen here (in brackets the names
//! they were tuned under): the focal point is the camera target's view depth
//! (the orbit's focus); near blur from half of it closer over a quarter of it
//! [`NEAR_START`], [`NEAR_RANGE`]; far blur from [`FAR_START`] [`dof_far`] of
//! it farther over [`FAR_RANGE`] [`dof_range`] of it; the Kawase iterations
//! [`ITERATIONS`]; no bokeh boost.

/// Chosen: the near blur starts this share of the focal distance closer.
pub const NEAR_START: f32 = 0.5;
/// Chosen: over this share of the focal distance.
pub const NEAR_RANGE: f32 = 0.25;
/// Chosen: the far blur starts this share of the focal distance farther.
pub const FAR_START: f32 = 0.35;
/// Chosen: over this share of the focal distance.
pub const FAR_RANGE: f32 = 1.2;
/// Chosen: the Kawase iterations of each blur (a medium kernel shape).
pub const ITERATIONS: [f32; 5] = [0.0, 1.0, 2.0, 2.0, 3.0];

/// The DOF parameters for focal distance `focal`.
#[must_use]
pub fn params(focal: f32) -> [f32; 4] {
    [
        NEAR_START * focal,
        NEAR_RANGE * focal,
        FAR_START * focal,
        FAR_RANGE * focal,
    ]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The focus weights (module docs): `(near, far)` at view depth `depth`.
#[must_use]
pub fn focus(depth: f32, focal: f32, p: [f32; 4]) -> [f32; 2] {
    let v = depth - focal;
    let near = if p[1] >= 0.0 {
        smoothstep(p[0], p[0] + p[1], -v)
    } else {
        0.0
    };
    let far = if p[3] >= 0.0 {
        smoothstep(p[2], p[2] + p[3], v)
    } else {
        0.0
    };
    [near, far]
}
