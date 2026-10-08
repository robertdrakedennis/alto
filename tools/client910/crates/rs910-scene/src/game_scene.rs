//! `Game`'s scene-graph reads: the renderer/CPU map agreement check before a
//! map is installed and the loc snapshot capture over the built scene. Split out of `game_runtime` (Phase 2.8) so the game state does not
//! depend on the scene layer (`rebuild`, `scene`, `map`); mapped to
//! rs910-scene. Since `game_runtime` moved to rs910-game (Phase 3.2) the
//! orphan rule makes it an extension trait, [`GameScene`].

use crate::{entity_runtime::PreparedMap, game_runtime::Game, protocol910::zone_state::Snapshot};

/// `Game`'s scene-graph reads (see the module docs); callers import it.
pub trait GameScene {
    /// See `impl GameScene for Game`.
    fn verify_scene(
        &self,
        map: &PreparedMap,
        scene: &crate::rebuild::Rebuild,
    ) -> anyhow::Result<()>;
    /// See `impl GameScene for Game`.
    fn capture_scene(&mut self, scene: &crate::scene::Scene);
}

impl GameScene for Game {
    /// Refuse to acknowledge/install a renderer built for different dimensions,
    /// origins, heights or bridge flags. PreparedMap's generation check then makes
    /// the CPU transition atomic with respect to delayed map work.
    fn verify_scene(
        &self,
        map: &PreparedMap,
        scene: &crate::rebuild::Rebuild,
    ) -> anyhow::Result<()> {
        let request = self
            .runtime
            .map_request
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no map installation requested"))?;
        let world = &request.world;
        anyhow::ensure!(
            scene.base_x == world.base_x
                && scene.base_z == world.base_z
                && scene.map_size == world.width as usize
                && scene.map_size == world.height as usize,
            "renderer/request map bounds differ"
        );
        let terrain = &map.data.terrain;
        anyhow::ensure!(
            terrain.width == scene.map_size && terrain.height == scene.map_size,
            "CPU/renderer dimensions differ"
        );
        for level in 0..4 {
            let floor = scene
                .scene
                .normal
                .get(level)
                .and_then(Option::as_ref)
                .ok_or_else(|| anyhow::anyhow!("missing scene floor {level}"))?;
            for x in 0..=scene.map_size {
                for z in 0..=scene.map_size {
                    anyhow::ensure!(
                        floor.heights.get_tile_height(x, z)
                            == terrain.heights[terrain.point(level, x, z)],
                        "CPU/renderer height differs at {level}:{x}:{z}"
                    );
                    if x < scene.map_size && z < scene.map_size {
                        anyhow::ensure!(
                            scene.flags.get(level, x, z)
                                == terrain.tiles[terrain.tile(level, x, z)].flags,
                            "CPU/renderer flags differ at {level}:{x}:{z}"
                        );
                    }
                }
            }
        }
        Ok(())
    }
    /// Capture the loc snapshots of the built scene: primary-layer locs looked
    /// up per tile. Traverse actual post-bridge tile links.
    fn capture_scene(&mut self, scene: &crate::scene::Scene) {
        self.scene_locs.clear();
        let snapshot = |id, shape, angle, srt: Option<crate::map::LocSrt>| Snapshot {
            id,
            shape,
            angle,
            transform: srt.map(|s| {
                [
                    s.rot[0], s.rot[1], s.rot[2], s.rot[3], s.trans[0], s.trans[1], s.trans[2],
                    s.scale[0], s.scale[1], s.scale[2],
                ]
            }),
        };
        for level in 0..scene.max_level {
            for x in 0..scene.max_x {
                for z in 0..scene.max_z {
                    let Some(tile) = scene.tile(level, x, z) else {
                        continue;
                    };
                    let key = |layer| (level as i32, layer, x as i32, z as i32);
                    if let Some(i) = tile.wall {
                        let e = &scene.walls[i];
                        self.scene_locs
                            .insert(key(0), snapshot(e.loc_id as i32, e.shape, e.angle, e.srt));
                    }
                    if let Some(i) = tile.wall_decoration {
                        let e = &scene.wall_decors[i];
                        self.scene_locs
                            .insert(key(1), snapshot(e.loc_id as i32, e.shape, e.angle, e.srt));
                    }
                    if let Some(e) = tile
                        .entities
                        .iter()
                        .filter_map(|&r| {
                            if let crate::scene::PrimaryRef::Scenery(i) = r {
                                Some(&scene.scenery[i])
                            } else {
                                None
                            }
                        })
                        .find(|e| e.primary_layer && e.min_tx == x as i32 && e.min_tz == z as i32)
                    {
                        self.scene_locs
                            .insert(key(2), snapshot(e.loc_id as i32, e.shape, e.angle, e.srt));
                    }
                    if let Some(i) = tile.ground_decoration {
                        let e = &scene.ground_decors[i];
                        self.scene_locs
                            .insert(key(3), snapshot(e.loc_id as i32, 22, e.angle, e.srt));
                    }
                }
            }
        }
    }
}
