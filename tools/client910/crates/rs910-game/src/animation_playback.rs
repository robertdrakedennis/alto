//! Animation playback. Extends the imported
//! selection node with frame progression; sound callbacks are separate.
use crate::entities910::animation_state::{Config, Node};
use crate::protocol910::sequence_types::Sequence;
pub use rs910_core::animation_random::AnimationRandom;

pub fn start(
    node: &mut Node,
    seq: Option<&Sequence>,
    delay: i32,
    mode: i32,
    random: bool,
    rng: &mut AnimationRandom,
) -> anyhow::Result<()> {
    let id = seq.map_or(-1, |s| s.id);
    if node.id() == id {
        return Ok(());
    }
    let c = Config {
        sequences: seq.map(|s| (s.id, s.selection())).into_iter().collect(),
        effects: Default::default(),
        slots: 0,
    };
    node.set(id, delay, mode, &c)
        .map_err(|e| anyhow::anyhow!("animation selection: {e:?}"))?;
    if node.sequence.is_none() {
        return Ok(());
    }
    let seq = seq.unwrap();
    if random {
        // The start sound plays for the frame the random start picked.
        if delay == 0 || seq.skeletal != -1 {
            node.unnote_last_sound_frame();
        }
        if seq.skeletal != -1 {
            node.time = -1;
        } else {
            let frames = seq.frames.as_ref().unwrap();
            node.frame = (rng.next() * seq.frame_ids.as_ref().unwrap().len() as f64) as i32;
            node.time = (rng.next() * frames[node.frame as usize] as f64) as i32;
            node.next = node.frame + 1;
            if node.next >= frames.len() as i32 {
                node.next = -1;
            }
            if delay == 0 {
                node.note_sound_frame(node.frame);
            }
        }
    }
    Ok(())
}

