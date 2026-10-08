use super::*;
use crate::audio_api::AdjustContext;
use crate::audio_backend::{Output, VolumePreferences};
use crate::audio_stream::SoundLoader;

fn api() -> AudioApi {
    let loader = SoundLoader::new(None::<crate::audio_runtime::PackGroups>, true);
    AudioApi::new(loader, Output::Clock, false, 0)
}

const PREFS: VolumePreferences = VolumePreferences {
    sound: 255,
    background_sound: 255,
    speech: 255,
    music: 255,
    login_music: 255,
};

/// Update the audio API with the local player at `(x, z)`.
fn tick(api: &mut AudioApi, x: f32, z: f32) {
    api.update(0, &PREFS, &AdjustContext::default(), Some([x, 0.0, z]));
}

fn place(level: i32, x: i32, z: i32, angle: i32, id: i32) -> LocPlacement {
    LocPlacement {
        level,
        x,
        z,
        angle,
        id,
    }
}

fn fountain() -> LocSound {
    LocSound {
        width: 2,
        length: 3,
        sound: 700,
        range: 4,
        dropoffrange: 1,
        volume: 128,
        ..LocSound::empty()
    }
}

fn table(entries: Vec<(u32, LocSound)>) -> Arc<LocSoundTable> {
    Arc::new(LocSoundTable::from_entries(entries.into_iter().collect()))
}

fn frame<'a>(npcs: &'a [NpcSample], players: &'a [PlayerSample]) -> Frame<'a> {
    Frame {
        level: 0,
        scene_delta: 1,
        background_volume: 255,
        npcs,
        players,
    }
}

fn fixed(values: Vec<f64>) -> Box<dyn FnMut() -> f64> {
    let mut values = values.into_iter().cycle();
    Box::new(move || values.next().unwrap_or(0.0))
}

#[test]
fn motion_classes_pick_the_walk_sequence() {
    let bas = BasMotion {
        run: [10, 11, 12, 13],
        crawl: [20, -1, -1, 23],
    };
    assert_eq!(motion(&bas, -1, false), 0);
    assert_eq!(motion(&bas, 5, true), 0);
    assert_eq!(motion(&bas, 12, false), 2);
    assert_eq!(motion(&bas, 23, false), 3);
    assert_eq!(motion(&bas, 5, false), 1);
    // The default movement set: any sequence walks.
    assert_eq!(motion(&BasMotion::DEFAULT, 12, false), 1);
    let npc = NpcSound {
        sounds: [1, 2, 3, 4],
        range: 1,
        dropoffrange: 0,
        volume: 255,
        minrate: 256,
        maxrate: 256,
    };
    assert_eq!([0, 1, 2, 3].map(|m| npc.for_motion(m)), [1, 3, 4, 2]);
}

#[test]
fn loc_footprint_centre_and_ranges_follow_the_placement() {
    let mut api = api();
    let mut sounds = PositionedSounds::new(fixed(vec![0.5]));
    sounds.table = table(vec![(9, fountain())]);
    // Angle 1 swaps width/length.
    sounds.add_loc(&mut api, place(0, 10, 20, 1, 9), &|_, _| None);
    let e = &sounds.locs[0];
    assert_eq!(
        (e.min_x, e.max_x, e.min_z, e.max_z),
        (5120, 5120 + 3 * 512, 10240, 10240 + 2 * 512)
    );
    assert_eq!((e.range, e.dropoffrange, e.volume), (4 << 9, 1 << 9, 128));
    // (int) ((float) (max - min) * 0.5F + (float) min).
    assert_eq!(e.centre(), [5120.0 + 768.0, 0.0, 10240.0 + 512.0]);
}

