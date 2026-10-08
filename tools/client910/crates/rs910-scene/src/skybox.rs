//! The environment skybox: config decode, per-environment selection, the
//! `SkyBox` update/fade state and the frame plan the renderer consumes.
//!
//! The 910 cache ships no `SKYDECORTYPE` group (config group 30 is absent)
//! and no `SKYBOXTYPE` references a decor, so no 910 content draws one. The
//! decor machinery is complete for the revisions that do: placement here, the
//! sprite bake in [`crate::sky_decor`], the drawing in the renderers.

pub use rs910_config::skybox_types::*;
use std::collections::BTreeMap;

/// The highest set bit of `v`.
#[must_use]
pub fn highest_bit(v: i32) -> i32 {
    let v1 = ((v as u32) >> 1) as i32;
    let v2 = v1 | ((v1 as u32) >> 1) as i32;
    let v3 = v2 | ((v2 as u32) >> 2) as i32;
    let v4 = v3 | ((v3 as u32) >> 4) as i32;
    let v5 = v4 | ((v4 as u32) >> 8) as i32;
    let v6 = v5 | ((v5 as u32) >> 16) as i32;
    v & !v6
}

/// `v` rounded up to a power of two (wrapping 32-bit arithmetic).
#[must_use]
pub fn bitceil(v: i32) -> i32 {
    let v6 = v.wrapping_sub(1);
    let v1 = v6 | ((v6 as u32) >> 1) as i32;
    let v2 = v1 | ((v1 as u32) >> 2) as i32;
    let v3 = v2 | ((v2 as u32) >> 4) as i32;
    let v4 = v3 | ((v3 as u32) >> 8) as i32;
    let v5 = v4 | ((v4 as u32) >> 16) as i32;
    v5.wrapping_add(1)
}

/// A sky decor (sun/moon sprite): the fields of its config type plus the
/// placement derived for the current viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyboxDecor {
    /// Decor kind (0 texture sprite, 1 lit sphere, 2 model).
    pub kind: i32,
    /// Texture or model id.
    pub texture: i32,
    /// Position; a fixed direction when `fixed` is set.
    pub position: [i32; 3],
    /// Size.
    pub size: i32,
    /// Colour.
    pub colour: i32,
    /// The position is a fixed direction.
    pub fixed: bool,
    /// Rotation.
    pub rotation: [i32; 3],
    /// Pitch of the decor direction.
    pub pitch: i32,
    /// Yaw of the decor direction.
    pub yaw: i32,
    /// Direction of the decor from the sky's origin, of length 256 (the
    /// vector the renderers project).
    pub direction: [i32; 3],
    /// Distance (sort key).
    pub distance: i32,
    /// Baked sprite size.
    pub sprite_size: i32,
    /// On-screen size.
    pub screen_size: i32,
}

impl SkyboxDecor {
    /// Build a decor from its config type.
    #[must_use]
    pub fn from_type(t: &SkyDecorType) -> Self {
        Self {
            kind: t.kind,
            texture: t.texture,
            position: t.position,
            size: t.size,
            colour: t.colour,
            fixed: t.fixed,
            rotation: t.rotation,
            pitch: 0,
            yaw: 0,
            direction: [0; 3],
            distance: 0,
            sprite_size: 0,
            screen_size: 0,
        }
    }

