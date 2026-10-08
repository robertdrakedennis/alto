//! Sun shadows of the modern renderer (renderer plan M3): cascaded shadow maps
//! of the sun, their split, fit and lookup parameters (CPU only; the passes
//! are in [`crate::frame`], the WGSL in [`crate::shaders`]).
//!
//! # What the modern client does
//!
//! - **Quality profiles**: quality 0 two cascades, 1024-texel tiles, shadow
//!   distance 10000; 1 three, 1024, 12500; 2 four, 1024, 17500; 3 four, 2048,
//!   22500 (and a 24-bit depth buffer). Common: split near 5.0, split blend
//!   λ 0.55, the casters' depth range pulled towards the sun by 7250, and a
//!   depth bias per cascade of -0.0095, -0.00925, -0.0075, -0.006. At most
//!   four cascades.
//! - **Atlas**: one depth target of twice the tile size, the four cascades as
//!   its 2x2 tiles (viewports `(0,0)`, `(res,0)`, `(0,res)`, `(res,res)`),
//!   each cascade's texture matrix a scale of 0.25 and an offset of 0.25/0.75
//!   into its tile, with the cascade's bias as the depth offset. Sampled with
//!   a `LEQUAL` comparison and clamp-to-edge.
//! - **Splits**: the practical split scheme, split *i* of *n* =
//!   `λ·near·(far/near)^(i/n) + (1-λ)·(near + (i/n)·(far-near))`, unused
//!   slots repeating the last; cascade 0 starts at view depth 0.
//! - **Fit**: the light view is a look-at from the camera along the sun
//!   (world up, or world right when the sun is nearly vertical); the
//!   cascade's slice of the view frustum (the projection's corner rays scaled
//!   to the slice's near and far depths) is transformed to light space and
//!   enclosed in an axis-aligned box; the orthographic projection spans that
//!   box, its near plane moved towards the sun by the pull-back, depth mapped
//!   to `[0, 1]`.
//! - **Update**: every cascade is refitted every frame while the camera moves
//!   (by 15 units or more) or the sun turns; a still camera refreshes cascade
//!   *i* every 1, 1, 2, 3 frames.
//! - **Receive**: the cascade is chosen by view depth against the splits
//!   (qualities 0-1) or as the first cascade whose atlas tile holds the point
//!   (qualities 2-3); filtered with hardware-compare taps: a 2x2 box
//!   (quality 0), an approximate 4x4 box (four corner taps, all sixteen only
//!   when they disagree; quality 1), a 4x4 box (qualities 2-3), with the tap
//!   coordinates clamped inside the cascade's tile; then `min(1, lit + 1 -
//!   strength)` and a fade to lit towards the shadow distance (linear for
//!   qualities 0-1, `smoothstep` for 2-3).
//! - **Casters**: the pass-0 queue's groups, the first with front faces
//!   culled, the others with none, no polygon offset; terrain casts (depth
//!   only); the sky does not.
//! - **Screen-space selection**: a shadow box (an eighth of the view, at least
//!   64 texels) with two extra pass types is the screen-space variant of the
//!   cascade selection.
//!
//! # What this crate does
//!
//! The profiles, splits, bias, atlas layout, cascade selection, filters and
//! fade above, with these choices (flagged in the docs where they apply):
//!
//! - **Stable fit** instead of the tight box: each slice is enclosed in a
//!   sphere (its radius depends only on the projection and the slice
//!   depths, so it does not change when the camera turns), the orthographic
//!   square is its diameter, and the square's centre is snapped to whole
//!   shadow texels in a light frame fixed to the scene (the sun's axes over
//!   scene-local coordinates), so a moving camera moves the shadow map by
//!   whole texels and the shadows do not shimmer. The cascade cache
//!   ([`cache`]) keeps a cascade's map, and the fit it was drawn with, while
//!   its casters and fit are unchanged, and lets a moving camera's far
//!   cascades keep their fit for 1, 1, 2, 3 frames.
//! - **Per-fragment selection** (by split or by map), not the screen-space
//!   shadow box pass.
//! - **Casters are two-sided**: the classic models are often open meshes
//!   (planes for leaves, flags, fences), which front-face culling of the
//!   first group would drop; the per-cascade bias is the modern client's.
//! - **Transparent entities** cast where their surface is at least half
//!   opaque (the caster pass's alpha test); spot shadows and hint arrows
//!   never cast; sky models never cast.
//! - **Strength** 1 (the modern client takes it from an environment value
//!   clamped to 1; the environment of map file 6 is M5). The fade starts
//!   at [`FADE_FROM`] of the last cascade; the modern client's fade fraction
//!   and its single-tap "low quality depth" fraction are fields this
//!   reference does not show being set, so the whole range uses the quality's
//!   filter.
//! - **Units**: the modern client's world units are taken to be the classic
//!   fine units (512 per tile; [inf]: its distances, 20-44 tiles, fit the
//!   game's view).
//!
//! # Settings
//!
//! [`Settings::from_options`] maps the faithful `ClientOptions` without
//! changing them (the capability profile and every value CS2 or the
//! packets see stay faithful): `shadowQuality` (the classic client stores
//! 0-4, default 1, and reads it nowhere) is taken as the modern client's
//! level, 0 LOW .. 4 ULTRA_PLUS ([inf]; any other value LOW; the presets
//! are [`crate::shadows::presets::profile`]'s); `sceneryShadows` 0 (classic:
//! no loc hard shadows, forced 0 without textures) stops locs and floors
//! casting; `characterShadows` 0 (classic: no spot shadows) stops players,
//! NPCs and other transient models casting. With neither, no shadow pass
//! runs.

