//! Ambient background sounds of locs, NPCs and players, their per-frame
//! update and the scene lifecycle (add, remove, refresh, reset).
//!
//! Every sound is created through the audio API as an owned sound (loops on
//! the generic location bus, random one-shots on the random location bus,
//! both stereo-shaped); the distance gate, the stereo attenuation between
//! `dropoffrange` and `range` and the background-sound volume bus all live in
//! the audio API.
//!
//! The original hooks fire inside packet decoding and map loading. The Rust
//! packet decoders produce retained state snapshots instead of callbacks, so
//! [`PositionedSounds::sync_npcs`], [`PositionedSounds::sync_players`] and
//! [`PositionedSounds::sync_locs`] derive the same add/remove calls from the
//! state each frame, and the map loader hands its ground-loc sound calls over
//! with the rebuild ([`LocSoundScene`]).

use crate::audio_api::{AudioApi, SoundParams, SoundShape};
use crate::audio_backend::sub_buss_type;
use crate::audio_stream::{SoundState, SoundType};
pub use rs910_config::loc_sound::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// One retained loc change request in scene tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocRequest {
    pub level: i32,
    pub layer: i32,
    pub x: i32,
    pub z: i32,
    pub old_id: i32,
    pub old_angle: i32,
    pub id: i32,
    pub angle: i32,
    pub remove: bool,
}

/// Where a loc with a background sound is placed: scene tile, level, angle and
/// loc id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocPlacement {
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub angle: i32,
    pub id: i32,
}

/// The run and crawl sequence ids of a movement set, which the walk sequence
/// is compared against to pick an NPC's, a player's or an entry's sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasMotion {
    pub run: [i32; 4],
    pub crawl: [i32; 4],
}

impl BasMotion {
    /// The default movement set (every sequence -1).
    pub const DEFAULT: Self = Self {
        run: [-1; 4],
        crawl: [-1; 4],
    };

    pub fn from_bas(bas: &crate::protocol910::bas_types::Bas) -> Self {
        Self {
            run: [bas.runanim, bas.runanim_b, bas.runanim_r, bas.runanim_l],
            crawl: [
                bas.crawlanim,
                bas.crawlanim_b,
                bas.crawlanim_r,
                bas.crawlanim_l,
            ],
        }
    }
}

/// The walk-sequence class shared by the NPC, player and per-frame sound
/// choice: 0 = no walk sequence or `seq_active` (ready/idle), 1 = walk,
/// 2 = run, 3 = crawl.
pub fn motion(bas: &BasMotion, walk_seq: i32, seq_active: bool) -> i32 {
    if walk_seq == -1 || seq_active {
        0
    } else if bas.run.contains(&walk_seq) {
        2
    } else if bas.crawl.contains(&walk_seq) {
        3
    } else {
        1
    }
}

/// An NPC type's background-sound fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NpcSound {
    /// `bgsound`, `bgsound_crawl`, `bgsound_walk`, `bgsound_run`.
    pub sounds: [i32; 4],
    pub range: i32,
    pub dropoffrange: i32,
    pub volume: i32,
    pub minrate: i32,
    pub maxrate: i32,
}

impl NpcSound {
    pub fn from_type(npc: &crate::protocol910::config_types::Npc) -> Self {
        Self {
            sounds: npc.sounds,
            range: npc.sound_range,
            dropoffrange: npc.sound_dropoff,
            volume: npc.sound_volume,
            minrate: npc.sound_rates[0],
            maxrate: npc.sound_rates[1],
        }
    }

    fn for_motion(&self, motion: i32) -> i32 {
        match motion {
            0 => self.sounds[0],
            2 => self.sounds[3],
            3 => self.sounds[1],
            _ => self.sounds[2],
        }
    }
}