    /// Place the decor as seen from `(a, b, c)` for a viewport `height`;
    /// false when it is under 8 px.
    pub fn select(&mut self, a: i32, b: i32, c: i32, height: i32) -> bool {
        let (dir_x, dir_y, dir_z);
        if self.fixed {
            self.distance = 1_073_741_823;
            dir_x = self.position[0];
            dir_y = self.position[1];
            dir_z = self.position[2];
        } else {
            let delta_x = self.position[0].wrapping_sub(a);
            let delta_y = self.position[1].wrapping_sub(b);
            let delta_z = self.position[2].wrapping_sub(c);
            // 32-bit wrapping sums: far decors overflow them.
            self.distance = (f64::from(
                delta_z
                    .wrapping_mul(delta_z)
                    .wrapping_add(delta_x.wrapping_mul(delta_x))
                    .wrapping_add(delta_y.wrapping_mul(delta_y)),
            ))
            .sqrt() as i32;
            if self.distance == 0 {
                self.distance = 1;
            }
            dir_x = (delta_x << 8) / self.distance;
            dir_y = (delta_y << 8) / self.distance;
            dir_z = (delta_z << 8) / self.distance;
        }
        let (mut dir_x, mut dir_y, mut dir_z) = (dir_x, dir_y, dir_z);
        let dir_length = ((f64::from(
            dir_z
                .wrapping_mul(dir_z)
                .wrapping_add(dir_x.wrapping_mul(dir_x))
                .wrapping_add(dir_y.wrapping_mul(dir_y)),
        ))
        .sqrt()
            * 256.0) as i32;
        if dir_length > 128 {
            dir_x = (dir_x << 16) / dir_length;
            dir_y = (dir_y << 16) / dir_length;
            dir_z = (dir_z << 16) / dir_length;
            self.direction = [dir_x, dir_y, dir_z];
            self.screen_size =
                self.size.wrapping_mul(height) / if self.fixed { 1024 } else { self.distance };
        } else {
            self.screen_size = 0;
        }
        if self.screen_size < 8 {
            return false;
        }
        let mut fitted_size = bitceil(self.screen_size);
        if fitted_size > height {
            fitted_size = highest_bit(height);
        }
        if fitted_size > 512 {
            fitted_size = 512;
        }
        self.sprite_size = fitted_size;
        self.pitch =
            ((f64::from(dir_y as f32 / 256.0)).asin() * 2607.5945876176133) as i32 & 0x3FFF;
        self.yaw =
            ((f64::from(dir_x)).atan2(f64::from(-dir_z)) * 2607.5945876176133) as i32 & 0x3FFF;
        true
    }
}

/// Sorts `items` alongside `keys` over `lo..=hi` (recursive quicksort); the
/// `(i & bias)` term reproduces the original order between equal keys.
fn sort_by_key(keys: &mut [i32], items: &mut [usize], lo: i32, hi: i32) {
    if lo >= hi {
        return;
    }
    let mid = (lo + hi) / 2;
    let mut store = lo;
    let pivot = keys[mid as usize];
    keys.swap(mid as usize, hi as usize);
    items.swap(mid as usize, hi as usize);
    let bias = i32::from(pivot != i32::MAX);
    for i in lo..hi {
        if keys[i as usize] < (i & bias) + pivot {
            keys.swap(i as usize, store as usize);
            items.swap(i as usize, store as usize);
            store += 1;
        }
    }
    keys.swap(hi as usize, store as usize);
    items.swap(hi as usize, store as usize);
    sort_by_key(keys, items, lo, store - 1);
    sort_by_key(keys, items, store + 1, hi);
}

/// Skybox cache key: `(id, a, b, c)`.
pub type SkyboxKey = (i32, i32, i32, i32);

/// `SkyBox` without its toolkit objects: the GPU model mesh
/// and the 2D sprite live in `skybox_render`; this owns the state machine.
#[derive(Clone, Debug)]
pub struct SkyBox {
    pub key: SkyboxKey,
    /// Material of the 2D path (`-1` = fill only).
    pub material: i32,
    /// The decors of the type, if it has any.
    pub decors: Option<Vec<SkyboxDecor>>,
    /// Visible decors, nearest first.
    pub visible: Vec<usize>,
    /// The decor lighting the others (read by the decor sprite bakes).
    pub sun: Option<usize>,
    /// The viewpoint the decors are placed relative to (`a`, `b`, `c` of the key).
    pub origin: [i32; 3],
    /// How the 2D sprite fills the sky.
    pub fill: Option<SkyBoxFillMode>,
    /// Model id (`>= 0` loads a 3D sky).
    pub model_id: i32,
    /// 2D sprite size (power of two, `<= 512`).
    pub sprite_size: i32,
    /// Height the decors were placed for.
    last_height: i32,
    /// The 3D model is (to be) drawn.
    pub model_wanted: bool,
    /// The sky model is missing or undecodable: no model is set and the box
    /// keeps the 2D path.
    pub model_missing: bool,
    /// A cross-fade to `fade_partner` is in progress.
    pub fading: bool,
    /// The box being faded to.
    pub fade_partner: Option<SkyboxKey>,
    /// Current fade amount (0..=255).
    pub fade: i32,
    /// Fade amount the current fade started from.
    fade_base: i32,
}

