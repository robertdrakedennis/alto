//! The scene's draw-list entities (the opaque, transparent and pending
//! entity lists the draw lists are built from, in intrusive-list order). Split out of `draw` (Phase 2.8) so `model_lights` and
//! `rebuild` read them without the planner, breaking the `draw`/
//! `model_lights`/`rebuild` cycle; `draw` re-exports it.

use crate::scene::{EntityRef, Scene};

#[derive(Clone, Debug)]
pub struct DrawEntity {
    pub source: EntityRef,
    pub dynamic: bool,
    pub bounds: Option<[i32; 6]>,
    pub wall_type: i32,
    pub cylinder: Option<[i32; 6]>,
    pub position: Option<[f32; 3]>,
    pub precise_cylinder: Option<[f32; 7]>,
    /// Index in the draw list.
    pub id: i32,
    /// The scene list it came from: 0 opaque, 1 transparent, 2 pending.
    pub bucket: i32,
    /// What it is: 0 scenery, 1 wall, 2 wall decoration, 3 ground
    /// decoration, 4 temporary (player and other transient entities).
    pub kind: i32,
    /// The loc type id (the player index for a temporary).
    pub loc_id: i32,
    pub shape: i32,
    pub angle: i32,
    pub level: i32,
    pub occlude_level: i32,
    /// Position in scene fine units.
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Footprint in scene tiles: min x, max x, min z, max z.
    pub tiles: [i32; 4],
    pub overlay_height: i32,
    pub transparent: bool,
}

impl DrawEntity {
    /// The numeric fields in the order the draw trace records them: id,
    /// bucket, kind, loc, shape, angle, level, occlude level, x, y, z, the
    /// four footprint tiles, overlay height, transparent.
    pub fn words(&self) -> [i32; 17] {
        let [min_x, max_x, min_z, max_z] = self.tiles;
        [
            self.id,
            self.bucket,
            self.kind,
            self.loc_id,
            self.shape,
            self.angle,
            self.level,
            self.occlude_level,
            self.x,
            self.y,
            self.z,
            min_x,
            max_x,
            min_z,
            max_z,
            self.overlay_height,
            i32::from(self.transparent),
        ]
    }
}

/// Preserve intrusive-list traversal (Rust arenas store insertion order).
/// Overlay height: a static entity's overlay height is its model's minimum Y.
pub fn entities(scene: &mut Scene) -> Vec<DrawEntity> {
    let buckets = [
        scene.opaque.clone(),
        scene.transparent.clone(),
        scene.pending.clone(),
    ];
    let mut out = Vec::new();
    for (bucket, refs) in buckets.iter().enumerate() {
        for &source in refs.iter().rev() {
            let (kind, loc, shape, angle, level, occlude, x, y, z, bounds, model, dynamic) =
                match source {
                    EntityRef::Temporary(_) => continue,
                    EntityRef::Scenery(i) => {
                        let e = &mut scene.scenery[i];
                        (
                            0,
                            e.loc_id,
                            e.shape,
                            e.angle,
                            e.level,
                            e.occlude_level,
                            e.x,
                            e.y,
                            e.z,
                            Some([e.min_tx, e.max_tx, e.min_tz, e.max_tz]),
                            &mut e.model,
                            e.dynamic,
                        )
                    }
                    EntityRef::Wall(i) => {
                        let e = &mut scene.walls[i];
                        (
                            1,
                            e.loc_id,
                            e.shape,
                            e.angle,
                            e.level,
                            e.occlude_level,
                            e.x,
                            e.y,
                            e.z,
                            None,
                            &mut e.model,
                            e.dynamic,
                        )
                    }
                    EntityRef::WallDecor(i) => {
                        let e = &mut scene.wall_decors[i];
                        (
                            2,
                            e.loc_id,
                            e.shape,
                            e.angle,
                            e.level,
                            e.occlude_level,
                            e.x,
                            e.y,
                            e.z,
                            None,
                            &mut e.model,
                            e.dynamic,
                        )
                    }
                    EntityRef::GroundDecor(i) => {
                        let e = &mut scene.ground_decors[i];
                        (
                            3,
                            e.loc_id,
                            22,
                            e.angle,
                            e.level,
                            e.occlude_level,
                            e.x,
                            e.y,
                            e.z,
                            None,
                            &mut e.model,
                            e.dynamic,
                        )
                    }
                };
            let height = model.as_mut().map_or(0, |m| m.min_y());
            let cylinder = model
                .as_mut()
                .filter(|m| m.unique_count != 0)
                .map(|m| [x, y, z, m.min_y(), m.max_y(), m.horizontal_radius()]);
            let transparent = dynamic || model.as_ref().is_some_and(|m| m.has_transparency);
            let [min_x, max_x, min_z, max_z] = bounds.unwrap_or([
                x >> scene.size,
                x >> scene.size,
                z >> scene.size,
                z >> scene.size,
            ]);
            let box_bounds = model.as_mut().map(|m| {
                let (x0, x1, y0, y1, z0, z1) = (
                    m.min_x(),
                    m.max_x(),
                    m.min_y(),
                    m.max_y(),
                    m.min_z(),
                    m.max_z(),
                );
                [
                    x.wrapping_add(x0),
                    y.wrapping_add(y0),
                    z.wrapping_add(z0),
                    x1.wrapping_sub(x0),
                    y1.wrapping_sub(y0),
                    z1.wrapping_sub(z0),
                ]
            });
            let wall_type = match source {
                EntityRef::Wall(i) => scene.walls[i].wall_type,
                _ => 0,
            };
            out.push(DrawEntity {
                source,
                dynamic,
                position: None,
                precise_cylinder: None,
                bounds: box_bounds,
                wall_type,
                cylinder: cylinder.map(|mut c| {
                    if let EntityRef::WallDecor(i) = source {
                        c[0] += scene.wall_decors[i].offset_x;
                        c[2] += scene.wall_decors[i].offset_z;
                    }
                    c
                }),
                id: out.len() as i32,
                bucket: bucket as i32,
                kind,
                loc_id: loc as i32,
                shape,
                angle,
                level,
                occlude_level: occlude,
                x,
                y,
                z,
                tiles: [min_x, max_x, min_z, max_z],
                overlay_height: height,
                transparent,
            });
        }
    }
    out
}