/// The NPC state the background sounds read, sampled per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NpcSample {
    /// The NPC list slot.
    pub index: usize,
    /// The NPC type id (a new id replaces the entry).
    pub type_id: i32,
    /// Whether the NPC type has any background sound.
    pub has_background_sound: bool,
    /// Whether the NPC type has a multi-NPC variant table.
    pub multinpc: bool,
    /// The NPC type's sound, or the variant it currently resolves to when
    /// `multinpc` (`None` when the variant is absent).
    pub resolved: Option<NpcSound>,
    pub level: i32,
    /// The first route waypoint tile.
    pub tile: [i32; 2],
    /// The world position x/z.
    pub trans: [f32; 2],
    pub size: i32,
    /// [`motion`] of the walk sequence against the movement set.
    pub motion: i32,
}

/// The player state the background sounds read, sampled per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerSample {
    pub index: usize,
    /// `bgsound_range` (appearance g1; 0 removes the sound).
    pub range: i32,
    /// `bgsound_player`, `_crawl_player`, `_walk_player`, `_run_player`.
    pub sounds: [i32; 4],
    pub volume: i32,
    pub level: i32,
    pub tile: [i32; 2],
    pub trans: [f32; 2],
    pub size: i32,
    pub motion: i32,
}

impl PlayerSample {
    /// The sound for the current motion class.
    fn sound(&self) -> i32 {
        match self.motion {
            0 => self.sounds[0],
            2 => self.sounds[3],
            3 => self.sounds[1],
            _ => self.sounds[2],
        }
    }
}

/// The sound for an NPC sample's current motion class.
fn npc_sound(npc: &NpcSample) -> i32 {
    match npc.resolved {
        Some(sound) => sound.for_motion(npc.motion),
        None if npc.multinpc => -1,
        // A non-multinpc `npcType` is never null; a sample without it has
        // no background sound.
        None => -1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Loc(i32),
    Npc(usize),
    Player(usize),
}

/// One background sound node. Fields stay 0 unless the add call sets them.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    owner: Owner,
    pub level: i32,
    pub max_x: i32,
    pub min_x: i32,
    pub min_z: i32,
    pub max_z: i32,
    /// The positions handed to the loop and random sounds.
    pub position: [f32; 3],
    pub random_position: [f32; 3],
    pub dropoffrange: i32,
    pub range: i32,
    pub volume: i32,
    /// The last [`motion`] class the sound was chosen for.
    pub motion: i32,
    pub multisound: bool,
    pub minrate: i32,
    pub maxrate: i32,
    pub sound: i32,
    /// Audio API pool keys of the live loop and random sounds.
    pub loop_sound: Option<usize>,
    pub random_sound: Option<usize>,
    pub mindelay: i32,
    pub maxdelay: i32,
    pub random: Option<Vec<i32>>,
    pub delay: i32,
}

impl Entry {
    fn new(owner: Owner, level: i32, x: i32, z: i32) -> Self {
        Self {
            owner,
            level,
            max_x: 0,
            min_x: x << 9,
            min_z: z << 9,
            max_z: 0,
            position: [0.0; 3],
            random_position: [0.0; 3],
            dropoffrange: 0,
            range: 0,
            volume: 0,
            motion: 0,
            multisound: false,
            minrate: 0,
            maxrate: 0,
            sound: 0,
            loop_sound: None,
            random_sound: None,
            mindelay: 0,
            maxdelay: 0,
            random: None,
            delay: 0,
        }
    }

    /// The footprint centre: `(int) ((float) (max - min) * 0.5F + (float) min)`
    /// per axis.
    fn centre(&self) -> [f32; 3] {
        let x = ((self.max_x - self.min_x) as f32 * 0.5 + self.min_x as f32) as i32;
        let z = ((self.max_z - self.min_z) as f32 * 0.5 + self.min_z as f32) as i32;
        [x as f32, 0.0, z as f32]
    }
}

/// Fade the sound out over `ms` and hand it back to the audio API to finish.
fn fade_out_play(api: &mut AudioApi, sound: &mut Option<usize>, ms: i32) {
    if let Some(key) = sound.take() {
        api.sound_fade_out_play(key, ms);
    }
}

