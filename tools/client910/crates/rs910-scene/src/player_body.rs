//! Live player body model build and per-actor pose.
//! A live actor owns pose readiness and the previous model key. Cached bases
//! remain undeformed; each call returns the body in actor-local coordinates.
use crate::{
    animation_assets::AnimationAssets,
    billboard::BillboardStore,
    cache::Pack,
    entities910::Player,
    gpumodel::GpuModel,
    particle::EmitterStore,
    player_model::{Inputs, Models},
    player_pose,
    protocol910::{
        bas_types::Bas,
        terrain::{self, Terrain},
    },
    texture::MaterialStore,
};
use anyhow::{Context, Result};

pub struct Body {
    pub model: GpuModel,
    pub min_y: i32,
    pub height: i32,
    pub ground: [i32; 3],
}
pub struct Resources<'a> {
    pub pack: &'a Pack,
    pub types: Inputs<'a>,
    pub materials: &'a MaterialStore,
    pub billboards: &'a BillboardStore,
    pub emitters: &'a EmitterStore,
    /// The NPC type store plus the local player's variable state for NPC-transformed
    /// players; `None` leaves a transformed body unbuilt.
    pub npcs: Option<NpcTypes<'a>>,
}

/// The NPC model owner a transformed player's body model
/// delegates to.
#[derive(Clone, Copy)]
pub struct NpcTypes<'a> {
    pub store: &'a crate::config::NpcStore,
    /// Variable read: varbit (`true`) or varp.
    pub vars: &'a dyn Fn(bool, i32) -> Option<i32>,
}
/// The render footprint uses a 240-unit margin.
pub fn tile_bounds(e: &Player) -> [i32; 4] {
    let radius = e.size.wrapping_sub(1).wrapping_mul(256).wrapping_add(240);
    let (x, z) = (e.fine_x as i32, e.fine_z as i32);
    [
        x.wrapping_sub(radius) >> 9,
        x.wrapping_add(radius) >> 9,
        z.wrapping_sub(radius) >> 9,
        z.wrapping_add(radius) >> 9,
    ]
    .map(|v| v as i16 as i32)
}
pub fn ground(
    e: &Player,
    scene: Option<&Terrain>,
    width: i32,
    depth: i32,
    limits: [i32; 2],
) -> Result<[i32; 3]> {
    let bounds = tile_bounds(e);
    let reference = [
        bounds[0].wrapping_add(bounds[1]) >> 1,
        bounds[2].wrapping_add(bounds[3]) >> 1,
    ];
    let (sin, cos) = (
        crate::trig::sin(e.angle & 16383),
        crate::trig::cos(e.angle & 16383),
    );
    let mut h = [0i32; 4];
    for (i, (x, z)) in [
        (-width / 2, -depth / 2),
        (width / 2, -depth / 2),
        (-width / 2, depth / 2),
        (width / 2, depth / 2),
    ]
    .into_iter()
    .enumerate()
    {
        let dx = sin.wrapping_mul(z).wrapping_add(cos.wrapping_mul(x)) >> 14;
        let dz = cos.wrapping_mul(z).wrapping_sub(sin.wrapping_mul(x)) >> 14;
        h[i] = terrain::footprint_height(
            scene,
            (e.fine_x as i32).wrapping_add(dx),
            (e.fine_z as i32).wrapping_add(dz),
            reference,
            e.level,
        )
        .map_err(|error| anyhow::anyhow!("player footprint height: {error:?}"))?;
    }
    let angle = |a: i32, b: i32, limit: i32| {
        let mut v = ((a as f64).atan2(b as f64) * 2607.5945876176133) as i32 & 16383;
        if v != 0 && limit != 0 {
            if v > 8192 {
                v = v.max(16384i32.wrapping_sub(limit));
            } else {
                v = v.min(limit);
            }
        }
        v
    };
    Ok([
        angle(
            h[0].min(h[1]).wrapping_sub(h[2].min(h[3])),
            depth,
            limits[0],
        ),
        angle(
            h[0].min(h[2]).wrapping_sub(h[1].min(h[3])),
            width,
            limits[1],
        ),
        (h[0].wrapping_add(h[3]).min(h[1].wrapping_add(h[2])) >> 1).wrapping_sub(e.motion.y as i32),
    ])
}
/// The tilt an actor stands with on the terrain: its body animation set's
/// footprint when it has one, else the actor's own size, as
/// `[pitch, roll, height]`. Whether the model is tilted by it is the
/// caller's (only a tilting set does).
pub fn stance_ground(e: &Player, b: &Bas, scene: Option<&Terrain>) -> Result<[i32; 3]> {
    if b.tilt_x != 0 || b.tilt_z != 0 {
        ground(
            e,
            scene,
            b.tilt_x,
            b.tilt_z,
            [b.tilt_scale_x, b.tilt_scale_z],
        )
    } else {
        ground(
            e,
            scene,
            e.size.wrapping_shl(9),
            e.size.wrapping_shl(9),
            [0, 0],
        )
    }
}
/// Exact post-pose tilt order, including the unmodified pre-tilt overlay height.
pub fn finish(
    mut model: GpuModel,
    e: &Player,
    b: &Bas,
    scene: Option<&Terrain>,
    cycle: i32,
) -> Result<Body> {
    let min_y = model.min_y();
    let height = model.height();
    model.radius();
    let (roll, pitch) = (e.motion.roll[0], e.motion.pitch[0]);
    if roll != 0 || pitch != 0 {
        let pivot = model.min_y() / 2;
        model.translate(0, -pivot, 0);
        model.rotate_z(roll & 16383);
        model.rotate_x(pitch & 16383);
        model.translate(0, pivot, 0);
    }
    let tilted = b.tilt_x != 0 || b.tilt_z != 0;
    let g = stance_ground(e, b, scene)?;
    if tilted {
        if g[0] != 0 {
            model.rotate_x(g[0]);
        }
        if g[1] != 0 {
            model.rotate_z(g[1]);
        }
        if g[2] != 0 {
            model.translate(0, g[2], 0);
        }
    }
    if tint_active(e, cycle) {
        model.tint(e.tint[0], e.tint[1], e.tint[2], e.tint[3] & 255);
    }
    Ok(Body {
        model,
        min_y,
        height,
        ground: g,
    })
}
fn tint_active(e: &Player, cycle: i32) -> bool {
    e.tint[3] != 0 && cycle >= e.tint[4] && cycle < e.tint[5]
}
/// What a body build is asked for: the client cycle (tint windows), the
/// render capability flags and whether the idle stance replaces walking.
#[derive(Clone, Copy, Debug)]
pub struct BodyRequest {
    pub cycle: i32,
    pub flags: i32,
    pub use_idle: bool,
}