/// Advance a node by `ticks`. Every point where the sequence's sound row is
/// due (the end of a start delay, each frame step, each skeletal tick) is
/// noted on the node for its owner to play.
///
/// The optional skeletal range is present only after the frame loader becomes
/// ready.
pub fn advance(
    node: &mut Node,
    seq: &Sequence,
    mut ticks: i32,
    skeletal_range: Option<(i32, i32)>,
    rng: &mut AnimationRandom,
) -> bool {
    if node.sequence.is_none() || ticks == 0 {
        return false;
    }
    if seq.skeletal != -1 {
        let Some((start, end)) = skeletal_range else {
            return false;
        };
        if node.time < 0 {
            node.time = start + (rng.next() * (end - start) as f64) as i32;
        }
        if node.delay > 0 {
            if node.delay > ticks {
                node.delay -= ticks;
                return false;
            }
            ticks -= node.delay;
            node.delay = 0;
        }
        for _ in 0..ticks {
            node.note_sound_frame(node.time);
            node.time = node.time.wrapping_add(1);
            if node.time >= end {
                if seq.replayoff == -1 || node.mode == 2 {
                    node.finished = true;
                } else {
                    node.time = (end - start).wrapping_sub(seq.replayoff);
                    if node.mode == 0 {
                        node.loops = node.loops.wrapping_add(1);
                    }
                    if node.loops >= seq.replaycount {
                        node.finished = true;
                    }
                }
            }
        }
        return ticks != 0;
    }
    if node.delay > 0 {
        if node.delay > ticks {
            node.delay -= ticks;
            return false;
        }
        ticks -= node.delay;
        node.delay = 0;
        node.note_sound_frame(node.frame);
    }
    let frames = seq.frames.as_ref().expect("classic sequence durations");
    let count = seq.frame_ids.as_ref().unwrap().len() as i32;
    let mut elapsed = node.time.wrapping_add(ticks);
    let mut changed = seq.tween;
    let next = |node: &mut Node| {
        node.next = node.frame + 1;
        if node.next >= count {
            if seq.replayoff == -1 && node.flag {
                node.next = 0;
            } else {
                node.next = node.next.wrapping_sub(seq.replayoff);
            }
            if node.next < 0 || node.next >= count {
                node.next = -1;
            }
        }
    };
    if elapsed > 100 && seq.replayoff > 0 {
        let first = count - seq.replayoff;
        while node.frame < first && elapsed > frames[node.frame as usize] {
            elapsed -= frames[node.frame as usize];
            node.frame += 1;
        }
        if node.frame >= first {
            let duration = frames[first as usize..count as usize]
                .iter()
                .fold(0i32, |s, &v| s.wrapping_add(v));
            if node.mode == 0 {
                node.loops = node.loops.wrapping_add(elapsed / duration);
            }
            elapsed %= duration;
        }
        next(node);
        changed = true;
    }
    while elapsed > frames[node.frame as usize] {
        changed = true;
        elapsed -= frames[node.frame as usize];
        node.frame += 1;
        if node.frame >= count {
            if seq.replayoff != -1 && node.mode != 2 {
                node.frame = node.frame.wrapping_sub(seq.replayoff);
                if node.mode == 0 {
                    node.loops = node.loops.wrapping_add(1);
                }
            }
            if node.loops >= seq.replaycount || node.frame < 0 || node.frame >= count {
                node.finished = true;
                break;
            }
        }
        node.note_sound_frame(node.frame);
        next(node);
    }
    node.time = elapsed;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities910::animation_state::Config;
    use std::collections::BTreeMap;

    /// The original client's animation node run over 80 scripted scenarios of
    /// set, restart and advance (`fixtures/recorded/animation-sound`): the
    /// frames whose sequence sound it raises, in order, and the node state
    /// after each step. Every start, restart, end of delay and frame step must
    /// raise exactly the sound triggers the original raises.
    #[test]
    fn sound_triggers_match_the_original_client() {
        use rs910_core::test_support::frozen;
        let cases = frozen::text("animation-sound/cases.txt");
        let expected = frozen::text("animation-sound/expected.txt");
        let mut want = expected.lines();
        let mut rng = AnimationRandom::new(1);
        let mut sequence = Sequence::empty(1);
        let mut node = Node::default();
        let mut config = Config {
            sequences: BTreeMap::new(),
            effects: BTreeMap::new(),
            slots: 0,
        };
        let mut checked = 0;
        for line in cases.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split(' ').collect();
            let int = |i: usize| f[i].parse::<i32>().unwrap();
            let got = match f[0] {
                "S" => {
                    let durations: Vec<i32> = f[5].split(',').map(|d| d.parse().unwrap()).collect();
                    sequence = Sequence::empty(1);
                    sequence.frame_ids = Some(vec![0; durations.len()]);
                    sequence.frames = Some(durations);
                    sequence.replayoff = int(2);
                    sequence.replaycount = int(3);
                    sequence.tween = int(4) == 1;
                    config.sequences = BTreeMap::from([(1, sequence.selection())]);
                    node = Node::default();
                    format!("S {}", &line[2..])
                }
                "E" => "E".to_string(),
                op => {
                    let mut changed = String::new();
                    if op == "adv" && node.finished {
                        format!("{line} -> skipped, the node is finished")
                    } else {
                        match op {
                            "set" => {
                                let sequence = Some(&sequence);
                                start(&mut node, sequence, int(1), int(2), false, &mut rng)
                                    .unwrap();
                            }
                            "restart" => node.restart(int(1)),
                            _ => {
                                changed = advance(&mut node, &sequence, int(1), None, &mut rng)
                                    .to_string()
                            }
                        }
                        let frames: String = node
                            .take_sound_triggers()
                            .iter()
                            .map(|(_, frame)| format!("{frame},"))
                            .collect();
                        format!(
                            "{line} -> [{frames}] {changed} frame={} time={} delay={} done={} loops={}",
                            node.frame, node.time, node.delay, node.finished, node.loops
                        )
                    }
                }
            };
            let want = want.next().unwrap();
            assert_eq!(got.trim_end(), want.trim_end());
            checked += 1;
        }
        assert!(checked > 700, "{checked} steps");
    }
}