impl SkyBox {
    /// Build the skybox for `key` from its config type.
    #[must_use]
    pub fn new(types: &SkyTypes, key: SkyboxKey) -> Self {
        let t = types.skybox(key.0);
        let decors = t.decors.as_ref().map(|ids| {
            ids.iter()
                .map(|&id| SkyboxDecor::from_type(&types.decor(id)))
                .collect::<Vec<_>>()
        });
        // The sun is the decor at `sun_decor` when that index is in range (the
        // cache never has an out-of-range one).
        let sun = decors
            .as_ref()
            .filter(|d| t.sun_decor >= 0 && (t.sun_decor as usize) < d.len())
            .map(|_| t.sun_decor as usize);
        Self {
            key,
            material: t.material,
            decors,
            visible: Vec::new(),
            sun,
            origin: [key.1, key.2, key.3],
            fill: t.fill,
            model_id: t.model,
            sprite_size: 0,
            last_height: -1,
            model_wanted: false,
            model_missing: false,
            fading: false,
            fade_partner: None,
            fade: 0,
            fade_base: 0,
        }
    }

    /// Interpolate the fade from its start amount towards `target` by `t / 255`.
    pub fn fade_step(&mut self, t: i32, target: i32) {
        self.fade = (target - self.fade_base) * t / 255 + self.fade_base;
    }

    /// Finish the fade: back to no partner and no fade.
    pub fn end_fade(&mut self) {
        self.fading = false;
        self.fade_partner = None;
        self.fade = 0;
    }

    /// Per-frame update for this box alone (the caller recurses into the
    /// fade partner): re-place the decors when the viewport height changed.
    /// The decors' sprites are baked where the frame is resolved
    /// ([`crate::sky_frame::SkyCache::resolve`]).
    pub fn update(&mut self, height: i32, skyboxes_pref: i32) {
        if self.last_height != height {
            self.last_height = height;
            let mut size = highest_bit(height);
            if size > 512 {
                size = 512;
            }
            if size <= 0 {
                size = 1;
            }
            if self.sprite_size != size {
                self.sprite_size = size;
            }
            if let Some(decors) = self.decors.as_mut() {
                let mut keys = Vec::with_capacity(decors.len());
                self.visible.clear();
                for (i, decor) in decors.iter_mut().enumerate() {
                    if decor.select(self.origin[0], self.origin[1], self.origin[2], height) {
                        keys.push(decor.distance);
                        self.visible.push(i);
                    }
                }
                let n = keys.len() as i32;
                sort_by_key(&mut keys, &mut self.visible, 0, n - 1);
            }
        }
        // The GPU path always supports the 3D sky model.
        self.model_wanted = skyboxes_pref != 0 && self.model_id >= 0 && !self.model_missing;
    }
}

/// One layer of the sky plan, before the renderer maps it to pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum SkyLayer {
    /// A viewport fill; `blend` alpha-blends it instead of replacing.
    Fill { argb: u32, blend: bool },
    /// Clear colour and depth.
    Clear { rgb: i32 },
    /// The 2D material path: tiled tinted sprite quads.
    Material {
        key: SkyboxKey,
        material: i32,
        sprite_size: i32,
        /// Tint alpha (`255 - fade`).
        alpha: i32,
        /// First layer (fills the fog colour below a translucent
        /// texture).
        first: bool,
        /// Fog colour.
        fog: i32,
        pitch: i32,
        /// Yaw plus the horizontal scroll offset, wrapped to 14 bits.
        yaw: i32,
        fill: Option<SkyBoxFillMode>,
    },
    /// One decor's baked sprite, at the projected direction of the decor
    /// (see [`decor_centre`]); drawn after the box's fill, texture or model,
    /// farthest decor first.
    Decor {
        key: SkyboxKey,
        /// Index in the box's decor list.
        decor: usize,
        /// Tint alpha (`255 - fade`).
        alpha: i32,
        /// Drawn size in pixels (a square).
        size: i32,
        /// The decor's direction, of length 256.
        direction: [i32; 3],
        pitch: i32,
        yaw: i32,
        roll: i32,
    },
    /// The sky model under a rotation-only view.
    Model {
        key: SkyboxKey,
        pitch: i32,
        yaw: i32,
        roll: i32,
        /// The face-alpha fade applied over the model's own alphas.
        fade: i32,
    },
}

