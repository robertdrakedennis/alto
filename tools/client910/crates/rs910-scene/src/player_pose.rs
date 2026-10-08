//! Player animation preparation and application.
//! Main/walk blends share a single fixed-point rounding pass.
use crate::{
    actor_matrix::Matrix,
    animation_assets::{AnimationAssets, Filter, Pose},
    cache::Pack,
    entities910::animation_state::Node,
    gpumodel::{GpuModel, Transform},
    protocol910::bas_types::Bas,
};
use anyhow::{Context, Result};

fn reset(normals: bool) -> Transform {
    Transform {
        kind: 0,
        labels: vec![],
        value: [0; 3],
        mask: 65535,
        angle: 0,
        normals,
        direct_pivot: false,
    }
}
/// Both nodes must load successfully, even when a missing blend mask makes the pose
/// use only the first animation. Secondary tween compatibility uses the first base.
pub fn dual(
    assets: &mut AnimationAssets,
    pack: &Pack,
    a: &mut Node,
    b: &mut Node,
) -> Result<Vec<Transform>> {
    let sa = assets
        .sequences
        .get(&a.id())
        .context("main blend sequence")?
        .clone();
    let sb = assets
        .sequences
        .get(&b.id())
        .context("walk blend sequence")?
        .clone();
    let first = assets.actor_pose(pack, a, Filter::default())?;
    if first.flags == 0 {
        return Ok(vec![]);
    }
    let second = assets.actor_pose(pack, b, Filter::default())?;
    if second.flags == 0 {
        return Ok(vec![]);
    }
    let both_skeletal = sa.frame_ids.is_none() && sb.frame_ids.is_none();
    let normals = sa.extra || (!both_skeletal && sb.extra);
    let mask = assets.groups.get(&sa.blend).and_then(|g| g.mask.clone());
    let filter = Filter {
        blend: mask.as_deref().map(|m| (m, false)),
        normals: Some(normals),
        wrap_skeletal: mask.is_some(),
        ..Default::default()
    };
    let mut ops = assets.actor_pose(pack, a, filter)?.transforms;
    if let Some(mask) = mask {
        ops.push(reset(normals));
        ops.extend(
            assets
                .actor_pose(
                    pack,
                    b,
                    Filter {
                        blend: Some((&mask, true)),
                        normals: Some(normals),
                        wrap_skeletal: true,
                        next_base: first.base_identity,
                        ..Default::default()
                    },
                )?
                .transforms,
        );
    }
    Ok(ops)
}
/// Inputs and outputs for the player body model's animation preparation.
/// Keep stages separate so cache construction can use every requested capability.
pub struct Prepared {
    pub flags: i32,
    overlays: Vec<(usize, Pose)>,
    pub main: Option<Pose>,
    pub walk: Option<Pose>,
}
pub fn prepare(
    assets: &mut AnimationAssets,
    pack: &Pack,
    overlays: &mut [Option<Node>],
    main: Option<&mut Node>,
    walk: Option<&mut Node>,
) -> Result<Prepared> {
    let mut p = Prepared {
        flags: 0,
        overlays: vec![],
        main: None,
        walk: None,
    };
    for (slot, n) in overlays.iter_mut().enumerate() {
        if let Some(n) = n {
            let classic = assets
                .sequences
                .get(&n.id())
                .is_some_and(|s| s.frame_ids.is_some());
            let pose = assets.actor_pose(
                pack,
                n,
                Filter {
                    part_mask: 1i32.wrapping_shl(slot as u32),
                    ..Default::default()
                },
            )?;
            p.flags |= pose.flags;
            if classic {
                p.overlays.push((slot, pose));
            }
        }
    }
    if let Some(n) = main {
        let pose = assets.actor_pose(pack, n, Filter::default())?;
        p.flags |= pose.flags;
        p.main = Some(pose);
    }
    if let Some(n) = walk {
        let pose = assets.actor_pose(pack, n, Filter::default())?;
        p.flags |= pose.flags;
        p.walk = Some(pose);
    }
    Ok(p)
}
/// How the worn parts sit: the animation set whose slot transforms bind
/// them, the per-slot wear angles and the entity's yaw.
#[derive(Clone, Copy)]
pub struct WearPose<'a> {
    pub bas: Option<&'a Bas>,
    pub angles: Option<&'a [i32]>,
    pub yaw: i32,
}

/// The main and walk animation nodes that are playing; a blend needs both.
pub struct ActiveNodes<'a> {
    pub main: Option<&'a mut Node>,
    pub walk: Option<&'a mut Node>,
}

/// Wear inverse-bind, overlays, relative yaw, bind, main/walk order.
/// Each overlay rounds separately; the final combined main/walk pass rounds once.
pub fn apply(
    model: &mut GpuModel,
    p: Prepared,
    wear_pose: WearPose<'_>,
    assets: &mut AnimationAssets,
    pack: &Pack,
    nodes: ActiveNodes<'_>,
) -> Result<()> {
    let WearPose {
        bas: b,
        angles,
        yaw,
    } = wear_pose;
    let ActiveNodes { main, walk } = nodes;
    let wear = angles.is_some_and(|a| a.iter().any(|&a| a != -1));
    let matrices = b.and_then(|b| b.slot_transforms.as_ref()).map(|ts| {
        ts.iter()
            .map(|t| {
                t.as_ref()
                    .filter(|t| t.iter().any(|&v| v != 0))
                    .map(|t| Matrix::bas(t))
            })
            .collect::<Vec<_>>()
    });
    if wear {
        if let Some(matrices) = &matrices {
            for slot in 0..angles.unwrap().len() {
                if let Some(m) = matrices.get(slot).context("BAS wear matrix slot")? {
                    model.apply_part_matrix(m, 1i32.wrapping_shl(slot as u32), true);
                }
            }
        }
    }
    for (_, pose) in p.overlays {
        if pose.flags != 0 {
            model.apply_animation(&pose.transforms);
        }
    }
    if wear {
        for (slot, &angle) in angles.unwrap().iter().enumerate() {
            if angle != -1 {
                let m = Matrix::axis(
                    0.,
                    1.,
                    0.,
                    crate::trig::radians(angle.wrapping_sub(yaw) & 16383),
                );
                model.apply_part_matrix(&m, 1i32.wrapping_shl(slot as u32), false);
            }
        }
        if let Some(matrices) = &matrices {
            for slot in 0..angles.unwrap().len() {
                if let Some(m) = matrices.get(slot).context("BAS wear matrix slot")? {
                    model.apply_part_matrix(m, 1i32.wrapping_shl(slot as u32), false);
                }
            }
        }
    }
    match (main, walk) {
        (Some(a), Some(b)) => {
            let ops = dual(assets, pack, a, b)?;
            if !ops.is_empty() {
                model.apply_animation(&ops);
            }
        }
        (Some(_), None) => {
            if let Some(p) = p.main.filter(|p| p.flags != 0) {
                model.apply_animation(&p.transforms);
            }
        }
        (None, Some(_)) => {
            if let Some(p) = p.walk.filter(|p| p.flags != 0) {
                model.apply_animation(&p.transforms);
            }
        }
        _ => {}
    }
    Ok(())
}