fn stopped(api: &AudioApi, key: usize) -> bool {
    matches!(
        api.sound_status(key),
        SoundState::Finished | SoundState::Stopped
    )
}

/// The per-frame world view the update reads.
pub struct Frame<'a> {
    /// The local player's level.
    pub level: i32,
    /// The scene time elapsed since the last frame.
    pub scene_delta: i32,
    /// The background-sound volume preference.
    pub background_volume: i32,
    pub npcs: &'a [NpcSample],
    pub players: &'a [PlayerSample],
}

/// Everything one drawn frame hands the owner: the rebuilt map (if a map
/// load finished since the last frame), the retained loc requests, the
/// sampled NPCs/players and the per-frame update arguments.
pub struct FrameInput<'a> {
    pub rebuilt: Option<&'a LocSoundScene>,
    pub loc_requests: &'a [LocRequest],
    /// The world size in tiles.
    pub map_size: [i32; 2],
    /// Whether textures are enabled.
    pub textures: bool,
    /// A variable changed since the last frame.
    pub varps_changed: bool,
    /// Reads the variable a multi-loc selects on: from the cutscene variable
    /// domain while the scene state is 0, else from the player game state.
    pub read_loc: &'a dyn Fn(bool, i32) -> Option<i32>,
    pub level: i32,
    pub scene_delta: i32,
    pub npcs: &'a [NpcSample],
    pub players: &'a [PlayerSample],
}

/// The live background sounds plus the state-diff trackers that stand in for
/// the packet-time hooks.
pub struct PositionedSounds {
    /// Loc sounds.
    pub locs: Vec<Entry>,
    /// NPC sounds.
    pub npcs: Vec<Entry>,
    /// Player sounds, kept in the iteration order of a 16-bucket table keyed
    /// by player index: bucket `index & 15`, then insertion order.
    pub players: Vec<Entry>,
    table: Arc<LocSoundTable>,
    /// The random source (uniform in [0, 1)).
    random: Box<dyn FnMut() -> f64>,
    /// NPC index -> `npcType.id` of NPCs holding an entry.
    tracked_npcs: BTreeMap<usize, i32>,
    /// Player index -> the appearance sound fields last applied
    /// (a change in these refreshes the entry).
    tracked_players: BTreeMap<usize, (i32, [i32; 4], i32)>,
    /// `(level, layer, x, z)` -> the loc id/angle the last applied loc
    /// request left in the scene.
    applied_locs: HashMap<(i32, i32, i32, i32), (i32, i32)>,
}

impl Default for PositionedSounds {
    fn default() -> Self {
        Self::new(Box::new(crate::ui_cam2::random_unit))
    }
}

impl PositionedSounds {
    pub fn new(random: Box<dyn FnMut() -> f64>) -> Self {
        Self {
            locs: Vec::new(),
            npcs: Vec::new(),
            players: Vec::new(),
            table: Arc::new(LocSoundTable::default()),
            random,
            tracked_npcs: BTreeMap::new(),
            tracked_players: BTreeMap::new(),
            applied_locs: HashMap::new(),
        }
    }

    /// Fade out and drop the loc sounds, and with `all` the NPC and player
    /// sounds too.
    pub fn reset(&mut self, api: &mut AudioApi, all: bool) {
        for mut entry in std::mem::take(&mut self.locs) {
            fade_out_play(api, &mut entry.loop_sound, 150);
            fade_out_play(api, &mut entry.random_sound, 150);
        }
        self.applied_locs.clear();
        if !all {
            return;
        }
        for mut entry in std::mem::take(&mut self.npcs) {
            fade_out_play(api, &mut entry.loop_sound, 150);
        }
        for mut entry in std::mem::take(&mut self.players) {
            fade_out_play(api, &mut entry.loop_sound, 150);
        }
        self.tracked_npcs.clear();
        self.tracked_players.clear();
    }