/// The camera's view of the sky: the horizontal scroll offset of the 2D
/// path, the pitch, yaw and roll (14-bit angles) and the fog colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SkyView {
    yaw_offset: i32,
    pitch: i32,
    yaw: i32,
    roll: i32,
    fog: i32,
}

/// The layers of one skybox, in draw order. `first` is the bottom box of a
/// fading pair (or the only one), `fade` the face-alpha fade of this box.
fn layers_for(sky: &SkyBox, view: SkyView, first: bool, fade: i32, out: &mut Vec<SkyLayer>) {
    let SkyView {
        yaw_offset,
        pitch,
        yaw,
        roll,
        fog,
    } = view;
    let alpha = 255 - fade;
    if !sky.model_wanted {
        let yaw = (yaw_offset + yaw) & 0x3FFF;
        if sky.material == -1 || sky.sprite_size == 0 {
            out.push(SkyLayer::Fill {
                argb: ((alpha as u32) << 24) | (fog as u32 & 0xFF_FFFF),
                blend: !first,
            });
        } else {
            out.push(SkyLayer::Material {
                key: sky.key,
                material: sky.material,
                sprite_size: sky.sprite_size,
                alpha,
                first,
                fog,
                pitch,
                yaw,
                fill: sky.fill,
            });
        }
    } else {
        // Only the bottom box clears; the model is drawn on top.
        if first {
            out.push(SkyLayer::Clear { rgb: fog });
        }
        out.push(SkyLayer::Model {
            key: sky.key,
            pitch,
            yaw,
            roll,
            fade,
        });
    }
    // The decors, farthest first (the visible list is nearest first).
    if let Some(decors) = sky.decors.as_ref() {
        for &index in sky.visible.iter().rev() {
            let decor = &decors[index];
            out.push(SkyLayer::Decor {
                key: sky.key,
                decor: index,
                alpha,
                size: decor.screen_size,
                direction: decor.direction,
                pitch,
                yaw,
                roll,
            });
        }
    }
}

/// The skybox cache plus the environment's current skybox.
#[derive(Debug, Default)]
pub struct SkyboxOwner {
    pub types: SkyTypes,
    pub boxes: BTreeMap<SkyboxKey, SkyBox>,
    /// Skybox of the current environment.
    pub current: Option<SkyboxKey>,
}

impl SkyboxOwner {
    #[must_use]
    pub fn new(types: SkyTypes) -> Self {
        Self {
            types,
            ..Self::default()
        }
    }

    /// Get or create the cached skybox for `key`.
    pub fn create(&mut self, key: SkyboxKey) -> SkyboxKey {
        if !self.boxes.contains_key(&key) {
            let sky = SkyBox::new(&self.types, key);
            self.boxes.insert(key, sky);
        }
        key
    }

    /// Adopt the current environment's skybox.
    pub fn select(&mut self, skybox: Option<crate::env::SkyboxRef>) {
        self.current = skybox.map(|s| self.create((s.kind, s.a, s.b, s.c)));
    }

    /// Skybox part of an environment fade: a new target environment starts a
    /// cross-fade from the current box.
    pub fn begin_fade(&mut self, to: Option<SkyboxKey>) {
        let Some(mut current) = self.current else {
            return;
        };
        if self.boxes[&current].fading {
            // A box already fading hands over to its fade partner (or to nothing).
            match self.boxes[&current].fade_partner {
                Some(partner) => {
                    current = partner;
                    self.current = Some(partner);
                }
                None => {
                    self.current = None;
                    return;
                }
            }
        }
        if Some(current) == to {
            return;
        }
        // Seed the fade from an in-progress fade of the target box.
        let partner_fade = to
            .and_then(|k| self.boxes.get(&k))
            .filter(|b| b.fading)
            .map(|b| b.fade);
        let sky = self.boxes.get_mut(&current).expect("current skybox cached");
        sky.fade_base = if sky.fading {
            sky.fade
        } else if let Some(f) = partner_fade {
            255 - f
        } else {
            0
        };
        sky.fading = true;
        sky.fade_partner = to;
        sky.fade = 0;
    }

