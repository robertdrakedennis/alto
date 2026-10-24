//! The `sound_*` requests retained for the audio service and
//! `random_sound_pitch`.
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::absent;
use super::super::Engine;
use super::super::SoundRequest;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_sound(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let sound_args = match c.command {
            "sound_synth" => Some(3),
            "sound_song" => Some(1),
            "sound_jingle" => Some(2),
            "sound_song_volume" | "sound_jingle_volume" => Some(3),
            "sound_synth_volume" | "sound_speech_volume" | "sound_vorbis_volume" => Some(4),
            "sound_synth_rate" | "sound_vorbis_rate" => Some(5),
            "sound_vorbis_volume_rate_group" => Some(6),
            "sound_group_start" | "sound_group_stop" => Some(1),
            "sound_song_stop" => Some(0),
            // sound_mixbuss_add / sound_mixbuss_setlevel /
            // sound_distancefocusfilter_setparams.
            "sound_mixbuss_add" => Some(3),
            "sound_mixbuss_setlevel" => Some(2),
            "sound_distancefocusfilter_setparams" => Some(5),
            _ => None,
        };

        if let Some(count) = sound_args {
            if ints.len() < count {
                return Err(VmError::StackUnderflow { stack: "int" });
            }
            let request = ints.split_off(ints.len() - count);
            self.effects.sounds.push(SoundRequest {
                command: c.command.to_string(),
                args: request,
            });
            return Ok(None);
        }
        // Unreachable: the table routes only the names above here.
        Err(absent(c.command))
    }

    // random_sound_pitch. This is a script value,
    // independent of the audio playback service.
    pub(super) fn cmd_random_sound_pitch(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let b = ints.pop().unwrap();
        let a = ints.pop().unwrap();
        let pitch = if a <= 700 && b <= 700 {
            (2f64.powf((self.next_double() * a.wrapping_add(b) as f64 - a as f64 + 800.0) / 100.0)
                + 0.5) as i32
        } else {
            256
        };
        Ok(Some(Value::Int(pitch)))
    }
}