    /// A map rebuild: drop the loc sounds, then add the rebuilt map's
    /// ground-loc sounds.
    pub fn rebuild(
        &mut self,
        api: &mut AudioApi,
        scene: &LocSoundScene,
        read: &dyn Fn(bool, i32) -> Option<i32>,
    ) {
        self.reset(api, false);
        self.table = scene.table.clone();
        for spawn in &scene.spawns {
            let placement = LocPlacement {
                level: spawn.level,
                x: spawn.x,
                z: spawn.z,
                angle: spawn.angle,
                id: spawn.id as i32,
            };
            self.add_loc(api, placement, read);
        }
    }

    /// Add the background sound of a loc placed at `placement`.
    pub fn add_loc(
        &mut self,
        api: &mut AudioApi,
        placement: LocPlacement,
        read: &dyn Fn(bool, i32) -> Option<i32>,
    ) {
        let LocPlacement {
            level,
            x,
            z,
            angle,
            id,
        } = placement;
        let loc = self.table.list(id);
        let mut entry = Entry::new(Owner::Loc(id), level, x, z);
        let (width, length) = if angle == 1 || angle == 3 {
            (loc.length, loc.width)
        } else {
            (loc.width, loc.length)
        };
        entry.max_x = (x + width) << 9;
        entry.max_z = (z + length) << 9;
        entry.sound = loc.sound;
        entry.range = loc.range << 9;
        entry.volume = loc.volume;
        entry.mindelay = loc.mindelay;
        entry.maxdelay = loc.maxdelay;
        entry.random = loc.random.clone();
        entry.maxrate = loc.maxrate;
        entry.minrate = loc.minrate;
        entry.dropoffrange = loc.dropoffrange << 9;
        if loc.multiloc.is_some() {
            entry.multisound = true;
            self.refresh(api, &mut entry, None, None, read);
        }
        if entry.random.is_some() {
            entry.delay = entry.mindelay
                + ((self.random)() * f64::from(entry.maxdelay - entry.mindelay)) as i32;
        }
        api.preload_sounds(entry.sound);
        if let Some(random) = &entry.random {
            for &id in random {
                api.preload_sounds(id);
            }
        }
        self.locs.push(entry);
    }

    /// Add the background sound of an NPC.
    pub fn add_npc(&mut self, api: &mut AudioApi, npc: &NpcSample) {
        let mut entry = Entry::new(Owner::Npc(npc.index), npc.level, npc.tile[0], npc.tile[1]);
        entry.multisound = npc.multinpc;
        if let Some(sound) = npc.resolved {
            entry.max_x = (npc.size + npc.tile[0]) << 9;
            entry.max_z = (npc.size + npc.tile[1]) << 9;
            entry.sound = npc_sound(npc);
            entry.range = sound.range << 9;
            entry.volume = sound.volume;
            entry.maxrate = sound.maxrate;
            entry.minrate = sound.minrate;
            entry.dropoffrange = sound.dropoffrange << 9;
            for id in [
                sound.sounds[0],
                sound.sounds[1],
                sound.sounds[2],
                sound.sounds[3],
            ] {
                api.preload_sounds(id);
            }
        }
        self.npcs.push(entry);
    }

    /// Add the background sound of a player.
    pub fn add_player(&mut self, api: &mut AudioApi, player: &PlayerSample) {
        let mut entry = Entry::new(
            Owner::Player(player.index),
            player.level,
            player.tile[0],
            player.tile[1],
        );
        entry.max_x = (player.tile[0] + player.size) << 9;
        entry.max_z = (player.tile[1] + player.size) << 9;
        entry.sound = player.sound();
        entry.range = player.range << 9;
        entry.volume = player.volume;
        entry.maxrate = 256;
        entry.minrate = 256;
        entry.dropoffrange = 0;
        for id in player.sounds {
            api.preload_sounds(id);
        }
        // Insert at the tail of bucket `index & 15`.
        let bucket = player.index & 15;
        let at = self
            .players
            .iter()
            .position(|e| matches!(e.owner, Owner::Player(i) if i & 15 > bucket))
            .unwrap_or(self.players.len());
        self.players.insert(at, entry);
    }

