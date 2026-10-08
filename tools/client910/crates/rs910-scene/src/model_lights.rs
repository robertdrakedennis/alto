//! Static point lights: the per-tile light table, per-entity light selection (up to four
//! lights per entity), flicker and the group colour/scale fades. Static entities cache their
//! first selection.
use crate::{
    draw_entity::DrawEntity,
    env::StaticLight,
    scene::{EntityRef, Scene},
};

#[derive(Clone)]
pub struct ModelLights {
    /// Static installation identity, shared by immutable frame copies.
    grid_identity: std::sync::Arc<()>,
    pub lights: Vec<StaticLight>,
    /// The flicker intensity times the fade scale.
    pub intensities: Vec<f32>,
    /// Colour/scale fade state, one per light.
    fades: Vec<LightFade>,
    tiles: Vec<[u16; 4]>,
    nx: usize,
    nz: usize,
    cached: Vec<Option<Vec<usize>>>,
}

/// The fade state of a light. Times are logic-clock milliseconds (`cycle * 20`) standing in for
/// the monotonic clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightFade {
    /// The light's base colour.
    base_colour: i32,
    /// Colour: current, fade start and target.
    colour: i32,
    colour_from: i32,
    colour_target: i32,
    /// Colour fade remaining and total milliseconds.
    colour_remaining: i32,
    colour_duration: i32,
    /// Time of the last colour update.
    colour_time: i64,
    /// Scale: current, fade start and target.
    scale: f32,
    scale_from: f32,
    scale_target: f32,
    /// Scale fade remaining and total milliseconds.
    scale_remaining: i32,
    scale_duration: i32,
    /// Time of the last scale update.
    scale_time: i64,
}
impl LightFade {
    /// A light at rest at `colour` with full scale.
    fn new(colour: i32) -> Self {
        Self {
            base_colour: colour,
            colour,
            colour_from: colour,
            colour_target: colour,
            colour_remaining: 0,
            colour_duration: 0,
            colour_time: 0,
            scale: 1.0,
            scale_from: 1.0,
            scale_target: 1.0,
            scale_remaining: 0,
            scale_duration: 0,
            scale_time: 0,
        }
    }
    /// Start a colour fade towards `colour` (`-1` restores the base colour).
    fn set_colour(&mut self, colour: i32, duration: i32, now: i64) {
        self.colour_target = if colour == -1 {
            self.base_colour
        } else {
            colour
        };
        self.colour_from = self.colour;
        self.colour_duration = duration;
        self.colour_remaining = duration;
        self.colour_time = now;
    }
    /// Start a scale fade towards `scale` (negative restores 1.0).
    fn set_scale(&mut self, scale: f32, duration: i32, now: i64) {
        self.scale_target = if scale < 0.0 { 1.0 } else { scale };
        self.scale_from = self.scale;
        self.scale_duration = duration;
        self.scale_remaining = duration;
        self.scale_time = now;
    }
    /// Advance both fades to `now`.
    fn update(&mut self, now: i64) {
        if self.colour_target != self.colour {
            self.colour_remaining =
                (i64::from(self.colour_remaining) - (now - self.colour_time)) as i32;
            self.colour = if self.colour_remaining > 0 {
                blend_colour(
                    self.colour_from,
                    self.colour_target,
                    ((self.colour_duration - self.colour_remaining) as f32
                        / self.colour_duration as f32
                        * 255.0) as i32,
                )
            } else {
                self.colour_target
            };
            self.colour_time = now;
        }
        if self.scale_target == self.scale {
            return;
        }
        self.scale_remaining = (i64::from(self.scale_remaining) - (now - self.scale_time)) as i32;
        self.scale = if self.scale_remaining > 0 {
            (self.scale_duration - self.scale_remaining) as f32 / self.scale_duration as f32
                * (self.scale_target - self.scale_from)
                + self.scale_from
        } else {
            self.scale_target
        };
        self.scale_time = now;
    }
}

