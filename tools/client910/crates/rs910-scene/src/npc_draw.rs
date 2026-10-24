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
