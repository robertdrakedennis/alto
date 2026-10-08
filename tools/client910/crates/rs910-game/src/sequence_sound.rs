//! Sequence sounds: the sound row a sequence plays when an animation node
//! reaches a frame, for every kind of emitter (players, NPCs, spot
//! animations and projectiles). The row is resolved here (gates, alternate
//! id, rate draw); the audio owner applies the preferences, the remote
//! volume rule and the positioning.

use crate::animation_playback::AnimationRandom;
use crate::entities910::animation_state::Node;
use crate::protocol910::sequence_types::Sequence;
use std::collections::BTreeMap;

/// One sequence-sound request that passed its level/visibility gate. The
/// volume is the sequence's per-frame volume; the preference mute and the
/// remote-volume attenuation are applied by the audio owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceSound {
    pub id: i32,
    pub loops: i32,
    pub range: i32,
    pub rate: i32,
    pub volume: i32,
    pub level: i32,
    /// The emitter's current position floored to a tile
    /// (`((x - 256) >> 9) << 9`), in scene-local fine units.
    pub x: i32,
    pub z: i32,
    /// The emitter is the local player.
    pub local: bool,
    /// The sequence's remote volume percentage (-1 when absent).
    pub target_volume: i32,
    /// The emitter's target id: `index + 1` for an NPC, `-index - 1` for a
    /// player, the packet's value for a spot animation or projectile, 0 for
    /// anything that cannot be targeted.
    pub targeted: i32,
    /// The local player's target id.
    pub local_targeted: i32,
}

/// What the sound of an emitter depends on, sampled when its animation frame
/// is reached.
#[derive(Clone, Copy, Debug)]
pub struct Emitter {
    pub level: i32,
    pub fine_x: f32,
    pub fine_z: f32,
    pub local: bool,
    /// Whether the emitter is visible: always true except for an NPC whose
    /// multi-NPC type resolves to nothing.
    pub visible: bool,
    pub targeted: i32,
    /// The local player's level and target id; `None` without one.
    pub listener: Option<(i32, i32)>,
}

/// The sound row of `frame` of `seq` for `emitter`, appended to `out` when
/// the emitter is on the listener's level, visible, and the row plays.
///
/// The random draws happen in the order the request makes them (alternate
/// id, then rate) and only once the gates pass.
pub fn emit(
    seq: &Sequence,
    frame: i32,
    emitter: Emitter,
    rng: &mut AnimationRandom,
    out: &mut Vec<SequenceSound>,
) {
    let Ok(frame) = usize::try_from(frame) else {
        return;
    };
    let Some(rows) = seq.sound.as_ref() else {
        return;
    };
    let Some(Some(row)) = rows.get(frame) else {
        return;
    };
    // The listener must be on the emitter's level and the emitter visible.
    let Some((listener_level, local_targeted)) = emitter.listener else {
        return;
    };
    if listener_level != emitter.level || !emitter.visible {
        return;
    }
    let Some(&first) = row.first() else {
        return;
    };
    // Loops and range always come from the first entry.
    let mut id = first >> 8;
    let loops = (first >> 5) & 0x7;
    let range = first & 0x1f;
    // An alternate entry is a raw sound id.
    if row.len() > 1 {
        let index = (rng.next() * row.len() as f64) as usize;
        if index > 0 {
            id = row[index.min(row.len() - 1)];
        }
    }
    // Playback rate: a random value between the frame's min and max.
    let rate = match (&seq.minrate, &seq.maxrate) {
        (Some(min), Some(max)) => {
            let low = *min.get(frame).unwrap_or(&256);
            let high = *max.get(frame).unwrap_or(&low);
            (rng.next() * f64::from(high.wrapping_sub(low))) as i32 + low
        }
        _ => 256,
    };
    let volume = seq
        .volume
        .as_ref()
        .and_then(|values| values.get(frame))
        .copied()
        .unwrap_or(255);
    // A zero range only plays for the local player.
    if range == 0 && !emitter.local {
        return;
    }
    out.push(SequenceSound {
        id,
        loops,
        range,
        rate,
        volume,
        level: emitter.level,
        x: ((emitter.fine_x as i32 - 256) >> 9) << 9,
        z: ((emitter.fine_z as i32 - 256) >> 9) << 9,
        local: emitter.local,
        target_volume: seq.remote_volume_percent,
        targeted: emitter.targeted,
        local_targeted,
    });
}

/// Play the sound rows a node's triggers name, oldest first; with no emitter
/// they are dropped. A trigger of a sequence that is no longer configured
/// plays nothing.
pub fn play_triggers(
    node: &mut Node,
    emitter: Option<Emitter>,
    sequences: &BTreeMap<i32, Sequence>,
    rng: &mut AnimationRandom,
    out: &mut Vec<SequenceSound>,
) {
    for (id, frame) in node.take_sound_triggers() {
        if let (Some(emitter), Some(seq)) = (emitter, sequences.get(&id)) {
            emit(seq, frame, emitter, rng, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMITTER: Emitter = Emitter {
        level: 1,
        fine_x: 1000.0,
        fine_z: 1800.0,
        local: false,
        visible: true,
        targeted: 5,
        listener: Some((1, -3)),
    };

    fn sequence(row: Vec<i32>) -> Sequence {
        let mut seq = Sequence::empty(7);
        seq.sound = Some(vec![Some(row)]);
        seq.volume = Some(vec![200]);
        seq.remote_volume_percent = 40;
        seq
    }

    /// The row's id, loops and range come from its first entry, an alternate
    /// entry is a raw sound id picked by the first random draw, and the
    /// position is the emitter's tile.
    #[test]
    fn a_row_resolves_to_the_requested_sound() {
        // id 0x123, loops 2, range 3.
        let seq = sequence(vec![(0x123 << 8) | (2 << 5) | 3, 999]);
        let mut sounds = vec![];
        let mut rng = AnimationRandom::new(1);
        emit(&seq, 0, EMITTER, &mut rng, &mut sounds);
        assert_eq!(
            sounds,
            [SequenceSound {
                // The seed-1 generator's first double, 0.7308781907032909,
                // picks entry (int) (0.73 * 2) = 1, the alternate.
                id: 999,
                loops: 2,
                range: 3,
                rate: 256,
                volume: 200,
                level: 1,
                // ((1000 - 256) >> 9) << 9, ((1800 - 256) >> 9) << 9.
                x: 512,
                z: 1536,
                local: false,
                target_volume: 40,
                targeted: 5,
                local_targeted: -3,
            }]
        );
        // Another level, an invisible emitter, no local player, a frame
        // without a row, or a row without range for a remote emitter return
        // before any draw.
        let before = rng.clone();
        for emitter in [
            Emitter {
                listener: Some((0, -3)),
                ..EMITTER
            },
            Emitter {
                visible: false,
                ..EMITTER
            },
            Emitter {
                listener: None,
                ..EMITTER
            },
        ] {
            emit(&seq, 0, emitter, &mut rng, &mut sounds);
        }
        emit(&seq, 1, EMITTER, &mut rng, &mut sounds);
        emit(&seq, -1, EMITTER, &mut rng, &mut sounds);
        assert_eq!(sounds.len(), 1);
        assert_eq!(rng.next(), before.clone().next());
        let silent = sequence(vec![0x123 << 8]);
        emit(&silent, 0, EMITTER, &mut rng, &mut sounds);
        assert_eq!(sounds.len(), 1, "no range plays only for the local player");
        emit(
            &silent,
            0,
            Emitter {
                local: true,
                ..EMITTER
            },
            &mut rng,
            &mut sounds,
        );
        assert_eq!(sounds.len(), 2);
    }
}