    /// Drop the first loc sound of `id` at this tile.
    pub fn remove_loc(&mut self, api: &mut AudioApi, level: i32, x: i32, z: i32, id: i32) {
        if let Some(at) = self.locs.iter().position(|e| {
            e.level == level && x << 9 == e.min_x && z << 9 == e.min_z && e.owner == Owner::Loc(id)
        }) {
            let mut entry = self.locs.remove(at);
            fade_out_play(api, &mut entry.loop_sound, 100);
        }
    }

    /// Drop an NPC's sound.
    pub fn remove_npc(&mut self, api: &mut AudioApi, index: usize) {
        if let Some(at) = self.npcs.iter().position(|e| e.owner == Owner::Npc(index)) {
            let mut entry = self.npcs.remove(at);
            fade_out_play(api, &mut entry.loop_sound, 100);
        }
    }

    /// Drop a player's sound.
    pub fn remove_player(&mut self, api: &mut AudioApi, index: usize) {
        if let Some(at) = self
            .players
            .iter()
            .position(|e| e.owner == Owner::Player(index))
        {
            let mut entry = self.players.remove(at);
            fade_out_play(api, &mut entry.loop_sound, 100);
        }
    }

    /// Add a player's sound, or refresh it when the entry already exists.
    pub fn add_or_refresh_player(&mut self, api: &mut AudioApi, player: &PlayerSample) {
        match self
            .players
            .iter()
            .position(|e| e.owner == Owner::Player(player.index))
        {
            None => self.add_player(api, player),
            Some(at) => {
                let mut entry = self.players[at].clone();
                self.refresh(api, &mut entry, None, Some(player), &|_, _| None);
                self.players[at] = entry;
            }
        }
    }

    /// Re-select the sound of every multi-variant loc and NPC entry; run for
    /// every changed variable.
    pub fn refresh_multisounds(
        &mut self,
        api: &mut AudioApi,
        npcs: &[NpcSample],
        read: &dyn Fn(bool, i32) -> Option<i32>,
    ) {
        let mut locs = std::mem::take(&mut self.locs);
        for entry in locs.iter_mut().filter(|e| e.multisound) {
            self.refresh(api, entry, None, None, read);
        }
        self.locs = locs;
        let mut entries = std::mem::take(&mut self.npcs);
        for entry in entries.iter_mut().filter(|e| e.multisound) {
            let Owner::Npc(index) = entry.owner else {
                continue;
            };
            if let Some(npc) = npcs.iter().find(|n| n.index == index) {
                self.refresh(api, entry, Some(npc), None, read);
            }
        }
        self.npcs = entries;
    }

    /// Re-select one entry's sound from its current variant, fading out the
    /// old loop when the sound changed.
    fn refresh(
        &mut self,
        api: &mut AudioApi,
        entry: &mut Entry,
        npc: Option<&NpcSample>,
        player: Option<&PlayerSample>,
        read: &dyn Fn(bool, i32) -> Option<i32>,
    ) {
        let previous = entry.sound;
        match entry.owner {
            Owner::Loc(id) => {
                // `read` supplies the variable domain the multi-loc selects on.
                let base = self.table.list(id);
                match self.table.multi_loc(&base, read) {
                    None => {
                        entry.sound = -1;
                        entry.range = 0;
                        entry.volume = 0;
                        entry.mindelay = 0;
                        entry.maxdelay = 0;
                        entry.random = None;
                        entry.maxrate = 256;
                        entry.minrate = 256;
                        entry.dropoffrange = 0;
                    }
                    Some(loc) => {
                        entry.sound = loc.sound;
                        entry.range = loc.range << 9;
                        entry.volume = loc.volume;
                        entry.mindelay = loc.mindelay;
                        entry.maxdelay = loc.maxdelay;
                        entry.random = loc.random;
                        entry.maxrate = loc.maxrate;
                        entry.minrate = loc.minrate;
                    }
                }
            }
            Owner::Npc(_) => {
                let Some(npc) = npc else { return };
                let sound = npc_sound(npc);
                if previous != sound {
                    entry.sound = sound;
                    match npc.resolved {
                        None => {
                            entry.dropoffrange = 0;
                            entry.range = 0;
                            entry.volume = 0;
                            entry.maxrate = 256;
                            entry.minrate = 256;
                        }
                        Some(resolved) => {
                            entry.range = resolved.range << 9;
                            entry.dropoffrange = resolved.dropoffrange << 9;
                            entry.volume = resolved.volume;
                            entry.maxrate = resolved.maxrate;
                            entry.minrate = resolved.minrate;
                        }
                    }
                }
            }
            Owner::Player(_) => {
                let Some(player) = player else { return };
                entry.sound = player.sound();
                entry.range = player.range << 9;
                entry.dropoffrange = 0;
                entry.volume = player.volume;
                entry.maxrate = 256;
                entry.minrate = 256;
            }
        }
        if entry.sound != previous {
            fade_out_play(api, &mut entry.loop_sound, 100);
        }
    }

