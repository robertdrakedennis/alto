//! CPU request/selection state only: no frame progression, mesh playback or audio.
use super::Error;
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Error>;
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::animation_state::*;
/// One spot-animation request: the effect id (-1 clears the slot), the packed
/// height and start delay parameter, the orientation, the start delay and the
/// flag that forces mode 1.
#[derive(Clone, Copy, Debug)]
pub struct SpotRequest {
    pub id: i32,
    pub param: i32,
    pub orientation: i32,
    pub delay: i32,
    pub flag: bool,
}
pub struct Config {
    pub sequences: BTreeMap<i32, Sequence>,
    pub effects: BTreeMap<i32, Effect>,
    pub slots: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Node {
    pub sequence: Option<Sequence>,
    pub time: i32,
    pub delay: i32,
    pub loops: i32,
    pub frame: i32,
    pub next: i32,
    pub finished: bool,
    pub mode: i32,
    pub flag: bool,
    pub overlay_delay: i32,
    /// Keyframe-set readiness belongs to this node, not the shared cache. Set
    /// only by successful model preparation.
    pub skeletal_range: Option<(i32, i32)>,
    /// Sequence-sound triggers the node raised and its owner has not played
    /// yet, oldest first: (sequence id, frame). A node nobody drains (the
    /// scene's model animation) keeps only the first [`SOUND_TRIGGERS_KEPT`].
    pub sound_triggers: Vec<(i32, i32)>,
}
/// How many undrained sound triggers a node keeps.
const SOUND_TRIGGERS_KEPT: usize = 8;
impl Node {
    /// Record that the current sequence reached `frame` (for a skeletal
    /// sequence, its time) and its sound row should play.
    pub fn note_sound_frame(&mut self, frame: i32) {
        if frame >= 0 && self.sound_triggers.len() < SOUND_TRIGGERS_KEPT {
            let id = self.id();
            self.sound_triggers.push((id, frame));
        }
    }
    /// Forget the trigger the last `set` raised (a random start replaces it).
    pub fn unnote_last_sound_frame(&mut self) {
        self.sound_triggers.pop();
    }
    /// The triggers raised since the last call, oldest first.
    pub fn take_sound_triggers(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.sound_triggers)
    }
    pub fn id(&self) -> i32 {
        self.sequence.as_ref().map_or(-1, |s| s.id)
    }
    pub fn set(&mut self, id: i32, delay: i32, mode: i32, c: &Config) -> Result<()> {
        if id == self.id() {
            return Ok(());
        }
        if id == -1 {
            self.sequence = None;
            self.skeletal_range = None;
            return Ok(());
        }
        let s = c
            .sequences
            .get(&id)
            .ok_or(Error::UnsupportedContext("animation sequence"))?
            .clone();
        if s.frames.is_none() && !s.skeletal {
            self.sequence = None;
            return Ok(());
        }
        self.sequence = Some(s.clone());
        self.skeletal_range = None;
        self.loops = 0;
        self.delay = delay;
        self.mode = mode;
        self.flag = false;
        self.time = 0;
        if !s.skeletal {
            self.frame = 0;
            self.next = if s.frames.unwrap() > 1 { 1 } else { -1 };
        }
        self.finished = false;
        // A skeletal sequence's first tick sounds whatever the delay.
        if delay == 0 || s.skeletal {
            self.note_sound_frame(0);
        }
        Ok(())
    }
    pub fn restart(&mut self, delay: i32) {
        let Some(s) = &self.sequence else { return };
        if !s.skeletal {
            self.skeletal_range = None;
            self.frame = 0;
            self.next = if s.frames.unwrap_or(0) > 1 { 1 } else { -1 };
        }
        self.time = 0;
        self.finished = false;
        self.delay = delay;
        self.loops = 0;
        // The first frame's sound plays at a restart whatever the delay.
        self.note_sound_frame(0);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spot {
    pub id: i32,
    pub height: i32,
    pub orientation: i32,
    pub delay: i32,
    pub looping: bool,
    pub cancels_on_move: bool,
    pub node: Node,
}
impl Default for Spot {
    fn default() -> Self {
        Self {
            id: -1,
            height: 0,
            orientation: 0,
            delay: 0,
            looping: false,
            cancels_on_move: false,
            node: Node::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AnimationState {
    pub main: Node,
    pub modes: Option<Vec<i32>>,
    pub overlays: Vec<Option<Node>>,
    pub spots: [Spot; 5],
}
impl AnimationState {
    /// The `RESET_ANIMS` packet: clear the movement animation selection and
    /// current sequence while preserving spot/overlay animations.
    pub fn reset_main(&mut self) {
        self.modes = None;
        self.main.sequence = None;
        self.main.skeletal_range = None;
    }

    pub fn select_modes(
        &mut self,
        modes: Vec<i32>,
        delay: i32,
        npc: bool,
        route: usize,
        steps: &mut i32,
        c: &Config,
    ) -> Result<()> {
        if self.modes.as_ref() == Some(&modes) {
            if let Some(s) = &self.main.sequence {
                match s.restart {
                    1 => self.main.restart(delay),
                    2 => self.main.loops = 0,
                    _ => {}
                }
            }
        }
        let mut all_clear = true;
        for i in 0..modes.len() {
            if modes[i] != -1 {
                all_clear = false
            }
            let replace = if let Some(old) = &self.modes {
                if old[i] == -1 {
                    true
                } else {
                    // An id of -1 lists as a fresh default sequence type with
                    // priority 5. Unknown positive IDs
                    // remain an error rather than being silently accepted.
                    let new_priority = if modes[i] == -1 {
                        5
                    } else {
                        c.sequences
                            .get(&modes[i])
                            .ok_or(Error::UnsupportedContext("sequence priority"))?
                            .priority
                    };
                    let old = c
                        .sequences
                        .get(&old[i])
                        .ok_or(Error::UnsupportedContext("prior sequence priority"))?;
                    new_priority >= old.priority
                }
            } else {
                true
            };
            if replace {
                self.modes = Some(modes.clone());
                self.main.delay = delay;
                if npc {
                    *steps = route as i32;
                }
            }
        }
        if all_clear {
            self.modes = Some(modes);
            self.main.delay = delay;
            if npc {
                *steps = route as i32;
            }
        }
        Ok(())
    }
    pub fn overlay(&mut self, id: i32, delay: i32, mask: i32, c: &Config) -> Result<()> {
        if self.overlays.is_empty() {
            self.overlays = vec![None; c.slots]
        }
        if self.overlays.len() != c.slots {
            return Err(Error::Invalid("overlay slots"));
        }
        let mut bits = mask as u32;
        for node in &mut self.overlays {
            if bits == 0 {
                break;
            }
            if bits & 1 != 0 {
                if id == -1 {
                    *node = None
                } else {
                    let seq = c
                        .sequences
                        .get(&id)
                        .ok_or(Error::UnsupportedContext("overlay sequence"))?;
                    if let Some(n) = node {
                        if let Some(old) = &n.sequence {
                            if id == old.id {
                                match seq.restart {
                                    0 => *node = None,
                                    1 => {
                                        n.restart(0);
                                        n.overlay_delay = delay
                                    }
                                    2 => n.loops = 0,
                                    _ => {}
                                }
                            } else if seq.priority >= old.priority {
                                *node = None;
                            }
                        }
                    }
                    if node.as_ref().is_none_or(|n| n.sequence.is_none()) {
                        let mut n = Node::default();
                        n.set(id, 0, 0, c)?;
                        n.overlay_delay = delay;
                        *node = Some(n);
                    }
                }
            }
            bits >>= 1;
        }
        Ok(())
    }
    /// Start, restart or clear the spot animation in slot `index`.
    pub fn spot(&mut self, index: usize, request: SpotRequest, c: &Config) -> Result<()> {
        let SpotRequest {
            id,
            param,
            orientation,
            delay,
            flag,
        } = request;
        let s = &mut self.spots[index];
        if id != -1 && s.id != -1 {
            if id == s.id {
                let t = c
                    .effects
                    .get(&id)
                    .ok_or(Error::UnsupportedContext("spot type"))?;
                if t.looping && t.sequence != -1 {
                    match c
                        .sequences
                        .get(&t.sequence)
                        .ok_or(Error::UnsupportedContext("spot restart"))?
                        .restart
                    {
                        0 => return Ok(()),
                        2 => {
                            s.node.loops = 0;
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            } else {
                let n = c
                    .effects
                    .get(&id)
                    .ok_or(Error::UnsupportedContext("new spot type"))?;
                let old = c
                    .effects
                    .get(&s.id)
                    .ok_or(Error::UnsupportedContext("old spot type"))?;
                if n.sequence != -1
                    && old.sequence != -1
                    && c.sequences
                        .get(&n.sequence)
                        .ok_or(Error::UnsupportedContext("new spot sequence"))?
                        .priority
                        < c.sequences
                            .get(&old.sequence)
                            .ok_or(Error::UnsupportedContext("old spot sequence"))?
                            .priority
                {
                    return Ok(());
                }
            }
        }
        let effect = if id == -1 {
            None
        } else {
            Some(
                c.effects
                    .get(&id)
                    .ok_or(Error::UnsupportedContext("spot type"))?,
            )
        };
        let mut mode = if effect.is_some_and(|e| !e.looping) {
            2
        } else {
            0
        };
        if id != -1 && flag {
            mode = 1
        }
        s.id = id;
        s.delay = delay;
        s.height = param >> 16;
        s.orientation = orientation;
        s.looping = effect.is_some_and(|e| e.looping);
        s.cancels_on_move = if let Some(t) = effect {
            t.looping
                && t.sequence != -1
                && c.sequences
                    .get(&t.sequence)
                    .ok_or(Error::UnsupportedContext("spot movement priority"))?
                    .stationary
                    == 1
        } else {
            false
        };
        s.node
            .set(effect.map_or(-1, |e| e.sequence), param & 65535, mode, c)
    }
    /// Player and NPC moves cancel priority-one sequences.
    pub fn cancel_for_move(&mut self) {
        if self
            .main
            .sequence
            .as_ref()
            .is_some_and(|s| s.stationary == 1)
        {
            self.modes = None;
            self.main.sequence = None;
        }
        for s in &mut self.spots {
            if s.id != -1 && s.cancels_on_move {
                s.node.sequence = None;
                s.id = -1;
            }
        }
    }
}

impl AnimationState {
    /// Main animation selection by speed: selection only, no playback.
    pub fn select_for_speed(
        &mut self,
        speed: i8,
        route: usize,
        steps: &mut i32,
        c: &Config,
    ) -> Result<()> {
        if let Some(modes) = &self.modes {
            let id = *modes
                .get((speed as i32 + 1) as usize)
                .ok_or(Error::Invalid("movement speed animation"))?;
            if id != self.main.id() {
                self.main.set(id, self.main.delay, 0, c)?;
                *steps = route as i32;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(priority: i32) -> Config {
        Config {
            sequences: [(
                7,
                Sequence {
                    id: 7,
                    frames: Some(1),
                    skeletal: false,
                    restart: 0,
                    priority,
                    stationary: 0,
                    moving: 0,
                },
            )]
            .into_iter()
            .collect(),
            effects: BTreeMap::new(),
            slots: 4,
        }
    }

    #[test]
    fn clear_modes_uses_default_sequence_priority() {
        let mut state = AnimationState::default();
        let mut old = Node::default();
        old.set(7, 0, 0, &config(6)).unwrap();
        state.main = old;
        state.modes = Some(vec![7, -1, -1, -1]);
        let mut steps = 0;
        state
            .select_modes(vec![-1, -1, -1, -1], 3, false, 0, &mut steps, &config(6))
            .unwrap();
        assert_eq!(state.modes, Some(vec![-1, -1, -1, -1]));
        assert_eq!(state.main.delay, 3);
    }

    #[test]
    fn unknown_positive_sequence_still_fails() {
        let mut state = AnimationState {
            modes: Some(vec![7, -1, -1, -1]),
            ..Default::default()
        };
        let mut steps = 0;
        assert!(state
            .select_modes(vec![99, -1, -1, -1], 0, false, 0, &mut steps, &config(6))
            .is_err());
    }
}