use glam::{DMat4, DVec3, DVec4};

/// At most four cascades.
pub const MAX_CASCADES: usize = 4;
/// The shadow atlas's depth format.
pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The split scheme's near distance.
pub const SPLIT_NEAR: f32 = 5.0;
/// The practical split blend λ.
pub const SPLIT_LAMBDA: f32 = 0.55;
/// How far the casters' depth range reaches towards the sun past the
/// cascade (added to the light-space minimum depth of the fit).
pub const CASTER_PULL_BACK: f32 = 7250.0;
/// Where in the last cascade the fade to lit starts (this crate's choice;
/// see the module docs).
pub const FADE_FROM: f32 = 0.8;
/// A caster keeps a fragment whose alpha reaches this.
pub const CASTER_ALPHA: f32 = 0.5;
/// The size of one cascade's caster uniform slot (the dynamic offset
/// alignment).
pub const CASTER_SLOT: u64 = 256;

/// The modern client's cascade selection for a quality (desktop branch): low
/// and medium by the cascades' bounding spheres (2), high and ultra by the
/// best map (1); the classic scheme chose by split for low and medium. The
/// filters and fades per quality are the classic ones already.
#[must_use]
pub fn cascade_selection(quality: Quality) -> f32 {
    if quality >= Quality::High {
        1.0
    } else {
        2.0
    }
}

/// The sun shadows' quality level (defined with the settings).
pub use crate::settings::ShadowQuality as Quality;

/// The shadow filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// The 2x2 box: four compare taps at texel offsets -1..0.
    Box2x2 = 0,
    /// The approximate 4x4 box: four corner taps at ±1.5 texels, all
    /// sixteen of the half-texel 4x4 grid only when they disagree.
    Box4x4Approx = 1,
    /// The 4x4 box: sixteen compare taps at texel offsets -2..1.
    Box4x4 = 2,
}

/// One quality's parameters (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub quality: Quality,
    pub cascades: usize,
    /// One cascade's tile size in texels; the atlas is twice this.
    pub resolution: u32,
    /// The shadow distance (the last split).
    pub distance: f32,
    pub filter: Filter,
    /// Select the cascade by map (else by split).
    pub select_by_map: bool,
    /// The fade is a `smoothstep` (else linear).
    pub smooth_fade: bool,
    /// The depth bias per cascade.
    pub bias: [f32; MAX_CASCADES],
    /// Where the quality's filter gives way to the 2x2 box, as a fraction of the way from the split
    /// near distance to the shadow distance; `None`: past the shadow
    /// distance (the pre-lane choice, see the module docs).
    pub low_quality_fraction: Option<f32>,
    /// The receiver's position offset along the normal.
    pub normal_bias: f32,
    /// How far (texels) the filter's taps reach past a texel's footprint:
    /// the caster culling's margin (`crate::shadows::casters::cascades_met`).
    pub reach_texels: f64,
    /// The tile extents' inset in atlas texels (the minimum extent that
    /// selection by map tests and clamps against; 0: the whole tile).
    pub extents_inset: f32,
}

impl Profile {
    /// The atlas's size in texels (2x2 tiles).
    #[must_use]
    pub fn atlas_size(&self) -> u32 {
        self.resolution * 2
    }