    /// The NPC half of the packet hooks (NPC updates, adds and type changes
    /// and scene removal), derived from the retained NPC list in slot order.
    pub fn sync_npcs(&mut self, api: &mut AudioApi, npcs: &[NpcSample]) {
        let current: HashMap<usize, &NpcSample> = npcs.iter().map(|n| (n.index, n)).collect();
        let stale: Vec<usize> = self
            .tracked_npcs
            .iter()
            .filter(|(index, type_id)| current.get(index).is_none_or(|n| n.type_id != **type_id))
            .map(|(&index, _)| index)
            .collect();
        for index in stale {
            self.remove_npc(api, index);
            self.tracked_npcs.remove(&index);
        }
        for npc in npcs {
            if npc.has_background_sound && !self.tracked_npcs.contains_key(&npc.index) {
                self.add_npc(api, npc);
                self.tracked_npcs.insert(npc.index, npc.type_id);
            }
        }
    }

    /// The player half: appearance changes and the drop when a player leaves
    /// the low-resolution list.
    pub fn sync_players(&mut self, api: &mut AudioApi, players: &[PlayerSample]) {
        let present: Vec<usize> = players.iter().map(|p| p.index).collect();
        let gone: Vec<usize> = self
            .tracked_players
            .keys()
            .copied()
            .filter(|index| !present.contains(index))
            .collect();
        for index in gone {
            self.remove_player(api, index);
            self.tracked_players.remove(&index);
        }
        for player in players {
            let fields = (player.range, player.sounds, player.volume);
            if player.range == 0 {
                if self.tracked_players.remove(&player.index).is_some() {
                    self.remove_player(api, player.index);
                }
            } else if self.tracked_players.get(&player.index) != Some(&fields) {
                self.add_or_refresh_player(api, player);
                self.tracked_players.insert(player.index, fields);
            }
        }
    }

    /// Apply loc change requests as far as they reach the background sounds:
    /// drop the sound of the loc leaving the tile and add that of the one
    /// arriving. `size` is the world size in tiles; `textures` is whether
    /// textures are enabled.
    pub fn sync_locs(
        &mut self,
        api: &mut AudioApi,
        requests: &[LocRequest],
        size: [i32; 2],
        textures: bool,
        read: &dyn Fn(bool, i32) -> Option<i32>,
    ) {
        for request in requests {
            if request.x < 1 || request.z < 1 || request.x > size[0] - 2 || request.z > size[1] - 2
            {
                continue;
            }
            let key = (request.level, request.layer, request.x, request.z);
            let target = if request.remove {
                (request.old_id, request.old_angle)
            } else {
                (request.id, request.angle)
            };
            let current = self.applied_locs.get(&key).copied();
            if current == Some(target) {
                continue;
            }
            let (old, _) = current.unwrap_or((request.old_id, request.old_angle));
            if old >= 0 && self.table.has_background_sound(old) {
                self.remove_loc(api, request.level, request.x, request.z, old);
            }
            let (id, angle) = target;
            // Textured locs are skipped with textures off, before the sound.
            if id >= 0
                && (textures || !self.table.list(id).istexture)
                && self.table.has_background_sound(id)
            {
                let placement = LocPlacement {
                    level: request.level,
                    x: request.x,
                    z: request.z,
                    angle,
                    id,
                };
                self.add_loc(api, placement, read);
            }
            self.applied_locs.insert(key, target);
        }
    }

