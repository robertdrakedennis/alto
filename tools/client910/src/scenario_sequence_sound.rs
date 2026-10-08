//! Sequence sounds of spot animations and projectiles, end to end: zone
//! packets of the dev server's wire form reach the game's transient update,
//! which raises the sound of the effect's sequence; the audio owner plays it
//! at a volume that depends on whom the effect is aimed at.
//!
//! The effect is a cache effect whose sequence has a sound on its first frame
//! and a remote volume percentage (a combat spell graphic): it is scaled to
//! that percentage unless it is aimed at the local player's own target or the
//! server's active target.

use super::scenario_tests::{live_step, replay_to, ui};
use super::session_replay::{server_frame, Replay};
use crate::proto::server as sp;
use rs910_symbols::spot;

/// The effect plays sequence 668: a sound on frame 0, remote volume 50%.
const EFFECT: i32 = spot::CONFUSE_CAST.id();
/// Another player's target id (`-index - 1`): nobody the local player or the
/// server has targeted.
const STRANGER: i32 = -30;

/// The local player's scene-local tile, level and target id.
fn local(replay: &Replay) -> (i32, i32, i32, i32) {
    let game = replay.game();
    let me = game.runtime.feed.state.players.players[game.runtime.map.local]
        .as_ref()
        .expect("local player");
    (
        me.x[0],
        me.z[0],
        me.level,
        -(game.runtime.map.local as i32) - 1,
    )
}

/// One world frame group: the zone origin and a spot animation on the
/// player's tile, aimed at `targeted`.
fn spot_frames(replay: &Replay, targeted: i32) -> Vec<Vec<u8>> {
    let (x, z, level, _) = local(replay);
    let coord = (((x & 7) << 4) | (z & 7)) as u8;
    let mut spot = vec![coord];
    spot.extend((EFFECT as u16).to_be_bytes());
    spot.extend(0u16.to_be_bytes()); // height offset
    spot.extend(0u16.to_be_bytes()); // delay
    spot.push(0); // orientation
    spot.extend((targeted as i16).to_be_bytes());
    vec![
        server_frame(
            sp::UPDATE_ZONE_PARTIAL_FOLLOWS,
            &[level as u8, (-(z >> 3)) as u8, (x >> 3) as u8],
        ),
        server_frame(sp::MAP_ANIM, &spot),
    ]
}

/// The same for a projectile one tile east, aimed at nobody in particular.
fn projectile_frames(replay: &Replay, targeted: i32) -> Vec<Vec<u8>> {
    let (x, z, level, _) = local(replay);
    let mut projectile = vec![(((x & 7) << 3) | (z & 7)) as u8, 1, 0];
    projectile.extend(0u16.to_be_bytes()); // target
    projectile.extend((EFFECT as u16).to_be_bytes());
    projectile.extend([8, 8]); // start and end heights
    projectile.extend(5u16.to_be_bytes()); // start delay
    projectile.extend(40u16.to_be_bytes()); // end delay
    projectile.push(16); // pitch
    projectile.extend(32u16.to_be_bytes()); // arc
    projectile.extend((targeted as i16).to_be_bytes());
    vec![
        server_frame(
            sp::UPDATE_ZONE_PARTIAL_FOLLOWS,
            &[level as u8, (-(z >> 3)) as u8, (x >> 3) as u8],
        ),
        server_frame(sp::MAP_PROJANIM, &projectile),
    ]
}

/// The volume of the first sound the audio owner was asked for over the next
/// few cycles of `frames`, or `None` when there was none.
fn first_sound_volume(
    replay: &mut Replay,
    cycle: &mut i32,
    frames: Vec<Vec<u8>>,
) -> anyhow::Result<Option<i32>> {
    ui(replay).audio.take_requests();
    live_step(replay, cycle, &frames)?;
    let mut volume = None;
    for _ in 0..3 {
        live_step(replay, cycle, &[])?;
        ui(replay).paint(*cycle, true, [0.; 3])?;
        let requests = ui(replay).audio.take_requests();
        volume = volume.or(requests.first().map(|r| r.volume));
    }
    Ok(volume)
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn spot_animation_sound_is_scaled_unless_aimed_at_the_local_players_target() -> anyhow::Result<()> {
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    let (_, _, _, own) = local(&replay);
    let aimed_at = |replay: &mut Replay, cycle: &mut i32, targeted| {
        let frames = spot_frames(replay, targeted);
        first_sound_volume(replay, cycle, frames)
    };
    let full = aimed_at(&mut replay, &mut cycle, 0)?.expect("an effect aimed at nothing plays");
    assert!(full > 1, "the sequence's own volume: {full}");
    // Aimed at someone nobody targeted: 50% of the sequence's volume.
    assert_eq!(
        aimed_at(&mut replay, &mut cycle, STRANGER)?,
        Some(full * 50 / 100)
    );
    // Aimed at the local player's own target id: full volume.
    assert_eq!(aimed_at(&mut replay, &mut cycle, own)?, Some(full));
    // The server's active target counts as the local player's.
    live_step(
        &mut replay,
        &mut cycle,
        &[server_frame(sp::SET_TARGET, &[0xff, 0x62])],
    )?;
    assert_eq!(
        ui(&mut replay).engine.scene.active_target,
        STRANGER,
        "SET_TARGET reached the scene state"
    );
    assert_eq!(aimed_at(&mut replay, &mut cycle, STRANGER)?, Some(full));
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn projectile_sound_plays_when_it_is_created_and_is_scaled_the_same_way() -> anyhow::Result<()> {
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    let (_, _, _, own) = local(&replay);
    let frames = projectile_frames(&replay, 0);
    let full = first_sound_volume(&mut replay, &mut cycle, frames)?
        .expect("a projectile plays its first frame's sound before it starts moving");
    let frames = projectile_frames(&replay, STRANGER);
    assert_eq!(
        first_sound_volume(&mut replay, &mut cycle, frames)?,
        Some(full * 50 / 100)
    );
    let frames = projectile_frames(&replay, own);
    assert_eq!(
        first_sound_volume(&mut replay, &mut cycle, frames)?,
        Some(full)
    );
    Ok(())
}
