//! The bus tree: main busses (fed by the volume preferences) and the
//! sub-busses sounds are routed to, with per-bus priority and volume easing.

use std::collections::HashMap;

/// Ids of the main busses: `MASTER` is -1, the others are tagged with
/// `0x1000000`.
#[allow(dead_code)]
pub mod buss_type {
    pub const MASTER: i32 = -1;
    pub const SFX: i32 = 0x100_0000;
    pub const MUSIC: i32 = 5 | 0x100_0000;
    pub const MUSIC_LOGIN: i32 = 3 | 0x100_0000;
    pub const AMBIENT: i32 = 2 | 0x100_0000;
    pub const VOICEOVER: i32 = 4 | 0x100_0000;
}

/// Ids of the sub-busses, tagged with `0x2000000`.
#[allow(dead_code)]
pub mod sub_buss_type {
    pub const SFX_SUB: i32 = 8 | 0x200_0000;
    pub const MUSIC_SUB: i32 = 0x200_0000;
    pub const DIALOG_SUB: i32 = 1 | 0x200_0000;
    pub const PLAYER_ANIMATION_SUB: i32 = 4 | 0x200_0000;
    pub const NPC_ANIMATION_SUB: i32 = 7 | 0x200_0000;
    pub const LOCATION_ANIMATION_SUB: i32 = 6 | 0x200_0000;
    pub const GENERAL_ANIMATION_SUB: i32 = 9 | 0x200_0000;
    pub const LOCATIONS_SUB: i32 = 2 | 0x200_0000;
    pub const LOCATION_RANDOM_SUB: i32 = 5 | 0x200_0000;
    pub const LOCATION_GENERIC_SUB: i32 = 3 | 0x200_0000;
}

/// Which volume preference drives a main bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeProvider {
    /// The sound effects volume.
    MainEffects,
    /// The music volume.
    MainMusic,
    /// The login music volume.
    LoginMusic,
    /// The background (ambient) sound volume.
    BackgroundEffects,
    /// The speech volume.
    Speech,
}

/// The volume preferences the providers read (0-255).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VolumePreferences {
    pub sound: i32,
    pub background_sound: i32,
    pub speech: i32,
    pub music: i32,
    pub login_music: i32,
}

impl VolumeProvider {
    fn volume(self, prefs: &VolumePreferences) -> f32 {
        let value = match self {
            Self::MainEffects => prefs.sound,
            Self::MainMusic => prefs.music,
            Self::LoginMusic => prefs.login_music,
            Self::BackgroundEffects => prefs.background_sound,
            Self::Speech => prefs.speech,
        };
        value as f32 / 255.0
    }
}

/// One bus: a volume, a priority weight, an optional preference that drives
/// its volume, and its parent.
#[derive(Clone, Debug)]
pub struct AudioBuss {
    volume: f32,
    parent: Option<i32>,
    provider: Option<VolumeProvider>,
    priority: f32,
    /// The volume being eased to (negative: not easing) and from.
    ease_to: f32,
    ease_from: f32,
    /// When the ease started and ends.
    ease_start: i64,
    ease_end: i64,
}

impl AudioBuss {
    fn new(priority: f32, provider: Option<VolumeProvider>, parent: Option<i32>) -> Self {
        Self {
            volume: 1.0,
            parent,
            provider,
            priority,
            ease_to: -1.0,
            ease_from: -1.0,
            ease_start: -1,
            ease_end: -1,
        }
    }

    /// Follow the preference (easing over 100 ms when it changes) and
    /// advance any ease in progress.
    fn update(&mut self, now: i64, prefs: &VolumePreferences) {
        if let Some(provider) = self.provider {
            let target = provider.volume(prefs);
            if self.volume != target && self.ease_to < 0.0 {
                self.ease_from = self.volume;
                self.ease_to = target;
                self.ease_start = now;
                self.ease_end = self.ease_start + 100;
            }
        }
        if self.ease_to >= 0.0 {
            if now > self.ease_end {
                self.volume = self.ease_to;
                self.ease_to = -1.0;
            } else {
                let delta = self.ease_to - self.ease_from;
                let span = self.ease_end - self.ease_start;
                let slope = delta / span as f32;
                self.volume = (now - self.ease_start) as f32 * slope + self.ease_from;
                if self.volume == self.ease_to {
                    self.ease_to = -1.0;
                }
            }
        }
        self.volume = self.volume.clamp(0.0, 1.0);
    }

    /// This bus's own volume, without its ancestors'
    pub fn self_volume(&self) -> f32 {
        self.volume
    }

    /// Ease a provider-less bus to `target` over 100 ms.
    fn ease(&mut self, target: f32, now: i64) {
        if self.provider.is_none() {
            self.ease_to = target;
            self.ease_from = self.volume;
            self.ease_start = now;
            self.ease_end = self.ease_start + 100;
        }
    }

    /// The parent bus id.
    pub fn parent(&self) -> Option<i32> {
        self.parent
    }
}

