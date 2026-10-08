//! CPU entity state for players and NPCs.
//! Movement and animation request state are CPU-only.
//! Particles attach through the scene owners (`player_renderer`, the app's
//! transient NPC/effect bindings) into `particle::Runtime`.
//! TODO(#gap-G-effects): audio/model-cache effects need an integration adapter.
pub mod animation_state;
pub mod appearance;
pub mod chat;
pub mod combat;
pub mod movement;
pub mod npc_custom;
pub mod scene_state;
pub mod transient;
/// Sparse variable-container values (moved to rs910-config, Phase 2.6).
pub use rs910_config::types910::variables;
pub mod walk_animation;
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::error::*;
/// Actor-owned render state survives packet updates, but not actor reuse.
#[derive(Clone, Debug, PartialEq)]
pub struct ActorState {
    pub scene: scene_state::State,
    pub walk: walk_animation::Walk,
    pub wear_angles: Option<Vec<i32>>,
    /// The model key, retained across appearance changes (default 0).
    pub model_key: i64,
    /// Rotation quaternion, stored in field order x, y, z, w.
    pub rotation: [f32; 4],
    /// Count of particle-system restarts (the held system replays its
    /// initial burst): appearance changes, teleports, NPC type changes and
    /// NPC moves with the teleport flag. The particle owner applies new
    /// counts before the next particle render.
    pub particle_resets: u32,
    /// Count of teleports: the local player's teleport resets the
    /// environment fade.
    pub teleports: u32,
}
impl Default for ActorState {
    fn default() -> Self {
        Self {
            scene: Default::default(),
            walk: Default::default(),
            wear_angles: None,
            model_key: 0,
            rotation: [0., 0., 0., 1.],
            particle_resets: 0,
            teleports: 0,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub actor: ActorState,
    pub appearance: appearance::Appearance,
    pub motion: movement::Motion,
    pub combat: Option<combat::Combat>,
    pub chat: Option<chat::Chat>,
    pub chat_history: Vec<chat::HistoryRequest>,
    pub animation: animation_state::AnimationState,
    pub variables: std::collections::BTreeMap<i32, variables::Value>,
    pub forced: [i32; 9],
    pub tint: [i32; 6],
    pub wear: Option<Vec<i32>>,
    pub partner: i32,
    pub level: i32,
    pub occlude_level: i32,
    pub x: [i32; 10],
    pub z: [i32; 10],
    pub speeds: [i8; 10],
    pub route_length: usize,
    pub steps_remaining: i32,
    pub seq_trigger: i32,
    pub fine_x: f32,
    pub fine_z: f32,
    pub size: i32,
    pub target: i32,
    pub face_override: i32,
    pub angle: i32,
    pub desired_angle: i32,
    pub suppress_partner: bool,
}
impl Default for Player {
    fn default() -> Self {
        Self {
            actor: ActorState::default(),
            appearance: appearance::Appearance::default(),
            motion: movement::Motion::default(),
            combat: None,
            chat: None,
            chat_history: vec![],
            animation: animation_state::AnimationState::default(),
            variables: Default::default(),
            forced: [0; 9],
            tint: [0, 0, 0, 0, -1, -1],
            wear: None,
            partner: 0,
            level: 0,
            occlude_level: 0,
            x: [0; 10],
            z: [0; 10],
            speeds: [0; 10],
            route_length: 0,
            steps_remaining: 0,
            seq_trigger: 0,
            fine_x: 0.,
            fine_z: 0.,
            size: 1,
            target: -1,
            face_override: -1,
            angle: 0,
            desired_angle: 0,
            suppress_partner: false,
        }
    }
}
impl Player {
    /// Teleport: does not clear the tail or route speed.
    pub fn tele(&mut self, x: i32, z: i32) {
        self.actor.particle_resets = self.actor.particle_resets.wrapping_add(1);
        self.actor.teleports = self.actor.teleports.wrapping_add(1);
        self.route_length = 0;
        self.steps_remaining = 0;
        self.seq_trigger = 0;
        self.x[0] = x;
        self.z[0] = z;
        self.fine_x = x
            .wrapping_mul(512)
            .wrapping_add(self.size.wrapping_mul(256)) as f32;
        self.fine_z = z
            .wrapping_mul(512)
            .wrapping_add(self.size.wrapping_mul(256)) as f32;
    }
    /// Move the player one route step, including priority-one animation cancellation.
    pub fn move_player(&mut self, x: i32, z: i32, speed: i8, width: i32, height: i32) {
        self.animation.cancel_for_move();
        self.face_override = -1;
        if x < 0
            || z < 0
            || x >= width
            || z >= height
            || self.x[0] < 0
            || self.z[0] < 0
            || self.x[0] >= width
            || self.z[0] >= height
        {
            self.tele(x, z);
            return;
        }
        self.route_length = (self.route_length + 1).min(9);
        for i in (1..=self.route_length).rev() {
            self.x[i] = self.x[i - 1];
            self.z[i] = self.z[i - 1];
            self.speeds[i] = self.speeds[i - 1];
        }
        self.x[0] = x;
        self.z[0] = z;
        self.speeds[0] = speed;
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct LowPlayer {
    pub partner: i32,
    pub coord: i32,
    pub angle: i32,
    pub target: i32,
    pub suppress_partner: bool,
}

/// NPC state. Path storage is shared with the player slice;
/// NPC movement uses its own rules and never calls move_player.
#[derive(Clone, Debug, PartialEq)]
pub struct Npc {
    pub path: Player,
    pub head_icons: Option<([i32; 8], [i16; 8])>,
    pub covermarker: i32,
    pub body: Option<npc_custom::Custom>,
    pub stats: [i32; 6],
    pub stat_max: [i32; 6],
    pub type_id: i32,
    pub update_serial: i32,
    pub turn_speed: i32,
    pub fade_alpha: i32,
    pub fade_start: i32,
    pub recolours: [i32; 4],
    pub face_x: i32,
    pub face_z: i32,
    pub op_mask: i32,
    pub vislevel: i32,
    pub bas_override: i32,
    pub flag: bool,
    pub name: String,
}
impl Npc {
    pub fn new(recolours: [i32; 4]) -> Self {
        Self {
            path: Player::default(),
            head_icons: None,
            covermarker: -1,
            body: None,
            stats: [0; 6],
            stat_max: [0; 6],
            type_id: -1,
            update_serial: 0,
            turn_speed: 256,
            fade_alpha: 0,
            fade_start: -1,
            recolours,
            face_x: -1,
            face_z: -1,
            op_mask: 0,
            vislevel: 0,
            bas_override: -1,
            flag: false,
            name: String::new(),
        }
    }
    pub fn enqueue(&mut self, x: i32, z: i32, speed: i8) {
        let p = &mut self.path;
        p.route_length = (p.route_length + 1).min(9);
        for i in (1..=p.route_length).rev() {
            p.x[i] = p.x[i - 1];
            p.z[i] = p.z[i - 1];
            p.speeds[i] = p.speeds[i - 1];
        }
        p.x[0] = x;
        p.z[0] = z;
        p.speeds[0] = speed;
    }
    /// Step in compass direction `dir` (serial id -> index).
    pub fn step(&mut self, dir: usize, speed: i8) {
        self.path.animation.cancel_for_move();
        let (dx, dz) = [
            (0, 1),
            (1, 1),
            (1, 0),
            (1, -1),
            (0, -1),
            (-1, -1),
            (-1, 0),
            (-1, 1),
        ][dir];
        self.enqueue(
            self.path.x[0].wrapping_add(dx),
            self.path.z[0].wrapping_add(dz),
            speed,
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundObject {
    pub id: i32,
    pub count: i32,
}

pub mod varps;