    /// One drawn frame: the state-derived hooks in packet order, then the
    /// per-frame update.
    pub fn frame(&mut self, api: &mut AudioApi, input: &FrameInput<'_>, background_volume: i32) {
        if let Some(scene) = input.rebuilt {
            self.rebuild(api, scene, input.read_loc);
        }
        self.sync_locs(
            api,
            input.loc_requests,
            input.map_size,
            input.textures,
            input.read_loc,
        );
        self.sync_npcs(api, input.npcs);
        self.sync_players(api, input.players);
        if input.varps_changed {
            self.refresh_multisounds(api, input.npcs, input.read_loc);
        }
        self.update(
            api,
            &Frame {
                level: input.level,
                scene_delta: input.scene_delta,
                background_volume,
                npcs: input.npcs,
                players: input.players,
            },
        );
    }

    /// Per-frame update of every entry, called once per drawn frame after the
    /// minimenu.
    pub fn update(&mut self, api: &mut AudioApi, frame: &Frame<'_>) {
        let mut locs = std::mem::take(&mut self.locs);
        for entry in &mut locs {
            self.update_entry(api, entry, frame);
        }
        self.locs = locs;
        let mut npcs = std::mem::take(&mut self.npcs);
        for entry in &mut npcs {
            let Owner::Npc(index) = entry.owner else {
                continue;
            };
            let Some(npc) = frame.npcs.iter().find(|n| n.index == index) else {
                continue;
            };
            if entry.motion != npc.motion {
                let sound = npc_sound(npc);
                match npc.resolved {
                    Some(resolved) if sound != -1 => {
                        if entry.sound == sound {
                            entry.motion = npc.motion;
                            entry.volume = resolved.volume;
                        } else {
                            let mut replace = false;
                            if entry.loop_sound.is_none() {
                                replace = true;
                            } else {
                                entry.volume -= 512;
                                if entry.volume <= 0 {
                                    fade_out_play(api, &mut entry.loop_sound, 100);
                                    replace = true;
                                }
                            }
                            if replace {
                                entry.volume = resolved.volume;
                                entry.sound = sound;
                                entry.motion = npc.motion;
                            }
                        }
                    }
                    _ => {
                        entry.sound = -1;
                        entry.motion = npc.motion;
                    }
                }
            }
            entry.min_x = npc.trans[0] as i32;
            entry.max_x = npc.trans[0] as i32 + (npc.size << 8);
            entry.min_z = npc.trans[1] as i32;
            entry.max_z = npc.trans[1] as i32 + (npc.size << 8);
            entry.level = npc.level;
            self.update_entry(api, entry, frame);
        }
        self.npcs = npcs;
        let mut players = std::mem::take(&mut self.players);
        for entry in &mut players {
            let Owner::Player(index) = entry.owner else {
                continue;
            };
            let Some(player) = frame.players.iter().find(|p| p.index == index) else {
                continue;
            };
            if entry.motion != player.motion {
                let sound = player.sound();
                if entry.sound == sound {
                    entry.volume = player.volume;
                    entry.motion = player.motion;
                } else {
                    let mut replace = false;
                    if entry.loop_sound.is_none() {
                        replace = true;
                    } else {
                        entry.volume -= 512;
                        if entry.volume <= 0 {
                            fade_out_play(api, &mut entry.loop_sound, 100);
                            replace = true;
                        }
                    }
                    if replace {
                        entry.volume = player.volume;
                        entry.sound = sound;
                        entry.motion = player.motion;
                    }
                }
            }
            entry.min_x = player.trans[0] as i32;
            entry.max_x = player.trans[0] as i32 + (player.size << 8);
            entry.min_z = player.trans[1] as i32;
            entry.max_z = player.trans[1] as i32 + (player.size << 8);
            entry.level = player.level;
            self.update_entry(api, entry, frame);
        }
        self.players = players;
    }