    /// The cascades' far view depths ([`practical_splits`]).
    #[must_use]
    pub fn splits(&self) -> [f32; MAX_CASCADES] {
        practical_splits(self.cascades, SPLIT_NEAR, self.distance, SPLIT_LAMBDA)
    }
}

/// The practical split scheme: split *i* (1-based) of `count` is
/// `λ·near·(far/near)^(i/count) + (1-λ)·(near + (i/count)·(far - near))`; the
/// slots past `count` repeat the last. `count` is clamped to 1-4.
#[must_use]
pub fn practical_splits(count: usize, near: f32, far: f32, lambda: f32) -> [f32; MAX_CASCADES] {
    let n = count.clamp(1, MAX_CASCADES);
    let mut out = [0.0; MAX_CASCADES];
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        let f = (i + 1) as f32 / n as f32;
        let log = f64::from(far / near).powf(f64::from(f)) as f32 * near;
        *slot = log * lambda + (near + f * (far - near)) * (1.0 - lambda);
    }
    let last = out[n - 1];
    for slot in &mut out[n..] {
        *slot = last;
    }
    out
}

/// The shadow settings the shell derives from `ClientOptions` (see the
/// module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `None`: shadows off.
    pub quality: Option<Quality>,
    /// Locs and floors cast (`sceneryShadows != 0`).
    pub scenery: bool,
    /// Players, NPCs and other transient models cast (`characterShadows !=
    /// 0`).
    pub characters: bool,
}

impl Default for Settings {
    /// The classic defaults (`sceneryShadows` 2, `shadowQuality` 1,
    /// `characterShadows` 1): the offline client, before any options load.
    fn default() -> Self {
        Self::from_options(2, 1, 1)
    }
}

impl Settings {
    /// The mapping of the module docs from the option values.
    #[must_use]
    pub fn from_options(scenery_shadows: i32, shadow_quality: i32, character_shadows: i32) -> Self {
        let quality = match shadow_quality {
            1 => Quality::Medium,
            2 => Quality::High,
            3 => Quality::Ultra,
            4 => Quality::UltraPlus,
            _ => Quality::Low,
        };
        Self {
            quality: Some(quality),
            scenery: scenery_shadows != 0,
            characters: character_shadows != 0,
        }
    }

    /// The profile to render with (`None`: no shadow pass).
    #[must_use]
    pub fn profile(&self) -> Option<Profile> {
        self.quality
            .filter(|_| self.scenery || self.characters)
            .map(crate::shadows::presets::profile)
    }
}

/// The light's axes over scene-local classic coordinates (y down): `forward`
/// the direction the light travels (away from the sun), `right` and `up`
/// across it (`Matrix44LookAtLH` with world up, or world right when the
/// sun is within ~2.5° of vertical).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightBasis {
    pub right: DVec3,
    pub up: DVec3,
    pub forward: DVec3,
}

impl LightBasis {
    /// The basis for `towards_sun` (the environment's sun direction, from
    /// the ground to the sun).
    #[must_use]
    pub fn new(towards_sun: [f32; 3]) -> Self {
        let sun = DVec3::from(towards_sun.map(f64::from));
        let forward = if sun.length_squared() > 1e-12 {
            -sun.normalize()
        } else {
            DVec3::new(0.0, 1.0, 0.0)
        };
        let world_up = DVec3::new(0.0, -1.0, 0.0);
        let reference = if forward.dot(world_up).abs() > 0.999 {
            DVec3::X
        } else {
            world_up
        };
        let right = reference.cross(forward).normalize();
        let up = forward.cross(right);
        Self { right, up, forward }
    }

    /// `p` in light axes.
    #[must_use]
    pub fn apply(&self, p: DVec3) -> DVec3 {
        DVec3::new(self.right.dot(p), self.up.dot(p), self.forward.dot(p))
    }

    /// The rotation as a column-vector matrix (glam, `f32`).
    #[must_use]
    pub fn matrix(&self) -> glam::Mat4 {
        let m = DMat4::from_cols(
            DVec4::new(self.right.x, self.up.x, self.forward.x, 0.0),
            DVec4::new(self.right.y, self.up.y, self.forward.y, 0.0),
            DVec4::new(self.right.z, self.up.z, self.forward.z, 0.0),
            DVec4::W,
        );
        m.as_mat4()
    }
}