impl ModelLights {
    /// Identity of the installed tiled light definitions, independent of fades.
    pub fn grid_identity(&self) -> &std::sync::Arc<()> {
        &self.grid_identity
    }
    pub fn reset_temporary(&mut self, base: usize, count: usize) {
        self.cached.truncate(base);
        self.cached.resize(count, None);
    }
    pub fn new(scene: &Scene, lights: &[StaticLight], count: usize) -> Self {
        let mut this = Self {
            grid_identity: std::sync::Arc::new(()),
            lights: lights.to_vec(),
            intensities: lights
                .iter()
                .map(|l| crate::light_animation::intensity(l, 0, true))
                .collect(),
            fades: lights.iter().map(|l| LightFade::new(l.colour)).collect(),
            tiles: vec![[0; 4]; scene.max_level * scene.max_x * scene.max_z],
            nx: scene.max_x,
            nz: scene.max_z,
            cached: vec![None; count],
        };
        for (id, l) in lights.iter().take(65253).enumerate() {
            let low = if l.below { 0 } else { l.level as usize };
            let high = if l.above {
                scene.max_level - 1
            } else {
                l.level as usize
            };
            let z0 = (l.z - l.radius + scene.half_tile_size) >> scene.size;
            let z1 =
                ((l.z + l.radius - scene.half_tile_size) >> scene.size).min(scene.max_z as i32 - 1);
            for level in low..=high {
                for z in z0.max(0)..=z1 {
                    let run = l.span_runs[(z - z0) as usize];
                    let x0 =
                        ((l.x - l.radius + scene.half_tile_size) >> scene.size) + (run >> 8) as i32;
                    let x1 = (x0 + (run & 255) as i32 - 1).min(scene.max_x as i32 - 1);
                    for x in x0.max(0)..=x1 {
                        let tile =
                            &mut this.tiles[(level * this.nx + x as usize) * this.nz + z as usize];
                        if let Some(slot) = tile.iter_mut().find(|v| **v == 0) {
                            *slot = (id + 1) as u16;
                        }
                    }
                }
            }
        }
        this
    }
    fn tile(&self, level: usize, x: i32, z: i32) -> Vec<usize> {
        // Normal-scene callers stay in bounds; the empty result is for the free viewer camera.
        if x < 0 || z < 0 || x as usize >= self.nx || z as usize >= self.nz {
            return vec![];
        }
        self.tiles[(level * self.nx + x as usize) * self.nz + z as usize]
            .iter()
            .take_while(|&&v| v != 0)
            .map(|&v| v as usize - 1)
            .collect()
    }
    pub fn collect(&mut self, scene: &Scene, e: &DrawEntity, eye: [i32; 2]) -> &[usize] {
        let id = e.id as usize;
        if self.cached[id].is_none() {
            let level = e.level as usize;
            let (mut x, mut z) = (e.x >> scene.size, e.z >> scene.size);
            let mut selected = Vec::new();
            match e.source {
                EntityRef::Temporary(i) => {
                    let b = scene.temporary[i].bounds;
                    'outer: for x in b[0].max(0)..=b[1].min(self.nx as i32 - 1) {
                        for z in b[2].max(0)..=b[3].min(self.nz as i32 - 1) {
                            for light in self.tile(level, x, z) {
                                if !selected.contains(&light) {
                                    selected.push(light);
                                    if selected.len() == 4 {
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                }
                EntityRef::Scenery(i) => {
                    let s = &scene.scenery[i];
                    'outer: for x in s.min_tx.max(0)..=s.max_tx.min(self.nx as i32 - 1) {
                        for z in s.min_tz.max(0)..=s.max_tz.min(self.nz as i32 - 1) {
                            for light in self.tile(level, x, z) {
                                if !selected.contains(&light) {
                                    selected.push(light);
                                    if selected.len() == 4 {
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    if s.diag != 0 {
                        let (dx, dz) = (s.min_tx - eye[0], s.min_tz - eye[1]);
                        let (az, bx) = if s.diag == 1 {
                            if dz > dx {
                                (-1, 1)
                            } else {
                                (1, -1)
                            }
                        } else if dz > -dx {
                            (-1, -1)
                        } else {
                            (1, 1)
                        };
                        let a = self.tile(level, s.min_tx, s.min_tz + az);
                        let b = self.tile(level, s.min_tx + bx, s.min_tz);
                        // The index advances after a removal, so the entry that shifts into the
                        // removed slot is skipped.
                        let mut at = 0;
                        while at < selected.len() {
                            if !a.contains(&selected[at]) && !b.contains(&selected[at]) {
                                selected.remove(at);
                            }
                            at += 1;
                        }
                    }
                }
                EntityRef::Wall(i) => {
                    let t = scene.walls[i].wall_type;
                    let mut direction = if eye[0] == x {
                        1
                    } else if eye[0] < x {
                        2
                    } else {
                        0
                    };
                    direction += if eye[1] == z {
                        3
                    } else if eye[1] > z {
                        6
                    } else {
                        0
                    };
                    if t & [19, 55, 38, 155, 255, 110, 137, 205, 76][direction] == 0 {
                        match t {
                            1 => x -= 1,
                            4 => x += 1,
                            8 => z -= 1,
                            2 => z += 1,
                            16 => {
                                x -= 1;
                                z += 1
                            }
                            32 => {
                                x += 1;
                                z += 1
                            }
                            128 => {
                                x -= 1;
                                z -= 1
                            }
                            64 => {
                                x += 1;
                                z -= 1
                            }
                            _ => {}
                        }
                    }
                    selected = self.tile(level, x, z);
                }
                _ => selected = self.tile(level, x, z),
            }
            self.cached[id] = Some(selected);
        }
        self.cached[id].as_deref().unwrap()
    }
}

impl ModelLights {
    /// Per-frame update: the flicker intensity is multiplied by the current fade scale, then the
    /// colour and scale fades advance.
    pub fn animate(&mut self, cycle: i32, flickering: bool) {
        let now = i64::from(cycle) * 20;
        for ((light, value), fade) in self
            .lights
            .iter()
            .zip(&mut self.intensities)
            .zip(&mut self.fades)
        {
            *value = crate::light_animation::intensity(light, cycle, !flickering) * fade.scale;
            fade.update(now);
        }
    }

    /// Every static light of the group starts a colour fade to `colour` over `duration_ms`.
    pub fn point_light_colour(&mut self, group: i32, duration_ms: i32, colour: i32, cycle: i32) {
        let now = i64::from(cycle) * 20;
        for (light, fade) in self.lights.iter().zip(&mut self.fades) {
            if light.group == group && group != -1 {
                fade.set_colour(colour, duration_ms, now);
            }
        }
    }

    /// Every static light of the group starts a scale fade: the percentage becomes a scale
    /// (`-1` restores 1.0).
    pub fn point_light_intensity(
        &mut self,
        group: i32,
        duration_ms: i32,
        percent: i32,
        cycle: i32,
    ) {
        let now = i64::from(cycle) * 20;
        let scale = if percent < 0 {
            -1.0
        } else {
            percent as f32 / 100.0
        };
        for (light, fade) in self.lights.iter().zip(&mut self.fades) {
            if light.group == group && group != -1 {
                fade.set_scale(scale, duration_ms, now);
            }
        }
    }

    /// A rebuilt scene's lights inherit the first light of their group's colour, scale and
    /// remaining fades from the previous scene.
    pub fn inherit_primary(&mut self, previous: &ModelLights, cycle: i32) {
        let now = i64::from(cycle) * 20;
        let mut primary = std::collections::BTreeMap::new();
        for (light, fade) in previous.lights.iter().zip(&previous.fades) {
            if light.group != -1 {
                primary.entry(light.group).or_insert(*fade);
            }
        }
        for (light, fade) in self.lights.iter().zip(&mut self.fades) {
            let Some(p) = primary.get(&light.group) else {
                continue;
            };
            fade.colour = p.colour;
            fade.set_colour(p.colour_target, p.colour_remaining, now);
            fade.scale = p.scale;
            fade.set_scale(p.scale_target, p.scale_remaining, now);
        }
    }
    pub fn actor_parameters(
        &self,
        e: &DrawEntity,
        matrix: &crate::actor_matrix::Matrix,
    ) -> [[f32; 4]; 8] {
        let mut out = self.parameters(e);
        let inverse = matrix.inverse();
        for (slot, &id) in self.selected(e.id as usize).iter().enumerate() {
            let l = &self.lights[id];
            let p = inverse.point(l.x as f32, l.y as f32, l.z as f32);
            out[slot][..3].copy_from_slice(&p);
        }
        out
    }
    pub fn selected(&self, id: usize) -> &[usize] {
        self.cached[id].as_deref().unwrap_or(&[])
    }
    /// The tile light table's size `(levels, x tiles, z tiles)` (read-only
    /// accessor for a backend with its own light lookup; NXT renderer plan
    /// M4).
    #[must_use]
    pub fn grid_size(&self) -> (usize, usize, usize) {
        let levels = self.tiles.len().checked_div(self.nx * self.nz).unwrap_or(0);
        (levels, self.nx, self.nz)
    }
    /// The raw slots of tile `(level, x, z)` as [`Self::new`] filled them
    /// (the per-tile light table): up to four light ids plus one
    /// in light order, `0` for an empty slot; all empty outside the table
    /// (read-only accessor; NXT renderer plan M4).
    #[must_use]
    pub fn tile_slots(&self, level: usize, x: usize, z: usize) -> [u16; 4] {
        if x >= self.nx || z >= self.nz {
            return [0; 4];
        }
        self.tiles
            .get((level * self.nx + x) * self.nz + z)
            .copied()
            .unwrap_or([0; 4])
    }
    /// Light `id`'s current colour (its group colour
    /// fade applied; read-only accessor, NXT renderer plan M4). `None` for
    /// an id outside the list.
    #[must_use]
    pub fn colour(&self, id: usize) -> Option<i32> {
        self.fades.get(id).map(|f| f.colour)
    }
    /// Light parameters for a model: model-local positions, radius reciprocal and RGB scaled by
    /// the current light intensity.
    pub fn parameters(&self, e: &DrawEntity) -> [[f32; 4]; 8] {
        let mut out = [[0.; 4]; 8];
        let c = e.cylinder.unwrap_or([e.x, e.y, e.z, 0, 0, 0]);
        for (slot, &id) in self.selected(e.id as usize).iter().enumerate() {
            let l = &self.lights[id];
            out[slot] = [
                (l.x - c[0]) as f32,
                (l.y - c[1]) as f32,
                (l.z - c[2]) as f32,
                1. / l.radius.wrapping_mul(l.radius) as f32,
            ];
            let scale = self.intensities[id] / 255.;
            let colour = self.fades[id].colour;
            out[4 + slot] = [
                ((colour >> 16) & 255) as f32 * scale,
                ((colour >> 8) & 255) as f32 * scale,
                (colour & 255) as f32 * scale,
                1.,
            ];
        }
        out
    }
}

fn blend_colour(from: i32, to: i32, amount: i32) -> i32 {
    let amount = amount.clamp(0, 255);
    let inverse = 255 - amount;
    let channel = |shift: u32| {
        ((((from >> shift) & 255) * inverse + ((to >> shift) & 255) * amount) / 255) & 255
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[cfg(test)]
#[test]
fn animated_intensity_reaches_model_and_actor_parameters() {
    let light = StaticLight {
        level: 0,
        above: false,
        below: false,
        x: 0,
        y: 0,
        z: 0,
        radius: 1024,
        colour: 0x804020,
        flicker: 2,
        phase: 0,
        wave: 1,
        offset: 0,
        amplitude: 2048,
        speed: 2048,
        group: -1,
        span_runs: vec![1],
    };
    let mut lights = ModelLights {
        grid_identity: std::sync::Arc::new(()),
        lights: vec![light],
        intensities: vec![1.],
        fades: vec![LightFade::new(0x804020)],
        tiles: vec![],
        nx: 0,
        nz: 0,
        cached: vec![Some(vec![0])],
    };
    let entity = DrawEntity {
        source: EntityRef::Temporary(0),
        dynamic: false,
        bounds: None,
        wall_type: 0,
        cylinder: None,
        position: None,
        precise_cylinder: None,
        id: 0,
        bucket: 0,
        kind: 0,
        loc_id: 0,
        shape: 0,
        angle: 0,
        level: 0,
        occlude_level: 0,
        x: 0,
        y: 0,
        z: 0,
        tiles: [0; 4],
        overlay_height: 0,
        transparent: false,
    };
    for (cycle, flicker) in [(0, true), (13, true), (13, false), (25, true)] {
        lights.animate(cycle, flicker);
        let model = lights.parameters(&entity);
        let scale = lights.intensities[0] / 255.;
        assert_eq!(model[4], [128. * scale, 64. * scale, 32. * scale, 1.]);
        let actor = lights.actor_parameters(&entity, &crate::actor_matrix::Matrix::default());
        assert_eq!(model, actor);
    }
    lights.animate(0, true);
    assert_eq!(lights.intensities[0], 0.5);
    lights.animate(0, false);
    assert_eq!(lights.intensities[0], 1.);
}

/// A group colour fade interpolates from the current
/// colour over its duration and `-1` restores the base; an intensity
/// percentage scales the flicker value from the next frame and `-1` (255)
/// restores 1.0; a rebuilt scene inherits the group's fade state.
#[cfg(test)]
#[test]
fn point_light_fades_follow_the_light_model() {
    let light = StaticLight {
        level: 0,
        above: false,
        below: false,
        x: 0,
        y: 0,
        z: 0,
        radius: 1024,
        colour: 0x000000,
        flicker: 0,
        phase: 0,
        wave: 1,
        offset: 0,
        amplitude: 2048,
        speed: 2048,
        group: 3,
        span_runs: vec![1],
    };
    let mut lights = ModelLights {
        grid_identity: std::sync::Arc::new(()),
        lights: vec![light.clone()],
        intensities: vec![1.],
        fades: vec![LightFade::new(0)],
        tiles: vec![],
        nx: 0,
        nz: 0,
        cached: vec![Some(vec![0])],
    };
    let flicker = |cycle| crate::light_animation::intensity(&light, cycle, false);
    lights.point_light_colour(3, 200, 0xff00ff, 10);
    lights.point_light_intensity(3, 0, 50, 10);
    lights.animate(15, true);
    // 100 of 200 ms elapsed: halfway, in blend_colour's 0..255 steps.
    assert_eq!(lights.fades[0].colour, blend_colour(0, 0xff00ff, 127));
    // The scale applied this frame was the pre-update 1.0.
    assert_eq!(lights.intensities[0], flicker(15));
    lights.animate(20, true);
    assert_eq!(lights.fades[0].colour, 0xff00ff);
    assert_eq!(lights.intensities[0], flicker(20) * 0.5);
    // Other groups are untouched.
    lights.point_light_colour(4, 0, 0x123456, 20);
    lights.animate(21, true);
    assert_eq!(lights.fades[0].colour, 0xff00ff);
    let mut rebuilt = ModelLights {
        grid_identity: std::sync::Arc::new(()),
        lights: vec![light.clone()],
        intensities: vec![1.],
        fades: vec![LightFade::new(0)],
        tiles: vec![],
        nx: 0,
        nz: 0,
        cached: vec![Some(vec![0])],
    };
    rebuilt.inherit_primary(&lights, 21);
    assert_eq!(rebuilt.fades[0].colour, 0xff00ff);
    assert_eq!(rebuilt.fades[0].scale, 0.5);
    // -1 restores the base colour and a 1.0 scale.
    lights.point_light_colour(3, 0, -1, 21);
    lights.point_light_intensity(3, 0, -1, 21);
    lights.animate(22, true);
    lights.animate(23, true);
    assert_eq!(lights.fades[0].colour, 0);
    assert_eq!(lights.intensities[0], flicker(23));
}