pub fn build(
    models: &mut Models,
    assets: &mut AnimationAssets,
    r: &Resources,
    e: &mut Player,
    scene: Option<&Terrain>,
    request: BodyRequest,
) -> Result<Option<Body>> {
    let BodyRequest {
        cycle,
        flags,
        use_idle,
    } = request;
    let Some(a) = e.appearance.model.as_ref() else {
        return Ok(None);
    };
    let fallback = Bas::default();
    let b = if e.appearance.bas == -1 {
        &fallback
    } else {
        r.types.bases.get(&e.appearance.bas).context("player BAS")?
    };
    let mut flags = flags;
    if b.tilt_x != 0 || b.tilt_z != 0 || b.roll_target != 0 || b.pitch_target != 0 {
        flags |= 7;
    }
    if tint_active(e, cycle) {
        flags |= 0x80000;
    }
    let mut main = (e.animation.main.sequence.is_some() && e.animation.main.delay == 0)
        .then_some(&mut e.animation.main);
    let mut walk = (e.actor.walk.node.sequence.is_some()
        && !use_idle
        && !(e.actor.walk.idle && main.is_some()))
    .then_some(&mut e.actor.walk.node);
    let p = player_pose::prepare(
        assets,
        r.pack,
        &mut e.animation.overlays,
        main.as_deref_mut(),
        walk.as_deref_mut(),
    )?;
    let seq = main
        .as_ref()
        .and_then(|n| assets.sequences.get(&n.id()))
        .cloned();
    flags |= p.flags;
    if e.actor
        .wear_angles
        .as_ref()
        .is_some_and(|a| a.iter().any(|&a| a != -1))
    {
        flags |= 0x20;
    }
    if a.npc != -1 {
        // The transmog NPC type builds the body; its own `bas` is passed through every
        // `multinpc` hop for the slot transforms.
        let Some(npcs) = r.npcs else {
            anyhow::bail!("NPC-transformed player body without NPC configs");
        };
        let base = crate::npc_type_model::list(npcs.store, a.npc as u32)?;
        let bas_id = base.bas;
        let Some(npc) = crate::npc_type_model::resolve(npcs.store, base, npcs.vars)? else {
            return Ok(None);
        };
        let key = (npc.id, bas_id);
        // The transmog body is requested with the overlay/main/walk render flags (not the
        // player-only wear-angle 0x20 above, which the NPC path never adds) and rebuilt with
        // the union when the cached model lacks one.
        let requested = crate::npc_type_model::sequenced_flags(p.flags);
        let cached = models.npc_bodies.get(&key).cloned();
        let rebuild = crate::npc_type_model::rebuild_flags(cached.as_ref(), requested);
        let mut model = if let (None, Some(model)) = (rebuild, cached) {
            model
        } else {
            let assets = crate::npc_type_model::Assets {
                pack: r.pack,
                materials: r.materials,
                billboards: r.billboards,
                emitters: r.emitters,
                bases: Some(r.types.bases),
                detail: models.detail,
            };
            let Some(model) = crate::npc_type_model::body(
                &npc,
                None,
                bas_id,
                rebuild.unwrap_or(requested),
                &assets,
            )?
            else {
                return Ok(None);
            };
            models.npc_bodies.insert(key, model.clone());
            model
        };
        // Overlay/wear/main animation on the per-call copy with the
        // same BAS bind matrices, then the resolved type's resize.
        player_pose::apply(
            &mut model,
            p,
            player_pose::WearPose {
                bas: r.types.bases.get(&bas_id),
                angles: e.actor.wear_angles.as_deref(),
                yaw: e.angle & 16383,
            },
            assets,
            r.pack,
            player_pose::ActiveNodes { main, walk },
        )?;
        crate::npc_type_model::resize(&npc, &mut model);
        return finish(model, e, b, scene, cycle).map(Some);
    }
    let Some(mut model) = models.body(
        r.pack,
        a,
        seq.as_ref(),
        &r.types,
        &crate::gpumodel::ModelStores {
            materials: r.materials,
            billboards: r.billboards,
            emitters: r.emitters,
        },
        crate::player_model::BodyBuild {
            flags,
            cache: true,
            last_key: &mut e.actor.model_key,
        },
    )?
    else {
        return Ok(None);
    };
    // The bind matrices use the appearance's BAS, which is independent
    // of the player's current stance selection.
    player_pose::apply(
        &mut model,
        p,
        player_pose::WearPose {
            bas: r.types.bases.get(&a.bas),
            angles: e.actor.wear_angles.as_deref(),
            yaw: e.angle & 16383,
        },
        assets,
        r.pack,
        player_pose::ActiveNodes { main, walk },
    )?;
    finish(model, e, b, scene, cycle).map(Some)
}
