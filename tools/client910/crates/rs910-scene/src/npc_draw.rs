//! What an NPC's scene draw adds around its body model: the fade-in of a new
//! arrival, the settling of the body onto raised ground, the ground shadow's
//! shape and the picking options of its type.

use crate::{
    animation_assets::AnimationAssets,
    billboard::BillboardStore,
    cache::Pack,
    config::Npc,
    entities910::{animation_state::Node, npc_custom::Custom, Player},
    gpumodel::GpuModel,
    npc_type_model,
    particle::EmitterStore,
    player_body, player_pose,
    player_shadow::{ShadowSources, Shadows, Shape},
    protocol910::{bas_types::Bas, terrain::Terrain},
    texture::MaterialStore,
};
use anyhow::Result;
use std::collections::BTreeMap;

/// How far above the ground an NPC is drawn, in model units.
pub const LIFT: i32 = 20;

/// The vertical offset an NPC (and its shadow and effects) is drawn with: the
/// fixed lift plus the smoothed offset of a ground decoration under it.
#[must_use]
pub fn lift(decoration_offset: i32) -> i32 {
    (-LIFT).wrapping_sub(decoration_offset)
}

/// One step of the smoothing that eases an actor up onto the height of a
/// ground decoration (`decor`) and back down when it leaves: a tenth of the
/// remaining distance per draw.
#[must_use]
pub fn settle_decoration_offset(offset: i32, decor: Option<i32>) -> i32 {
    let remaining = match decor {
        Some(height) => offset.wrapping_sub(height),
        None => offset,
    };
    (offset as f32 - remaining as f32 / 10.) as i32
}

/// The fade-in of an NPC that entered the scene: transparent at first
/// (`alpha` 255), opaque (0) once the type's duration has passed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fade {
    pub alpha: i32,
    /// The cycle the fade began.
    pub start: i32,
}

impl Fade {
    /// The transparency this cycle's draw applies to every face, or `None`
    /// when the NPC is opaque. Ends the fade once `duration` cycles have
    /// passed since it began.
    pub fn step(&mut self, cycle: i32, duration: i32) -> Option<u8> {
        if self.alpha == 0 {
            return None;
        }
        if duration <= 0 || cycle >= self.start.wrapping_add(duration) {
            self.alpha = 0;
            return None;
        }
        self.alpha = 255 - cycle.wrapping_sub(self.start).wrapping_mul(255) / duration;
        Some(self.alpha as u8)
    }
}

/// The shape of an NPC's ground shadow, or `None` when the type casts none.
/// `default_texture` is the graphics defaults' shadow material and its
/// transparency; `textures` is the textures preference. A material, from the
/// type or the defaults, is used only with textures on; otherwise the shadow
/// is the type's rings.
#[must_use]
pub fn shadow_shape(base: &Npc, default_texture: (i32, i32), textures: bool) -> Shape {
    let (mut material, mut alpha) = default_texture;
    if base.spotshadow_texture > -1 {
        material = base.spotshadow_texture;
        alpha = base.spotshadow_texture_alpha;
    }
    if material > -1 && textures {
        Shape::Texture {
            material: material as i16,
            alpha: alpha as i8,
        }
    } else {
        Shape::Rings {
            size: i32::from(base.size),
            colours: base.spotshadow_colours,
            alpha: base.spotshadow_trans,
        }
    }
}

/// The picking options of an NPC type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PickOptions {
    /// Whether the NPC can be picked at all.
    pub active: bool,
    /// The cuboid that stands in for the model, `[min x, min y, min z, max x,
    /// max y, max z]`.
    pub clickbox: Option<[i32; 6]>,
    /// How many times the model's box is widened for picking.
    pub size_shift: u32,
}

impl PickOptions {
    #[must_use]
    pub fn of(base: &Npc) -> Self {
        Self {
            active: base.active,
            clickbox: base.clickbox,
            size_shift: base.picksizeshift.clamp(0, 30) as u32,
        }
    }
}