/// The corner rays of a projection: `(x/z, y/z)` in classic view space of the
/// four NDC corners (the inverse-projection step of the fit).
#[must_use]
pub fn corner_rays(projection: &[f32; 16]) -> [[f64; 2]; 4] {
    let inverse = DMat4::from_cols_array(&projection.map(f64::from)).inverse();
    let corner = |sx: f64, sy: f64| {
        let v = inverse * DVec4::new(sx, sy, 0.0, 1.0);
        [v.x / v.z, v.y / v.z]
    };
    [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(-1.0, 1.0),
        corner(1.0, 1.0),
    ]
}

/// One cascade's fitted orthographic box (see the module docs "Stable
/// fit"), in the light frame over scene-local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// The square's centre across the light (`right`, `up`), snapped to
    /// whole texels.
    pub centre: [f64; 2],
    /// Half the square's side (the slice's bounding sphere radius).
    pub radius: f64,
    /// One shadow texel.
    pub texel: f64,
    /// The depth range along `forward`: the sphere's, its near end moved
    /// towards the sun by [`CASTER_PULL_BACK`].
    pub depth: [f64; 2],
}

/// The bounding sphere of the view-space slice between depths `near` and
/// `far` of the frustum with corner `rays`: its centre (view space) and
/// radius. The centre lies on the rays' mean direction at the depth that
/// minimises the farthest corner (a convex search, deterministic), and the
/// radius is rounded up to a whole unit: both depend only on the
/// projection and the depths, not on where the camera is or looks.
#[must_use]
pub fn slice_sphere(rays: &[[f64; 2]; 4], near: f64, far: f64) -> (DVec3, f64) {
    let mean = rays
        .iter()
        .fold([0.0, 0.0], |a, r| [a[0] + r[0] / 4.0, a[1] + r[1] / 4.0]);
    let axis = DVec3::new(mean[0], mean[1], 1.0);
    let corners: Vec<DVec3> = [near, far]
        .iter()
        .flat_map(|&d| rays.iter().map(move |r| DVec3::new(r[0] * d, r[1] * d, d)))
        .collect();
    let reach = |t: f64| {
        let c = axis * t;
        corners
            .iter()
            .map(|p| p.distance(c))
            .fold(0.0_f64, f64::max)
    };
    let (mut lo, mut hi) = (near, far);
    for _ in 0..100 {
        let a = lo + (hi - lo) / 3.0;
        let b = hi - (hi - lo) / 3.0;
        if reach(a) <= reach(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    let t = (lo + hi) / 2.0;
    (axis * t, reach(t).ceil())
}

/// Fit one cascade: the slice `near..far` of the camera with corner `rays`,
/// whose view-to-scene transform is `view_to_scene` (scene-local
/// coordinates), in the light frame `basis`, `resolution` texels across.
#[must_use]
pub fn fit_cascade(
    view_to_scene: &DMat4,
    rays: &[[f64; 2]; 4],
    near: f64,
    far: f64,
    basis: &LightBasis,
    resolution: u32,
) -> Fit {
    let (centre, radius) = slice_sphere(rays, near, far);
    let world = view_to_scene.transform_point3(centre);
    let light = basis.apply(world);
    let texel = 2.0 * radius / f64::from(resolution);
    let snap = |v: f64| (v / texel).round() * texel;
    Fit {
        centre: [snap(light.x), snap(light.y)],
        radius,
        texel,
        depth: [
            light.z - radius - f64::from(CASTER_PULL_BACK),
            light.z + radius,
        ],
    }
}

/// An affine map `v * scale + offset` per component from the camera-local
/// light axes (`basis` applied to a camera-local point) of one cascade.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CascadeMap {
    pub scale: [f32; 3],
    pub offset: [f32; 3],
}

impl Fit {
    /// Camera-local light axes to the cascade's NDC (`x`, `y` in `[-1, 1]`,
    /// y up) and `[0, 1]` depth, for a camera whose scene-local position is
    /// `origin` (the offsets absorb it in `f64`, so large scene coordinates
    /// never reach the GPU).
    #[must_use]
    pub fn clip_map(&self, basis: &LightBasis, origin: DVec3) -> CascadeMap {
        let o = basis.apply(origin);
        let range = self.depth[1] - self.depth[0];
        CascadeMap {
            scale: [
                (1.0 / self.radius) as f32,
                (1.0 / self.radius) as f32,
                (1.0 / range) as f32,
            ],
            offset: [
                ((o.x - self.centre[0]) / self.radius) as f32,
                ((o.y - self.centre[1]) / self.radius) as f32,
                ((o.z - self.depth[0]) / range) as f32,
            ],
        }
    }

    /// Camera-local light axes to atlas texture coordinates of `cascade`'s
    /// tile (u right, v down) and its compare depth with `bias` (the
    /// texture matrix: scale 0.25, offset 0.25/0.75, the bias as the depth
    /// offset).
    #[must_use]
    pub fn texture_map(
        &self,
        basis: &LightBasis,
        origin: DVec3,
        cascade: usize,
        bias: f32,
    ) -> CascadeMap {
        let clip = self.clip_map(basis, origin);
        let (tx, ty) = tile_origin(cascade);
        CascadeMap {
            scale: [clip.scale[0] * 0.25, -clip.scale[1] * 0.25, clip.scale[2]],
            offset: [
                clip.offset[0] * 0.25 + 0.25 + tx,
                -clip.offset[1] * 0.25 + 0.25 + ty,
                clip.offset[2] + bias,
            ],
        }
    }
}

/// The atlas UV of the top-left corner of `cascade`'s tile.
#[must_use]
pub fn tile_origin(cascade: usize) -> (f32, f32) {
    (
        (cascade & 1) as f32 * 0.5,
        ((cascade >> 1) & 1) as f32 * 0.5,
    )
}

/// The `Shadow` uniform block of [`crate::shaders::FORWARD_WGSL`]
/// (the modern client's sunlight shadow block: the cascades' view depths,
/// the mapping parameters, the fade attenuation, the texture matrix scale and
/// offset, the atlas extents and the single-tap depth).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowUniforms {
    /// Camera-local to light axes (rotation).
    pub light_view: [[f32; 4]; 4],
    pub tex_scale: [[f32; 4]; MAX_CASCADES],
    pub tex_offset: [[f32; 4]; MAX_CASCADES],
    /// Each tile's UV box `[u0, v0, u1, v1]`.
    pub extents: [[f32; 4]; MAX_CASCADES],
    /// The splits (cascade far view depths).
    pub splits: [f32; 4],
    /// x: one atlas texel in UV, y: cascade count (0: off), z: strength, w:
    /// [`Filter`].
    pub params: [f32; 4],
    /// x: fade start, y: fade end (view depth), z: `1 / (end - start)`, w: 1
    /// for the smooth fade.
    pub fade: [f32; 4],
    /// x: 1 to select by map, y: the single-tap depth, zw unused.
    pub lookup: [f32; 4],
    /// The cascades' world bounding spheres (`crate::models::shading`): each
    /// cascade's sphere, its centre in the
    /// camera-local light axes (`light_view` applied to a camera-local
    /// point) and its radius squared. The pre-lane shader does not read it.
    pub spheres: [[f32; 4]; MAX_CASCADES],
    /// x: the receiver's normal offset ([`presets::NORMAL_BIAS`]); y:
    /// the modern client's cascade selection ([`cascade_selection`]: 1 map, 2 sphere).
    pub bias_select: [f32; 4],
}

