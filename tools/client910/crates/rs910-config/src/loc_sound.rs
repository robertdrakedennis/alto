//! The loc ambient sounds a map build emits (the loc types'
//! `bgsound`/`randomsound` fields): data only, read by the scene's
//! loc placement and by the audio owner (`positioned_sound`). Split out of
//! client910's `positioned_sound` (Phase 2.8; target-architecture.md §2.3
//! "rebuild emits `LocSoundScene` data").

use std::collections::HashMap;
use std::sync::Arc;

/// The loc-type fields the positioned-sound system reads, plus
/// the footprint and the `multiloc` selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocSound {
    pub width: i32,
    pub length: i32,
    pub istexture: bool,
    pub sound: i32,
    pub range: i32,
    pub dropoffrange: i32,
    pub volume: i32,
    pub mindelay: i32,
    pub maxdelay: i32,
    pub random: Option<Vec<i32>>,
    pub minrate: i32,
    pub maxrate: i32,
    pub multivarbit: i32,
    pub multivarp: i32,
    /// `multiloc` (the last entry is the fallback), `None` when absent.
    pub multiloc: Option<Vec<i32>>,
}

impl LocSound {
    /// A default-constructed loc type (the list entry of a missing file).
    pub fn empty() -> Self {
        Self {
            width: 1,
            length: 1,
            sound: -1,
            volume: 255,
            minrate: 256,
            maxrate: 256,
            multivarbit: -1,
            multivarp: -1,
            ..Self::default()
        }
    }

    pub fn from_loc(loc: &crate::config::Loc) -> Self {
        Self {
            width: i32::from(loc.width),
            length: i32::from(loc.length),
            istexture: loc.istexture,
            sound: loc.bgsound_sound,
            range: loc.bgsound_range,
            dropoffrange: loc.bgsound_dropoffrange,
            volume: loc.bgsound_volume,
            mindelay: loc.bgsound_mindelay,
            maxdelay: loc.bgsound_maxdelay,
            random: loc.bgsound_random.clone(),
            minrate: loc.bgsound_minrate,
            maxrate: loc.bgsound_maxrate,
            multivarbit: loc.multivarbit,
            multivarp: loc.multivarp,
            multiloc: loc.has_multiloc.then(|| loc.multiloc.clone()),
        }
    }

    fn own_sound(&self) -> bool {
        self.sound != -1 || self.random.is_some()
    }
}

/// The loc type list subset positioned sound can reach: every loc with its
/// own background sound or a `multiloc` (other ids list as the default
/// type, which has neither).
#[derive(Clone, Debug, Default)]
pub struct LocSoundTable {
    entries: HashMap<u32, LocSound>,
}

impl LocSoundTable {
    pub fn from_store(store: &crate::config::LocStore) -> Self {
        let entries = store
            .iter()
            .filter(|(_, loc)| {
                loc.has_multiloc || loc.bgsound_sound != -1 || loc.bgsound_random.is_some()
            })
            .map(|(&id, loc)| (id, LocSound::from_loc(loc)))
            .collect();
        Self { entries }
    }

    #[allow(
        clippy::len_without_is_empty,
        reason = "the loc types with sound the map-load log line counts; nothing asks whether it is empty"
    )]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn from_entries(entries: HashMap<u32, LocSound>) -> Self {
        Self { entries }
    }

    /// The loc type list entry for `id`.
    pub fn list(&self, id: i32) -> LocSound {
        u32::try_from(id)
            .ok()
            .and_then(|id| self.entries.get(&id))
            .cloned()
            .unwrap_or_else(LocSound::empty)
    }

    /// Whether the loc, or any of its multiloc alternatives, plays a sound.
    pub fn has_background_sound(&self, id: i32) -> bool {
        let loc = self.list(id);
        match &loc.multiloc {
            None => loc.own_sound(),
            Some(ids) => ids.iter().any(|&id| id != -1 && self.list(id).own_sound()),
        }
    }

    /// The multiloc alternative selected by the var read; `None` when there is none.
    pub fn multi_loc(
        &self,
        loc: &LocSound,
        read: &dyn Fn(bool, i32) -> Option<i32>,
    ) -> Option<LocSound> {
        let list = loc.multiloc.as_ref()?;
        let id = crate::config::select_multi(loc.multivarbit, loc.multivarp, list, read)?;
        Some(self.list(id as i32))
    }
}

/// One ground-loc placement that reached the positioned-sound system:
/// level, scene tile, angle and loc id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocSoundSpawn {
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub angle: i32,
    pub id: u32,
}

/// A map load's loc sounds: the `addPositionalSound` calls in load order and
/// the loc types they (and later `multiloc`/`LOC_ADD_CHANGE` changes) read.
#[derive(Clone, Debug, Default)]
pub struct LocSoundScene {
    pub spawns: Vec<LocSoundSpawn>,
    pub table: Arc<LocSoundTable>,
}