/// The listener distance gate belongs to sound creation:
/// no loop exists until the local player is inside `range`, the loop
/// then fades to `volume / 255` over 150 ms and the stereo adjuster
/// attenuates between `dropoffrange` and `range`.
#[test]
fn loop_is_created_inside_range_and_attenuates_like_the_stereo_adjuster() {
    let mut api = api();
    let mut sounds = PositionedSounds::new(fixed(vec![0.5]));
    sounds.table = table(vec![(9, fountain())]);
    sounds.add_loc(&mut api, place(0, 10, 20, 0, 9), &|_, _| None);
    let centre = sounds.locs[0].centre();
    // 4 tiles + 1 fine unit away: outside `range` (2048).
    tick(&mut api, centre[0] + 2049.0, centre[2]);
    sounds.update(&mut api, &frame(&[], &[]));
    assert_eq!(sounds.locs[0].loop_sound, None);
    tick(&mut api, centre[0] + 1024.0, centre[2]);
    sounds.update(&mut api, &frame(&[], &[]));
    let key = sounds.locs[0].loop_sound.expect("loop inside range");
    assert_eq!(api.sound_status(key), SoundState::Starting);
    // SoundStereoAdjuster volume at 1024 with size 512, range 2048:
    // 1 - (1024 - 512) / (2048 - 512) = 2/3.
    let adjuster = crate::audio_api::Adjuster {
        shape: SoundShape::Stereo,
        position: centre,
        size: 512.0,
        range: 2048.0,
    };
    let ctx = AdjustContext {
        listener: api.listener(),
        ..AdjustContext::default()
    };
    let mut gains = [1.0, 1.0];
    adjuster.apply(&mut gains, &ctx);
    let power = (gains[0] * gains[0] + gains[1] * gains[1]) / 2.0;
    assert!((power.sqrt() - 2.0 / 3.0).abs() < 1e-3, "{gains:?}");
    // Leaving the level drops the loop with a 100 ms fade (:535-539).
    let mut upstairs = frame(&[], &[]);
    upstairs.level = 1;
    sounds.update(&mut api, &upstairs);
    assert_eq!(sounds.locs[0].loop_sound, None);
    assert_eq!(api.sound_status(key), SoundState::Stopped);
    assert!(
        api.active_sounds().contains(&key),
        "handed back with play()"
    );
    // backgroundSoundVolume 0 behaves the same.
    sounds.update(&mut api, &frame(&[], &[]));
    assert!(sounds.locs[0].loop_sound.is_some());
    let mut muted = frame(&[], &[]);
    muted.background_volume = 0;
    sounds.update(&mut api, &muted);
    assert_eq!(sounds.locs[0].loop_sound, None);
}

#[test]
fn random_sounds_draw_rate_index_then_delay() {
    let mut api = api();
    let loc = LocSound {
        sound: -1,
        range: 8,
        dropoffrange: 2,
        mindelay: 10,
        maxdelay: 30,
        random: Some(vec![100, 101, 102, 103]),
        minrate: 200,
        maxrate: 300,
        ..LocSound::empty()
    };
    // add: delay draw 0.5 -> 20; then rate 0.25 -> 225, index 0.5 -> 2,
    // delay 0.0 -> 10.
    let mut sounds = PositionedSounds::new(fixed(vec![0.5, 0.25, 0.5, 0.0]));
    sounds.table = table(vec![(5, loc)]);
    sounds.add_loc(&mut api, place(0, 10, 10, 0, 5), &|_, _| None);
    assert_eq!(sounds.locs[0].delay, 20);
    tick(&mut api, 10.0 * 512.0, 10.0 * 512.0);
    let mut f = frame(&[], &[]);
    f.scene_delta = 19;
    sounds.update(&mut api, &f);
    assert_eq!(sounds.locs[0].random_sound, None);
    assert_eq!(sounds.locs[0].delay, 1);
    f.scene_delta = 1;
    sounds.update(&mut api, &f);
    let e = &sounds.locs[0];
    let key = e.random_sound.expect("random sound due");
    assert_eq!(e.delay, 10);
    assert_eq!(api.sound_status(key), SoundState::Starting);
    // No loop: bgsound_sound is -1.
    assert_eq!(e.loop_sound, None);
}