/// One cascade's caster uniforms (`Caster` in the WGSL), in a
/// [`CASTER_SLOT`]-byte slot.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CasterUniforms {
    pub light_view: [[f32; 4]; 4],
    pub clip_scale: [f32; 4],
    pub clip_offset: [f32; 4],
    pub pad: [[f32; 4]; 10],
}

impl Default for CasterUniforms {
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// One frame's shadow parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowFrame {
    pub profile: Profile,
    pub basis: LightBasis,
    pub fits: Vec<Fit>,
    pub uniforms: ShadowUniforms,
    pub casters: [CasterUniforms; MAX_CASCADES],
}

impl ShadowFrame {
    /// The cascades of a camera (`view` and `projection`: its camera-local
    /// classic entries, row-major; `origin`: the camera-local origin in
    /// scene-local coordinates) under a sun towards `towards_sun`.
    #[must_use]
    pub fn new(
        profile: Profile,
        view: &[f32; 16],
        projection: &[f32; 16],
        origin: [f32; 3],
        towards_sun: [f32; 3],
    ) -> Self {
        let basis = LightBasis::new(towards_sun);
        let fits = Self::fit(&profile, &basis, view, projection, origin);
        Self::from_fits(profile, basis, fits, origin)
    }

    /// Each cascade's fit of a camera ([`Self::new`]'s arguments) in the
    /// light frame `basis`.
    #[must_use]
    pub fn fit(
        profile: &Profile,
        basis: &LightBasis,
        view: &[f32; 16],
        projection: &[f32; 16],
        origin: [f32; 3],
    ) -> Vec<Fit> {
        let origin = DVec3::from(origin.map(f64::from));
        let view_to_local = DMat4::from_cols_array(&view.map(f64::from)).inverse();
        let view_to_scene = DMat4::from_translation(origin) * view_to_local;
        let rays = corner_rays(projection);
        let splits = profile.splits();
        (0..profile.cascades)
            .map(|k| {
                let near = if k == 0 { 0.0 } else { splits[k - 1] };
                fit_cascade(
                    &view_to_scene,
                    &rays,
                    f64::from(near),
                    f64::from(splits[k]),
                    basis,
                    profile.resolution,
                )
            })
            .collect()
    }