/// The bus tree.
#[derive(Clone, Debug, Default)]
pub struct BussManager {
    busses: HashMap<i32, AudioBuss>,
}

impl BussManager {
    /// Update every bus.
    pub fn update(&mut self, now: i64, prefs: &VolumePreferences) {
        for buss in self.busses.values_mut() {
            buss.update(now, prefs);
        }
    }

    /// Add a bus under `parent` (-1 or unknown: a root). Returns false if
    /// the id exists.
    pub fn add(
        &mut self,
        id: i32,
        parent: i32,
        priority: f32,
        provider: Option<VolumeProvider>,
    ) -> bool {
        if self.busses.contains_key(&id) {
            return false;
        }
        let parent = (parent != -1 && self.busses.contains_key(&parent)).then_some(parent);
        self.busses
            .insert(id, AudioBuss::new(priority, provider, parent));
        true
    }

    /// A bus by id.
    pub fn get(&self, id: i32) -> Option<&AudioBuss> {
        self.busses.get(&id)
    }

    /// Ease a provider-less bus. Returns false if the bus does not exist.
    pub fn ease(&mut self, id: i32, target: f32, now: i64) -> bool {
        match self.busses.get_mut(&id) {
            Some(buss) => {
                buss.ease(target, now);
                true
            }
            None => false,
        }
    }

    /// A bus's volume times its ancestors', clamped to 0-1.
    pub fn volume(&self, id: i32) -> Option<f32> {
        let buss = self.busses.get(&id)?;
        let mut volume = buss.volume;
        let mut parent = buss.parent;
        while let Some(p) = parent.and_then(|p| self.busses.get(&p)) {
            volume *= p.self_volume();
            parent = p.parent;
        }
        Some(volume.clamp(0.0, 1.0))
    }

    /// The product of the priority weights from a bus up to the root.
    pub fn priority(&self, id: i32) -> Option<f32> {
        let mut buss = self.busses.get(&id)?;
        let mut priority = 1.0_f32;
        loop {
            priority *= buss.priority;
            match buss.parent.and_then(|p| self.busses.get(&p)) {
                Some(parent) => buss = parent,
                None => return Some(priority),
            }
        }
    }

    /// Whether one bus is an ancestor of the other. An unknown first id
    /// walks as an empty chain, which matches any existing second chain at
    /// its root.
    pub fn common_parent_exists(&self, a: i32, b: i32) -> bool {
        if a == b {
            return true;
        }
        let first = self.busses.contains_key(&a).then_some(a);
        let second = self.busses.contains_key(&b).then_some(b);
        let parent = |id: Option<i32>| {
            id.and_then(|id| self.busses.get(&id))
                .and_then(|b| b.parent)
        };
        let mut walk = first;
        loop {
            if walk.is_none() {
                let mut other = second;
                loop {
                    if other.is_none() {
                        return false;
                    }
                    other = parent(other);
                    if first == other {
                        return true;
                    }
                    if second == other {
                        return false;
                    }
                }
            }
            walk = parent(walk);
            if second == walk {
                return true;
            }
            if first == walk {
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buss_tree_matches_routing_architecture() {
        let mut buss = BussManager::default();
        buss.add(
            buss_type::SFX,
            buss_type::MASTER,
            0.2,
            Some(VolumeProvider::MainEffects),
        );
        buss.add(sub_buss_type::SFX_SUB, buss_type::SFX, 1.0, None);
        buss.add(
            buss_type::VOICEOVER,
            buss_type::MASTER,
            1.0,
            Some(VolumeProvider::Speech),
        );
        buss.add(sub_buss_type::DIALOG_SUB, buss_type::VOICEOVER, 1.0, None);
        let prefs = VolumePreferences {
            sound: 127,
            speech: 255,
            ..Default::default()
        };
        buss.update(1000, &prefs);
        buss.update(1200, &prefs);
        let expected = 127.0 / 255.0;
        assert!((buss.volume(sub_buss_type::SFX_SUB).unwrap() - expected).abs() < 1e-6);
        assert!((buss.priority(sub_buss_type::SFX_SUB).unwrap() - 0.2).abs() < 1e-6);
        assert!(buss.common_parent_exists(sub_buss_type::DIALOG_SUB, buss_type::VOICEOVER));
        assert!(buss.common_parent_exists(buss_type::VOICEOVER, sub_buss_type::DIALOG_SUB));
        assert!(!buss.common_parent_exists(sub_buss_type::SFX_SUB, sub_buss_type::DIALOG_SUB));
        // An unknown first bus walks to the root.
        assert!(buss.common_parent_exists(1234, sub_buss_type::DIALOG_SUB));
        // Provider-less busses ease over 100 ms.
        assert!(buss.ease(sub_buss_type::SFX_SUB, 0.5, 2000));
        buss.update(2050, &prefs);
        let mid = buss.volume(sub_buss_type::SFX_SUB).unwrap() / expected;
        assert!((mid - 0.75).abs() < 1e-3, "{mid}");
    }
}