#[test]
fn multiloc_selection_changes_the_sound_and_fades_the_old_loop() {
    let mut api = api();
    let base = LocSound {
        multivarp: 7,
        multiloc: Some(vec![20, 21, -1]),
        ..LocSound::empty()
    };
    let quiet = LocSound {
        sound: 300,
        range: 3,
        ..LocSound::empty()
    };
    let loud = LocSound {
        sound: 301,
        range: 5,
        volume: 64,
        ..LocSound::empty()
    };
    let mut sounds = PositionedSounds::new(fixed(vec![0.5]));
    sounds.table = table(vec![(1, base), (20, quiet), (21, loud)]);
    assert!(sounds.table.has_background_sound(1));
    let varp = std::cell::Cell::new(0);
    let read = |bit: bool, id: i32| (!bit && id == 7).then(|| varp.get());
    sounds.add_loc(&mut api, place(0, 10, 10, 0, 1), &read);
    assert!(sounds.locs[0].multisound);
    assert_eq!((sounds.locs[0].sound, sounds.locs[0].range), (300, 3 << 9));
    tick(&mut api, 5120.0, 5120.0);
    sounds.update(&mut api, &frame(&[], &[]));
    let old = sounds.locs[0].loop_sound.expect("loop");
    varp.set(1);
    sounds.refresh_multisounds(&mut api, &[], &read);
    assert_eq!((sounds.locs[0].sound, sounds.locs[0].volume), (301, 64));
    assert_eq!(sounds.locs[0].loop_sound, None);
    assert_eq!(api.sound_status(old), SoundState::Stopped);
    // The fallback -1 is "no loc": silence.
    varp.set(9);
    sounds.refresh_multisounds(&mut api, &[], &read);
    assert_eq!((sounds.locs[0].sound, sounds.locs[0].range), (-1, 0));
}

#[test]
fn rebuild_and_loc_changes_replace_the_tile_sound() {
    let mut api = api();
    let mut sounds = PositionedSounds::new(fixed(vec![0.5]));
    let quiet = LocSound {
        sound: 400,
        range: 2,
        ..LocSound::empty()
    };
    let scene = LocSoundScene {
        spawns: vec![
            LocSoundSpawn {
                level: 0,
                x: 30,
                z: 30,
                angle: 0,
                id: 9,
            },
            LocSoundSpawn {
                level: 0,
                x: 40,
                z: 40,
                angle: 0,
                id: 9,
            },
        ],
        table: table(vec![(9, fountain()), (10, quiet)]),
    };
    sounds.rebuild(&mut api, &scene, &|_, _| None);
    assert_eq!(sounds.locs.len(), 2);
    tick(&mut api, 30.0 * 512.0, 30.0 * 512.0);
    sounds.update(&mut api, &frame(&[], &[]));
    let first = sounds.locs[0].loop_sound.expect("loop at 30,30");
    // LOC_ADD_CHANGE 9 -> 10 at 30,30.
    let mut request = LocRequest {
        level: 0,
        layer: 2,
        x: 30,
        z: 30,
        old_id: 9,
        old_angle: 0,
        id: 10,
        angle: 0,
        remove: false,
    };
    sounds.sync_locs(&mut api, &[request], [104, 104], true, &|_, _| None);
    assert_eq!(api.sound_status(first), SoundState::Stopped);
    assert_eq!(sounds.locs.len(), 2);
    assert!(sounds
        .locs
        .iter()
        .any(|e| e.owner == Owner::Loc(10) && e.min_x == 30 << 9));
    // Unchanged state: no churn.
    sounds.sync_locs(&mut api, &[request], [104, 104], true, &|_, _| None);
    assert_eq!(sounds.locs.len(), 2);
    // LOC_DEL of a changed loc restores the snapshot (remove branch).
    request.remove = true;
    sounds.sync_locs(&mut api, &[request], [104, 104], true, &|_, _| None);
    assert!(sounds.locs.iter().all(|e| e.owner == Owner::Loc(9)));
    assert_eq!(sounds.locs.len(), 2);
    // Out of replaceLoc's 1..size-2 window: ignored.
    let edge = LocRequest {
        x: 103,
        old_id: -1,
        ..request
    };
    sounds.sync_locs(&mut api, &[edge], [104, 104], true, &|_, _| None);
    assert_eq!(sounds.locs.len(), 2);
    // A rebuild drops every loc sound and reloads.
    let live: Vec<usize> = sounds.locs.iter().filter_map(|e| e.loop_sound).collect();
    sounds.rebuild(&mut api, &LocSoundScene::default(), &|_, _| None);
    assert!(sounds.locs.is_empty());
    for key in live {
        assert_eq!(api.sound_status(key), SoundState::Stopped);
    }
}