    /// The frame of cascades `fits` (one per cascade of `profile`, fitted
    /// in `basis`: this frame's, or a cascade's earlier fit its map still
    /// holds, `crate::shadows::cache`) seen from a camera whose camera-local
    /// origin is `origin` (scene-local).
    #[must_use]
    pub fn from_fits(
        profile: Profile,
        basis: LightBasis,
        fits: Vec<Fit>,
        origin: [f32; 3],
    ) -> Self {
        let origin = DVec3::from(origin.map(f64::from));
        let splits = profile.splits();
        let light_view = basis.matrix().to_cols_array_2d();
        let mut uniforms = ShadowUniforms {
            light_view,
            splits,
            ..ShadowUniforms::default()
        };
        let mut casters = [CasterUniforms::default(); MAX_CASCADES];
        for (k, fit) in fits.iter().enumerate() {
            let clip = fit.clip_map(&basis, origin);
            let tex = fit.texture_map(&basis, origin, k, profile.bias[k]);
            uniforms.tex_scale[k] = [tex.scale[0], tex.scale[1], tex.scale[2], 0.0];
            uniforms.tex_offset[k] = [tex.offset[0], tex.offset[1], tex.offset[2], 0.0];
            let (u0, v0) = tile_origin(k);
            let inset = profile.extents_inset / profile.atlas_size() as f32;
            uniforms.extents[k] = [u0 + inset, v0 + inset, u0 + 0.5 - inset, v0 + 0.5 - inset];
            casters[k] = CasterUniforms {
                light_view,
                clip_scale: [clip.scale[0], clip.scale[1], clip.scale[2], 0.0],
                clip_offset: [clip.offset[0], clip.offset[1], clip.offset[2], 0.0],
                ..CasterUniforms::default()
            };
            // The fit's sphere (its centre is the square's, snapped; its
            // depth the middle of the caster range past the pull-back) in
            // the camera-local light axes.
            let o = basis.apply(origin);
            let centre_depth = fit.depth[0] + fit.radius + f64::from(CASTER_PULL_BACK);
            uniforms.spheres[k] = [
                (fit.centre[0] - o.x) as f32,
                (fit.centre[1] - o.y) as f32,
                (centre_depth - o.z) as f32,
                (fit.radius * fit.radius) as f32,
            ];
        }
        let n = profile.cascades;
        let last = splits[n - 1];
        let before = if n > 1 { splits[n - 2] } else { 0.0 };
        let start = before + (last - before) * FADE_FROM;
        uniforms.params = [
            1.0 / profile.atlas_size() as f32,
            n as f32,
            1.0,
            profile.filter as u32 as f32,
        ];
        uniforms.fade = [
            start,
            last,
            1.0 / (last - start),
            if profile.smooth_fade { 1.0 } else { 0.0 },
        ];
        uniforms.lookup = [
            if profile.select_by_map { 1.0 } else { 0.0 },
            // The single-tap depth: past the shadow distance (see the
            // module docs), or the modern client's single-tap depth.
            profile.low_quality_fraction.map_or(last * 2.0, |f| {
                SPLIT_NEAR + (profile.distance - SPLIT_NEAR) * f
            }),
            0.0,
            0.0,
        ];
        uniforms.bias_select = [
            profile.normal_bias,
            cascade_selection(profile.quality),
            0.0,
            0.0,
        ];
        Self {
            profile,
            basis,
            fits,
            uniforms,
            casters,
        }
    }
}

#[cfg(test)]
mod tests;

pub mod cache;
pub mod casters;
pub mod interior;
pub mod point;
pub mod presets;