    /// Create, move or fade out one entry's loop and random sounds.
    fn update_entry(&mut self, api: &mut AudioApi, entry: &mut Entry, frame: &Frame<'_>) {
        if entry.sound == -1 && entry.random.is_none() {
            return;
        }
        let volume = entry.volume;
        if entry.range != 0 && frame.background_volume != 0 && entry.level == frame.level {
            if let Some(key) = entry.loop_sound {
                if stopped(api, key) {
                    api.sound_play(key);
                    entry.loop_sound = None;
                }
            }
            if let Some(key) = entry.loop_sound {
                entry.position = entry.centre();
                api.sound_set_position(key, entry.position);
            } else if entry.sound >= 0 {
                entry.position = entry.centre();
                let params = SoundParams::new(
                    SoundType::PositionedLoop,
                    entry.sound,
                    sub_buss_type::LOCATION_GENERIC_SUB,
                )
                .with_loops(-1)
                .with_volume(0)
                .with_rate(256)
                .positioned(
                    SoundShape::Stereo,
                    entry.position,
                    entry.dropoffrange as f32,
                    entry.range as f32,
                );
                entry.loop_sound = api.create_owned_sound(&params);
                if let Some(key) = entry.loop_sound {
                    api.sound_set_volume(key, volume as f32 / 255.0, 150);
                    api.sound_start(key);
                }
            }
            if let Some(key) = entry.random_sound {
                entry.random_position = entry.centre();
                api.sound_set_position(key, entry.random_position);
                if stopped(api, key) {
                    api.sound_play(key);
                    entry.random_sound = None;
                }
            } else if let Some(random) = entry.random.clone() {
                entry.delay -= frame.scene_delta;
                if entry.delay <= 0 {
                    let rate = if entry.maxrate == 256 && entry.minrate == 256 {
                        256
                    } else {
                        ((self.random)() * f64::from(entry.maxrate - entry.minrate)) as i32
                            + entry.minrate
                    };
                    let pick = ((self.random)() * random.len() as f64) as usize;
                    entry.random_position = entry.centre();
                    let params = SoundParams::new(
                        SoundType::PositionedRandom,
                        random[pick],
                        sub_buss_type::LOCATION_RANDOM_SUB,
                    )
                    .with_loops(0)
                    .with_volume(volume)
                    .with_rate(rate)
                    .positioned(
                        SoundShape::Stereo,
                        entry.random_position,
                        entry.dropoffrange as f32,
                        (entry.range + entry.dropoffrange) as f32,
                    );
                    entry.random_sound = api.create_owned_sound(&params);
                    if let Some(key) = entry.random_sound {
                        api.sound_start(key);
                    }
                    entry.delay = entry.mindelay
                        + ((self.random)() * f64::from(entry.maxdelay - entry.mindelay)) as i32;
                }
            }
        } else {
            fade_out_play(api, &mut entry.loop_sound, 100);
        }
    }

    /// Diagnostics: `(kind, id, level, centre, sound, volume, range,
    /// dropoff, loop key)` of every entry with a live loop or random sound.
    pub fn active(&self) -> impl Iterator<Item = &Entry> {
        self.locs
            .iter()
            .chain(&self.npcs)
            .chain(&self.players)
            .filter(|e| e.loop_sound.is_some() || e.random_sound.is_some())
    }

    pub fn owner_label(entry: &Entry) -> String {
        match entry.owner {
            Owner::Loc(id) => format!("loc {id}"),
            Owner::Npc(index) => format!("npc #{index}"),
            Owner::Player(index) => format!("player #{index}"),
        }
    }
}

#[cfg(test)]
mod tests;
