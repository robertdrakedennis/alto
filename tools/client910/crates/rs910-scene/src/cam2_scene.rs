//! What the client hands the cam2 camera and the scene pickers each cycle:
//! the camera-trackable view of the local player (with its trackable kind ids)
//! and the borrowed `SceneInput` view of the entity runtime. Split out of
//! client910's `ui_cam2` so `player_picking` reads it in the scene layer;
//! `ui_cam2` re-exports it.

use crate::protocol910::terrain::Terrain;
use crate::vector_math::*;

/// Trackable kind ids: player and NPC.
pub const TRACKABLE_PLAYER: i32 = 0;

pub const TRACKABLE_NPC: i32 = 1;

// ---------------------------------------------------------------------------
// Camera trackable over the local player, and the scene the camera collides with.
// ---------------------------------------------------------------------------

/// The local player as a camera trackable, sampled once per logic cycle from
/// the entity runtime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trackable {
    /// Trackable kind (player or NPC).
    pub kind: i32,
    /// Entity index (the local player index).
    pub index: i32,
    /// Plane the entity stands on.
    pub level: i32,
    /// `(fine x + base x * 512, -height, fine z + base z * 512)`.
    pub coord: [i32; 3],
    /// The eased facing angle.
    pub yaw: i32,
}

impl Trackable {
    /// The coordinate as a float vector.
    pub fn coord_vec(&self) -> Vec3 {
        Vec3::new(
            self.coord[0] as f32,
            self.coord[1] as f32,
            self.coord[2] as f32,
        )
    }
    /// Facing as a rotation about the yaw axis only.
    pub fn orientation(&self) -> Quat {
        let mut q = Quat::IDENTITY;
        q.set_to_rotation_ypr(crate::trig::radians(self.yaw), 0.0, 0.0);
        q
    }
    /// A zero vector (players report no velocity).
    pub fn create_vector3(&self) -> Vec3 {
        Vec3::ZERO
    }
}

/// The borrowed per-cycle view of the entity runtime; `Cam2::sync_scene` copies it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SceneInput<'a> {
    pub players: Option<&'a crate::protocol910::Players>,
    /// Retained NPC slots from the normal live entity feed. Menu construction
    /// uses their scene tile and server op mask; model picking remains a
    /// separate presentation boundary.
    pub npcs: Option<&'a crate::protocol910::npc::Npcs>,
    /// Object stacks retained by the live zone decoder. Scene menu
    /// construction reads the stack at the picked tile; rendering/culling
    /// remains owned by the scene renderer.
    pub objects: Option<&'a crate::protocol910::zone::Objects>,
    /// Static loc snapshots captured from the installed scene graph. Scene
    /// menu construction uses these to resolve the location at a picked tile
    /// before encoding the ordinary OPLOC action.
    pub locations: Option<&'a crate::protocol910::zone_state::SceneLocs>,
    pub map_width: i32,
    pub base: [i32; 2],
    pub local_player: Option<Trackable>,
    /// Footprint size of the local player, in tiles.
    pub local_size: i32,
    pub terrain: Option<&'a Terrain>,
    pub terrain_generation: u64,
}

impl<'a> SceneInput<'a> {
    /// Builds the view from the map base, the local player slot and the terrain.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "context constructor; exercised by tests only")
    )]
    pub fn new(
        map: &crate::protocol910::Context,
        players: &'a crate::protocol910::Players,
        terrain: Option<&'a Terrain>,
        terrain_generation: u64,
    ) -> Self {
        Self::new_with_objects(map, players, None, terrain, terrain_generation)
    }
    pub fn new_with_objects(
        map: &crate::protocol910::Context,
        players: &'a crate::protocol910::Players,
        objects: Option<&'a crate::protocol910::zone::Objects>,
        terrain: Option<&'a Terrain>,
        terrain_generation: u64,
    ) -> Self {
        let local_player = players
            .players
            .get(map.local)
            .and_then(Option::as_ref)
            .map(|p| Trackable {
                kind: TRACKABLE_PLAYER,
                index: map.local as i32,
                level: p.level,
                coord: [
                    (p.fine_x as i32).wrapping_add(map.base_x.wrapping_mul(512)),
                    -(p.motion.y as i32),
                    (p.fine_z as i32).wrapping_add(map.base_z.wrapping_mul(512)),
                ],
                yaw: p.angle,
            });
        let local_size = players
            .players
            .get(map.local)
            .and_then(Option::as_ref)
            .map_or(1, |p| p.size);
        Self {
            players: Some(players),
            npcs: None,
            objects,
            locations: None,
            map_width: map.width,
            base: [map.base_x << 9, map.base_z << 9],
            local_player,
            local_size,
            terrain,
            terrain_generation,
        }
    }
}