/// How the type of an NPC changes the way it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    /// The random colour shift of this NPC (hue, saturation, luminance and
    /// weight) when its type allows one.
    pub antimacro: Option<[i32; 4]>,
    /// The shape of its ground shadow when it casts one.
    pub shadow: Option<Shape>,
    /// The height overhead elements are drawn at instead of the model's own;
    /// `-1` for the model's.
    pub overlay_height: i32,
    pub pick: PickOptions,
}

/// The preferences and defaults an NPC's look depends on.
#[derive(Clone, Copy, Debug)]
pub struct LookInputs {
    /// The character shadows preference.
    pub shadows: bool,
    /// The textures preference.
    pub textures: bool,
    /// The graphics defaults' shadow material and its transparency.
    pub default_shadow_texture: (i32, i32),
}

impl Look {
    /// From the type the packet named (`base`) and the type its `multinpc`
    /// selection resolved to (`resolved`); the colour shift, the shadow's
    /// shape and the picking come from the first, whether the shadow is cast
    /// and the overhead height from the second. `seeds` are this NPC's
    /// random colour shift. Without types (no config yet) nothing changes.
    #[must_use]
    pub fn new(
        types: Option<(&Npc, &Npc)>,
        bas: Option<&Bas>,
        seeds: [i32; 4],
        inputs: &LookInputs,
    ) -> Self {
        let Some((base, resolved)) = types else {
            return Self {
                antimacro: None,
                shadow: None,
                overlay_height: -1,
                pick: PickOptions {
                    active: true,
                    ..Default::default()
                },
            };
        };
        let casts = bas.is_none_or(|bas| bas.casts_shadow);
        Self {
            antimacro: base.antimacro.then_some(seeds),
            shadow: (inputs.shadows && resolved.spotshadow && casts)
                .then(|| shadow_shape(base, inputs.default_shadow_texture, inputs.textures)),
            overlay_height: if resolved.overlayheight != -1 {
                resolved.overlayheight
            } else {
                base.overlayheight
            },
            pick: PickOptions::of(base),
        }
    }
}

/// The model stores an NPC is built from.
pub struct Sources<'a> {
    pub pack: &'a Pack,
    pub materials: &'a MaterialStore,
    pub billboards: &'a BillboardStore,
    pub emitters: &'a EmitterStore,
    /// The loaded BAS types.
    pub bases: Option<&'a BTreeMap<i32, Bas>>,
    /// Model detail flags.
    pub detail: i32,
}

/// The cached bodies and shadows of the scene's NPCs.
#[derive(Default)]
pub struct Cache {
    /// Base bodies by type, model detail and customisation.
    bodies: rs910_core::soft_cache::SoftMap<(u32, i32, i64), GpuModel>,
    shadows: Shadows,
}

impl Cache {
    /// Age the cached bodies by one clean (see `rs910_core::soft_cache`).
    pub fn clean(&mut self, age: u32) {
        self.bodies.clean(age);
    }

    /// Drop the bodies nothing has drawn for a while; how many went.
    pub fn clear_soft(&mut self) -> usize {
        self.bodies.clear_soft()
    }
}

/// Everything one NPC's draw reads.
pub struct Spec<'a> {
    /// The type its `multinpc` selection resolved to.
    pub config: &'a Npc,
    /// The body customisation (NPC_INFO mask 0x400).
    pub custom: Option<&'a Custom>,
    /// The body animation set id and type.
    pub bas: i32,
    pub bas_type: Option<&'a Bas>,
    /// Its actor state: tilt, tint and where it stands.
    pub path: &'a Player,
    pub node: Node,
    pub walk: Option<Node>,
    pub overlays: Vec<Option<Node>>,
    pub wear_angles: Option<&'a [i32]>,
    /// The yaw it faces.
    pub angle: i32,
    pub look: &'a Look,
    /// The animation the shadow follows.
    pub shadow_node: Option<Node>,
    pub fade: Fade,
    /// The fade-in duration of the resolved type.
    pub fade_in: i32,
    /// The ground decoration offset after this draw's smoothing.
    pub decoration_offset: i32,
    pub terrain: Option<&'a Terrain>,
    /// The client cycle (tint and fade windows).
    pub cycle: i32,
}