    /// Skybox part of interpolating between two environments:
    /// `from`/`to` are the fade endpoints' boxes, `t` the fade progress.
    pub fn fade_sample(&mut self, from: Option<SkyboxKey>, to: Option<SkyboxKey>, t: f32) {
        if from == to {
            self.current = from;
            return;
        }
        let amount = (t * 255.0) as i32;
        match from {
            None => {
                self.current = to;
                if let Some(sky) = to.and_then(|k| self.boxes.get_mut(&k)) {
                    sky.fade_step(amount, 0);
                }
            }
            Some(k) => {
                self.current = Some(k);
                if let Some(sky) = self.boxes.get_mut(&k) {
                    sky.fade_step(amount, 255);
                }
            }
        }
    }

    /// Completion of an environment fade.
    pub fn end_fade(&mut self, to: Option<SkyboxKey>) {
        self.current = to;
        if let Some(sky) = to.and_then(|k| self.boxes.get_mut(&k)) {
            sky.end_fade();
        }
    }

    /// The box the environment settles on: the current box, or, while it
    /// fades, the box it fades to (`None`: no skybox).
    #[must_use]
    pub fn target(&self) -> Option<SkyboxKey> {
        let key = self.current?;
        let sky = self.boxes.get(&key)?;
        if sky.fading {
            sky.fade_partner
        } else {
            Some(key)
        }
    }

    /// Whether the current box is cross-fading towards another environment
    /// (a box installed without a fade is not: a teleport, a first
    /// environment).
    #[must_use]
    pub fn is_fading(&self) -> bool {
        self.current
            .and_then(|key| self.boxes.get(&key))
            .is_some_and(|sky| sky.fading)
    }

    /// Update the current skybox every interface cycle from the scene
    /// viewport component height and the skyboxes preference, recursing into
    /// the fade partner.
    pub fn update(&mut self, height: i32, skyboxes_pref: i32) {
        let Some(mut key) = self.current else {
            return;
        };
        let mut seen = Vec::new();
        loop {
            seen.push(key);
            let Some(sky) = self.boxes.get_mut(&key) else {
                return;
            };
            sky.update(height, skyboxes_pref);
            let Some(partner) = self.boxes[&key].fade_partner else {
                return;
            };
            if seen.contains(&partner) {
                return;
            }
            // The partner's fade ends before its own update.
            if let Some(p) = self.boxes.get_mut(&partner) {
                p.end_fade();
            }
            key = partner;
        }
    }

    /// The layers for the current skybox at the given camera angles and fog
    /// colour; the 2D scroll offset is `yaw_offset_steps << 3`. `None` means
    /// no skybox: the caller clears colour and depth to the fog colour.
    #[must_use]
    pub fn frame(
        &self,
        yaw_offset_steps: i32,
        pitch: i32,
        yaw: i32,
        roll: i32,
        fog: i32,
    ) -> Option<Vec<SkyLayer>> {
        let key = self.current?;
        let sky = self.boxes.get(&key)?;
        let view = SkyView {
            yaw_offset: yaw_offset_steps << 3,
            pitch,
            yaw,
            roll,
            fog,
        };
        let mut out = Vec::new();
        let mut fade = if sky.fading { sky.fade } else { 0 };
        match sky.fade_partner.and_then(|k| self.boxes.get(&k)) {
            None => {
                if sky.material == -1 {
                    out.push(SkyLayer::Fill {
                        argb: 0xFF00_0000 | (fog as u32 & 0xFF_FFFF),
                        blend: false,
                    });
                }
                layers_for(sky, view, true, fade, &mut out);
            }
            Some(partner) => {
                // The original orders the pair by an identity hash with no
                // stable value; the cache key order stands in for it.
                let (a, b) = if sky.key > partner.key {
                    fade = 255 - fade;
                    (partner, sky)
                } else {
                    (sky, partner)
                };
                layers_for(a, view, true, fade, &mut out);
                layers_for(b, view, false, 255 - fade, &mut out);
            }
        }
        Some(out)
    }
}

/// One screen quad of the 2D material path, in viewport pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlatQuad {
    /// A sprite quad tinted white at `alpha`.
    Sprite { rect: [i32; 4], alpha: i32 },
    /// A solid ARGB rectangle.
    Fill { rect: [i32; 4], argb: u32 },
}