fn npc(index: usize, type_id: i32, motion: i32) -> NpcSample {
    NpcSample {
        index,
        type_id,
        has_background_sound: true,
        multinpc: false,
        resolved: Some(NpcSound {
            sounds: [50, 51, 52, 53],
            range: 6,
            dropoffrange: 1,
            volume: 200,
            minrate: 256,
            maxrate: 256,
        }),
        level: 0,
        tile: [20, 20],
        trans: [20.0 * 512.0 + 256.0, 20.0 * 512.0 + 256.0],
        size: 1,
        motion,
    }
}

#[test]
fn npc_sounds_follow_the_npc_and_switch_on_walk_state() {
    let mut api = api();
    let mut sounds = PositionedSounds::new(fixed(vec![0.5]));
    let mut n = npc(3, 77, 0);
    sounds.sync_npcs(&mut api, &[n]);
    assert_eq!(sounds.npcs.len(), 1);
    assert_eq!(sounds.npcs[0].sound, 50);
    tick(&mut api, n.trans[0], n.trans[1]);
    sounds.update(&mut api, &frame(&[n], &[]));
    let e = &sounds.npcs[0];
    // minX = trans.x, maxX = trans.x + (size << 8) (:431-434).
    assert_eq!((e.min_x, e.max_x), (10496, 10752));
    let idle = e.loop_sound.expect("idle loop");
    // Starts running: the old loop's volume drops by 512 (<= 0), so it
    // fades out and the run sound replaces it (:411-427).
    n.motion = 2;
    n.trans[0] += 128.0;
    sounds.update(&mut api, &frame(&[n], &[]));
    let e = &sounds.npcs[0];
    assert_eq!((e.sound, e.motion, e.volume), (53, 2, 200));
    assert_eq!(api.sound_status(idle), SoundState::Stopped);
    assert!(e.loop_sound.is_some(), "run loop created the same frame");
    assert_eq!(e.min_x, 10624);
    // Type change: the old entry is dropped, then a fresh one added.
    let changed = NpcSample { type_id: 78, ..n };
    sounds.sync_npcs(&mut api, &[changed]);
    assert_eq!(sounds.npcs.len(), 1);
    assert_eq!(sounds.npcs[0].loop_sound, None);
    // Removal.
    sounds.sync_npcs(&mut api, &[]);
    assert!(sounds.npcs.is_empty());
}

#[test]
fn player_sounds_track_appearance_and_logout_resets_everything() {
    let mut api = api();
    let mut sounds = PositionedSounds::new(fixed(vec![0.5]));
    let mut p = PlayerSample {
        index: 17,
        range: 3,
        sounds: [60, 61, 62, 63],
        volume: 90,
        level: 0,
        tile: [5, 5],
        trans: [2816.0, 2816.0],
        size: 1,
        motion: 1,
    };
    let q = PlayerSample { index: 2, ..p };
    sounds.sync_players(&mut api, &[p, q]);
    // HashTable(16) order: bucket 1 (17) before bucket 2.
    assert_eq!(
        sounds.players.iter().map(|e| e.owner).collect::<Vec<_>>(),
        [Owner::Player(17), Owner::Player(2)]
    );
    assert_eq!(
        (sounds.players[0].sound, sounds.players[0].range),
        (62, 3 << 9)
    );
    // New appearance volume: the existing entry is refreshed.
    p.volume = 40;
    sounds.sync_players(&mut api, &[p, q]);
    assert_eq!(sounds.players[0].volume, 40);
    // bgsound_range 0 removes it.
    p.range = 0;
    sounds.sync_players(&mut api, &[p, q]);
    assert_eq!(sounds.players.len(), 1);
    sounds.table = table(vec![(9, fountain())]);
    sounds.add_loc(&mut api, place(0, 1, 1, 0, 9), &|_, _| None);
    sounds.sync_npcs(&mut api, &[npc(1, 5, 0)]);
    // Logout resets everything.
    sounds.reset(&mut api, true);
    assert!(sounds.locs.is_empty() && sounds.npcs.is_empty() && sounds.players.is_empty());
}