/// One NPC as drawn: its body and shadow in scene-local space around its
/// position, and the state the draw leaves behind.
pub struct Drawn {
    pub body: GpuModel,
    pub shadow: Option<GpuModel>,
    /// The model's own height before any tilt, and the height overhead
    /// elements use.
    pub min_y: i32,
    pub overlay_height: i32,
    pub ground: [i32; 3],
    pub fade: Fade,
    pub node: Node,
    pub walk: Option<Node>,
    pub overlays: Vec<Option<Node>>,
}

/// Where a drawn NPC stands, for the clickbox picking places.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub position: [f32; 3],
    /// The actor rotation (its yaw).
    pub rotation: [f32; 4],
    pub lift: i32,
}

/// The pick entry of a drawn NPC `index`. Its models are already turned in
/// model space and lifted, so the draw matrix is the plain translation. A type
/// with a clickbox is picked by that cuboid instead of the model, placed with
/// the actor's own matrix; an inactive type cannot be picked.
pub fn pickable(
    index: usize,
    body: &mut GpuModel,
    place: Option<(&Placement, &PickOptions)>,
    depth: i32,
    frame: &crate::player_picking::Frame,
) -> crate::scene_player_pick::PickablePlayer {
    use crate::scene_player_pick as pick;
    let position = place.map_or([0.0; 3], |(place, _)| place.position);
    let options = place.map(|(_, options)| *options).unwrap_or_default();
    let (bounds, screen_bounds, matrix) = match (options.clickbox, place) {
        (Some(clickbox), Some((place, _))) => {
            let matrix = crate::actor_matrix::Matrix::actor(
                place.rotation,
                place.position,
                place.lift as f32,
            );
            let draw_mvp = crate::camera::multiply(&matrix.entries(), &frame.vp);
            let capsule = pick::cuboid_screen_bounds(
                clickbox.map(|v| v as f32),
                draw_mvp,
                frame.projection,
                frame.screen,
            );
            let [min_x, min_y, min_z, max_x, max_y, max_z] = clickbox;
            (
                pick::ModelBounds {
                    min: [min_x, min_y, min_z],
                    max: [max_x, max_y, max_z],
                },
                capsule,
                draw_mvp,
            )
        }
        _ => {
            let raw = pick::ModelBounds {
                min: [body.min_x(), body.min_y(), body.min_z()],
                max: [body.max_x(), body.max_y(), body.max_z()],
            };
            let matrix = crate::actor_matrix::Matrix::actor([0.0, 0.0, 0.0, 1.0], position, 0.0);
            let draw_mvp = crate::camera::multiply(&matrix.entries(), &frame.vp);
            let capsule = pick::screen_bounds(
                raw,
                body.horizontal_radius(),
                draw_mvp,
                frame.projection,
                frame.screen,
            );
            (
                pick::pick_bounds(raw, options.size_shift),
                capsule,
                draw_mvp,
            )
        }
    };
    pick::PickablePlayer {
        id: pick::PlayerPickId {
            pid: index as i32,
            generation: 0,
        },
        bounds,
        screen_bounds: Some(screen_bounds),
        projected_depth: depth,
        matrix,
        active: place.is_none() || options.active,
    }
}