/// The 2D sky sprite as drawn: pitch and yaw (14-bit), the sprite alpha, the
/// fill mode, and the ARGB of the sprite's first and last pixel (which fill
/// the space above and below a horizon strip).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlatSky {
    pub pitch: i32,
    pub yaw: i32,
    pub alpha: i32,
    pub fill: Option<SkyBoxFillMode>,
    pub edges: [u32; 2],
}

/// The pixel layout of the 2D sky path for a viewport
/// `(w, h)` at `(0, 0)` (the pass viewport supplies the offset). `first` /
/// `last` are the sprite's first and last ARGB pixels.
#[must_use]
pub fn flat_quads(viewport: [i32; 2], sky: FlatSky) -> Vec<FlatQuad> {
    let [w, h] = viewport;
    let FlatSky {
        pitch,
        yaw,
        alpha,
        fill,
        edges: [first, last],
    } = sky;
    let mut out = Vec::new();
    if h <= 0 {
        return out;
    }
    let mut scroll_y = h * pitch / -4096;
    let mut scroll_x = h * yaw / 4096 + (w - h) / 2;
    while scroll_x > h {
        scroll_x -= h;
    }
    while scroll_x < 0 {
        scroll_x += h;
    }
    if fill == Some(SkyBoxFillMode::Horizon) {
        let mut x = scroll_x - h;
        while x < w {
            out.push(FlatQuad::Sprite {
                rect: [x, scroll_y, h, h],
                alpha,
            });
            x += h;
        }
        if first & 0xFF00_0000 != 0 {
            out.push(FlatQuad::Fill {
                rect: [0, 0, w, scroll_y + 1],
                argb: first,
            });
        }
        if last & 0xFF00_0000 != 0 {
            out.push(FlatQuad::Fill {
                rect: [0, scroll_y + h, w, h - (scroll_y + h)],
                argb: last,
            });
        }
    } else {
        while scroll_y > h {
            scroll_y -= h;
        }
        while scroll_y < 0 {
            scroll_y += h;
        }
        let mut x = scroll_x - h;
        while x < w {
            let mut y = scroll_y - h;
            while y < h {
                out.push(FlatQuad::Sprite {
                    rect: [x, y, h, h],
                    alpha,
                });
                y += h;
            }
            x += h;
        }
    }
    out
}

/// The rotation-only view the sky model is drawn under, as a
/// `Matrix4x3`.
#[must_use]
pub fn model_view(pitch: i32, yaw: i32, roll: i32) -> crate::camera::Matrix4x3 {
    let mut m = crate::camera::Matrix4x3::translation(0.0, 0.0, 0.0);
    m.rotate_around_axis(0.0, -1.0, 0.0, crate::trig::radians(-yaw & 0x3FFF));
    m.rotate_around_axis(-1.0, 0.0, 0.0, crate::trig::radians(-pitch & 0x3FFF));
    m.rotate_around_axis(0.0, 0.0, -1.0, crate::trig::radians(-roll & 0x3FFF));
    m
}

/// The viewport pixel of a decor's `direction` under the sky's rotation-only
/// view (`None`: behind the camera). `projection` is the scene camera's
/// ([`crate::camera::SceneCamera::projection`]), `viewport` the scene
/// viewport in pixels.
#[must_use]
pub fn decor_centre(
    direction: [i32; 3],
    (pitch, yaw, roll): (i32, i32, i32),
    projection: [f32; 16],
    viewport: (i32, i32),
) -> Option<[f32; 2]> {
    let mut projection = projection;
    crate::camera::glx_flip_y(&mut projection);
    let cpu = crate::camera::CpuProjection::new(
        model_view(pitch, yaw, roll).to_entries(),
        projection,
        [0, 0, viewport.0, viewport.1],
    );
    let [x, y, depth] = cpu.project(direction.map(|v| v as f32));
    (depth >= 0.0 && x.is_finite() && y.is_finite()).then_some([x, y])
}

/// The per-face alpha for a skybox fade over the model's own alphas.
#[must_use]
pub fn faded_alpha(own: Option<u8>, fade: u8) -> u8 {
    match own {
        None => fade,
        Some(a) => (255 - (255 - i32::from(a)) * (255 - i32::from(fade)) / 255) as u8,
    }
}

#[cfg(test)]
mod tests;