impl Cache {
    /// Builds the NPC's body on its animation, tilt, tint, fade and colour
    /// shift, and its ground shadow; `None` when a model of it is not
    /// available yet.
    pub fn draw(
        &mut self,
        assets: &mut AnimationAssets,
        src: &Sources<'_>,
        spec: Spec<'_>,
    ) -> Result<Option<Drawn>> {
        let Spec {
            config,
            custom,
            bas,
            bas_type,
            path,
            mut node,
            mut walk,
            mut overlays,
            wear_angles,
            angle,
            look,
            shadow_node,
            mut fade,
            fade_in,
            decoration_offset,
            terrain,
            cycle,
        } = spec;
        let stance = bas_type.cloned().unwrap_or_default();
        let key = (
            config.id,
            src.detail,
            npc_type_model::body_cache_salt(custom),
        );
        let fade_alpha = fade.step(cycle, fade_in);
        let tint_active = path.tint[3] != 0 && cycle >= path.tint[4] && cycle < path.tint[5];
        let use_main = node.sequence.is_some() && node.delay == 0;
        // The active overlays, main and walk nodes widen the requested flags
        // with their render flags before the cache lookup, so the cached body
        // carries the label groups the animation moves.
        let prepared = player_pose::prepare(
            assets,
            src.pack,
            &mut overlays,
            use_main.then_some(&mut node),
            walk.as_mut(),
        );
        // The capabilities the draw needs beyond the animation's: the tilt
        // (vertex moves), an active tint mask and the fade's transparency.
        let mut wanted = prepared.as_ref().map_or(0, |prepared| prepared.flags);
        if stance.tilt_x != 0
            || stance.tilt_z != 0
            || stance.roll_target != 0
            || stance.pitch_target != 0
        {
            wanted |= 7;
        }
        if tint_active {
            wanted |= 0x80000;
        }
        if fade_alpha.is_some() {
            wanted |= 0x100;
        }
        let requested = npc_type_model::sequenced_flags(wanted);
        let cached = self.bodies.get(&key).cloned();
        let rebuild = npc_type_model::rebuild_flags(cached.as_ref(), requested);
        let mut model = if let (None, Some(model)) = (rebuild, cached) {
            model
        } else {
            let custom = custom.map(|c| npc_type_model::body_customisation(config, c));
            let assets = npc_type_model::Assets {
                pack: src.pack,
                materials: src.materials,
                billboards: src.billboards,
                emitters: src.emitters,
                bases: src.bases,
                detail: src.detail,
            };
            let Some(model) = npc_type_model::body(
                config,
                custom.as_ref(),
                bas,
                rebuild.unwrap_or(requested),
                &assets,
            )?
            else {
                return Ok(None);
            };
            self.bodies.insert(key, model.clone());
            model
        };
        // Overlays, wear angles and the main/walk pair (blended when both are
        // active) on the per-call copy, the same order the player model uses.
        let posed = prepared.and_then(|prepared| {
            model.has_transparency |= prepared.flags & 0x100 != 0;
            player_pose::apply(
                &mut model,
                prepared,
                player_pose::WearPose {
                    bas: bas_type,
                    angles: wear_angles,
                    yaw: angle & 16383,
                },
                assets,
                src.pack,
                player_pose::ActiveNodes {
                    main: use_main.then_some(&mut node),
                    walk: walk.as_mut(),
                },
            )
        });
        if let Err(error) = posed {
            rs910_core::log_repeat::warn_repeated!("NPC {} animation: {error:#}", config.id);
        }
        npc_type_model::resize(config, &mut model);
        // The body tilts with its animation set and the ground it stands on;
        // the active tint mask follows. Without terrain under the footprint
        // it stands level.
        let (mut model, min_y, ground) =
            if player_body::stance_ground(path, &stance, terrain).is_ok() {
                let body = player_body::finish(model, path, &stance, terrain, cycle)?;
                (body.model, body.min_y, body.ground)
            } else {
                let min_y = model.min_y();
                (model, min_y, [0; 3])
            };
        if let Some(alpha) = fade_alpha {
            model.set_alpha(alpha);
            model.has_transparency = true;
        }
        if let Some([hue, saturation, luminence, weight]) = look.antimacro {
            model.tint(hue, saturation, luminence, weight);
        }
        // The ground shadow is built from the body before it turns to face
        // its way.
        let mut shadow = match look.shadow {
            Some(shape) => {
                let mut shadow_node = shadow_node;
                Some(self.shadows.build(
                    ShadowSources {
                        pack: src.pack,
                        materials: src.materials,
                        billboards: src.billboards,
                        emitters: src.emitters,
                    },
                    assets,
                    &mut model,
                    shape,
                    ground,
                    shadow_node.as_mut(),
                )?)
            }
            None => None,
        };
        let lift = lift(decoration_offset);
        model.rotate_y_keep_normals(angle);
        model.translate(0, lift, 0);
        if let Some(shadow) = shadow.as_mut() {
            shadow.rotate_y_keep_normals(angle);
            shadow.translate(0, lift, 0);
        }
        Ok(Some(Drawn {
            body: model,
            shadow,
            min_y,
            overlay_height: if look.overlay_height == -1 {
                min_y
            } else {
                look.overlay_height
            },
            ground,
            fade,
            node,
            walk,
            overlays,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_runs_from_transparent_to_opaque_over_its_duration() {
        let mut fade = Fade {
            alpha: 255,
            start: 100,
        };
        assert_eq!(fade.step(100, 10), Some(255));
        assert_eq!(fade.step(105, 10), Some(128));
        assert_eq!(fade.step(109, 10), Some(26));
        assert_eq!(fade.step(110, 10), None);
        assert_eq!(fade.alpha, 0);
        // Finished: stays opaque whatever the cycle.
        assert_eq!(fade.step(101, 10), None);
    }

    #[test]
    fn a_type_without_a_fade_is_opaque_at_once() {
        let mut fade = Fade {
            alpha: 255,
            start: 7,
        };
        assert_eq!(fade.step(7, 0), None);
        assert_eq!(fade.alpha, 0);
    }
}

/// The draw of real cache NPCs on a synthetic slope: fade, shadow, lift,
/// tilt and colour shift, each observed on the model the scene receives.
#[cfg(test)]
mod scene_tests {
    use super::*;
    use crate::{animation_assets::AnimationAssets, config::decode_npc};

    const HANS: u32 = 0;
    const CYCLE: i32 = 1000;
    /// The tile the NPC stands on in the synthetic terrain.
    const TILE: i32 = 40;

    struct World {
        pack: Pack,
        materials: MaterialStore,
        billboards: BillboardStore,
        emitters: EmitterStore,
        assets: AnimationAssets,
        cache: Cache,
    }

    fn world() -> World {
        let pack = crate::test_support::require_pack("client.npc.config.js5");
        World {
            materials: MaterialStore::load(&pack).unwrap(),
            billboards: BillboardStore::load(&pack).unwrap(),
            emitters: EmitterStore::load(&pack).unwrap(),
            assets: AnimationAssets::load(&pack).unwrap(),
            cache: Cache::default(),
            pack,
        }
    }

    fn config(world: &World, id: u32) -> Npc {
        let files = world.pack.read_group("npc.config", id >> 7).unwrap();
        decode_npc(id, &files[&(id & 127)]).unwrap()
    }

    /// A terrain rising `slope` units per tile eastwards.
    fn hillside(slope: i32) -> Terrain {
        let mut terrain = Terrain::new(104, 104).unwrap();
        for level in 0..4 {
            for x in 0..=104 {
                for z in 0..=104 {
                    let point = terrain.point(level, x, z);
                    terrain.heights[point] = -(x as i32) * slope;
                }
            }
        }
        terrain
    }

    fn standing(angle: i32) -> Player {
        Player {
            fine_x: (TILE * 512 + 256) as f32,
            fine_z: (TILE * 512 + 256) as f32,
            angle,
            ..Default::default()
        }
    }

    fn look(shadow: Option<Shape>) -> Look {
        Look {
            antimacro: None,
            shadow,
            overlay_height: -1,
            pick: PickOptions::default(),
        }
    }

    fn draw(
        world: &mut World,
        config: &Npc,
        path: &Player,
        look: &Look,
        bas: Option<&Bas>,
        terrain: Option<&Terrain>,
        fade: Fade,
    ) -> Drawn {
        let sources = Sources {
            pack: &world.pack,
            materials: &world.materials,
            billboards: &world.billboards,
            emitters: &world.emitters,
            bases: None,
            detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
        };
        world
            .cache
            .draw(
                &mut world.assets,
                &sources,
                Spec {
                    config,
                    custom: None,
                    bas: -1,
                    bas_type: bas,
                    path,
                    node: Node::default(),
                    walk: None,
                    overlays: vec![],
                    wear_angles: None,
                    angle: path.angle,
                    look,
                    shadow_node: None,
                    fade,
                    fade_in: 20,
                    decoration_offset: 0,
                    terrain,
                    cycle: CYCLE,
                },
            )
            .unwrap()
            .expect("the cache has Hans's models")
    }

    fn alphas(model: &GpuModel) -> Vec<u8> {
        model.face_alpha[..model.face_count as usize]
            .iter()
            .map(|&a| a as u8)
            .collect()
    }

    #[test]
    fn a_new_arrival_fades_in_and_is_solid_afterwards() {
        let mut w = world();
        let hans = config(&w, HANS);
        let path = standing(0);
        let look = look(None);
        let fresh = Fade {
            alpha: 255,
            start: CYCLE,
        };
        let start = draw(&mut w, &hans, &path, &look, None, None, fresh);
        assert!(alphas(&start.body).iter().all(|&a| a == 255));
        assert!(
            start.body.has_transparency,
            "a fading body is drawn as translucent"
        );
        // Half way (duration 20): every face at the interpolated transparency.
        let mut half = fresh;
        half.start = CYCLE - 10;
        let mid = draw(&mut w, &hans, &path, &look, None, None, half);
        assert!(alphas(&mid.body).iter().all(|&a| a == 128));
        assert_eq!(mid.fade.alpha, 128);
        // Past the duration: as solid as the unfaded body, and it stays so.
        let mut done = fresh;
        done.start = CYCLE - 20;
        let solid = draw(&mut w, &hans, &path, &look, None, None, done);
        let plain = draw(&mut w, &hans, &path, &look, None, None, Fade::default());
        assert_eq!(solid.fade.alpha, 0);
        assert_eq!(alphas(&solid.body), alphas(&plain.body));
    }

    #[test]
    fn the_body_stands_above_the_ground_and_its_shadow_lies_beneath_it() {
        let mut w = world();
        let hans = config(&w, HANS);
        let path = standing(4096);
        let rings = Shape::Rings {
            size: 1,
            colours: [0, 0],
            alpha: [160, 240],
        };
        let plain = draw(
            &mut w,
            &hans,
            &path,
            &look(None),
            None,
            None,
            Fade::default(),
        );
        let shadowed = draw(
            &mut w,
            &hans,
            &path,
            &look(Some(rings)),
            None,
            None,
            Fade::default(),
        );
        assert!(plain.shadow.is_none());
        let mut shadow = shadowed.shadow.expect("shadow");
        let mut body = shadowed.body;
        // Model y grows downwards: the lift raises the feet above 0.
        assert_eq!(
            body.max_y(),
            plain.body.clone().max_y(),
            "the shadow changes nothing of the body"
        );
        assert!(body.max_y() <= -LIFT + 1, "the feet stand above the ground");
        assert_eq!(shadow.min_y(), shadow.max_y(), "the shadow is flat");
        assert_eq!(shadow.max_y(), -LIFT, "on the same lift as the body");
        assert!(
            shadow.face_alpha.iter().any(|&a| a as u8 != 0),
            "translucent"
        );
    }

    #[test]
    fn a_tilting_animation_set_leans_the_body_on_a_slope_and_a_plain_one_does_not() {
        let mut w = world();
        let hans = config(&w, HANS);
        let path = standing(0);
        let hill = hillside(64);
        let flat_ground = hillside(0);
        let flat = draw(
            &mut w,
            &hans,
            &path,
            &look(None),
            None,
            Some(&flat_ground),
            Fade::default(),
        );
        let level = draw(
            &mut w,
            &hans,
            &path,
            &look(None),
            None,
            Some(&hill),
            Fade::default(),
        );
        let tilting = Bas {
            tilt_x: 512,
            tilt_z: 512,
            ..Default::default()
        };
        let leaning = draw(
            &mut w,
            &hans,
            &path,
            &look(None),
            Some(&tilting),
            Some(&hill),
            Fade::default(),
        );
        // The slope is measured either way (effects placed on the body follow it) ...
        assert_eq!(flat.ground[..2], [0, 0]);
        assert_ne!(level.ground[..2], [0, 0]);
        // ... but only a tilting set moves the body.
        assert_eq!(flat.body.vy, level.body.vy);
        assert_ne!(flat.body.vy, leaning.body.vy);
        assert_eq!(
            flat.min_y, leaning.min_y,
            "the height overhead elements use is the untilted one"
        );
    }

    #[test]
    fn a_clickbox_replaces_the_model_in_picking_and_an_inactive_type_is_not_picked() {
        use crate::scene_player_pick::capsule_hit;
        let mut w = world();
        let hans = config(&w, HANS);
        let path = standing(0);
        let mut drawn = draw(
            &mut w,
            &hans,
            &path,
            &look(None),
            None,
            None,
            Fade::default(),
        );
        let position = [path.fine_x, 0.0, path.fine_z];
        // A camera looking at the NPC.
        let mut camera =
            crate::camera::SceneCamera::new([position[0] as i32, 0, position[2] as i32]);
        camera.viewport = (512, 334);
        let frame = crate::player_picking::Frame::new(&camera, [0, 0], 0, [0, 0, 512, 334]);
        let place = Placement {
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            lift: -LIFT,
        };
        let solid = PickOptions {
            active: true,
            clickbox: None,
            size_shift: 0,
        };
        let by_model = pickable(7, &mut drawn.body, Some((&place, &solid)), 0, &frame);
        assert_eq!(by_model.id.pid, 7);
        assert!(by_model.active);
        let model_capsule = by_model.screen_bounds.unwrap();
        let middle = [
            (model_capsule.a[0] + model_capsule.b[0]) / 2,
            (model_capsule.a[1] + model_capsule.b[1]) / 2,
        ];
        assert!(capsule_hit(model_capsule, middle, [0, 0]));
        // A clickbox three times as wide as tall is hit far beside the body.
        let boxed = PickOptions {
            clickbox: Some([-1500, -200, -1500, 1500, 0, 1500]),
            ..solid
        };
        let by_box = pickable(7, &mut drawn.body, Some((&place, &boxed)), 0, &frame);
        let box_capsule = by_box.screen_bounds.unwrap();
        assert!(box_capsule.radius > model_capsule.radius);
        let aside = [middle[0] + model_capsule.radius * 2, middle[1]];
        assert!(!capsule_hit(model_capsule, aside, [0, 0]));
        assert!(capsule_hit(box_capsule, aside, [0, 0]));
        // A type that cannot be interacted with is never picked.
        let dead = PickOptions {
            active: false,
            ..solid
        };
        assert!(!pickable(7, &mut drawn.body, Some((&place, &dead)), 0, &frame).active);
    }

    #[test]
    fn the_colour_shift_of_a_type_recolours_the_body() {
        let mut w = world();
        let hans = config(&w, HANS);
        let path = standing(0);
        let mut shifted = look(None);
        shifted.antimacro = Some([33, 4, 17, 11]);
        let plain = draw(
            &mut w,
            &hans,
            &path,
            &look(None),
            None,
            None,
            Fade::default(),
        );
        let tinted = draw(&mut w, &hans, &path, &shifted, None, None, Fade::default());
        assert_eq!(plain.body.face_count, tinted.body.face_count);
        assert_ne!(plain.body.face_colour, tinted.body.face_colour);
        assert_eq!(plain.body.vx, tinted.body.vx, "only the colours change");
    }

    #[test]
    fn a_type_that_casts_no_shadow_or_has_no_overlay_height_reads_its_own_options() {
        let hans = decode_npc(HANS, &[0]).unwrap();
        let mut no_shadow = hans.clone();
        no_shadow.spotshadow = false;
        no_shadow.overlayheight = 300;
        let inputs = LookInputs {
            shadows: true,
            textures: true,
            default_shadow_texture: (-1, 0),
        };
        let casts = Look::new(Some((&hans, &hans)), None, [1, 2, 3, 4], &inputs);
        assert!(matches!(casts.shadow, Some(Shape::Rings { size: 1, .. })));
        assert_eq!(casts.antimacro, Some([1, 2, 3, 4]));
        assert_eq!(casts.overlay_height, -1);
        // A morph that resolves to a type without a shadow casts none, and
        // its overhead height wins.
        let morphed = Look::new(Some((&hans, &no_shadow)), None, [1, 2, 3, 4], &inputs);
        assert!(morphed.shadow.is_none());
        assert_eq!(morphed.overlay_height, 300);
        // Textures on and a default material: the material replaces the rings.
        let textured = LookInputs {
            default_shadow_texture: (4321, 100),
            ..inputs
        };
        assert_eq!(
            Look::new(Some((&hans, &hans)), None, [0; 4], &textured).shadow,
            Some(Shape::Texture {
                material: 4321,
                alpha: 100
            })
        );
    }
}
