//! Entity packet-apply cost over a recorded live stream (quality item Q0.3).
//!
//! Replays a recorded `Game::apply_next` input stream (lines
//! `login <pid> <seed> <members>` and
//! `packet <cycle> <now_ms> <textures> <opcode> <hex|->`) through the real
//! production adapter and reports, per opcode, wall time and heap bytes
//! allocated by each apply, plus a digest chain over the decoded entity state,
//! applied receipts and the committed NPC random stream. Two builds that print
//! the same digest decoded the stream identically.
//!
//! `entity_apply_rejects_atomically` replays the same stream but first feeds
//! every truncation (and two over-long variants) of each PLAYER_INFO, NPC_INFO
//! and varp packet: a rejected packet must leave the entity state and the NPC
//! random stream untouched, and the error sequence is digested for comparison.
//!
//! Input: the checked-in `fixtures/replays/entity-stream` corpus
//! (`cd server && npm run export-fixtures`), or a text recording named by
//! `CLIENT910_ENTITY_BENCH`. Benchmark: `cargo test --release --lib
//! entity_apply_bench -- --ignored --nocapture`.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Counts heap bytes/allocations per thread so concurrently running tests do
/// not pollute a measurement. Forwards everything to the system allocator.
struct CountingAlloc;
thread_local! {
    static BYTES: Cell<u64> = const { Cell::new(0) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}
fn note(bytes: usize) {
    let _ = BYTES.try_with(|b| b.set(b.get() + bytes as u64));
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}
/// This thread's allocated bytes and allocations so far (also read by
/// `app::session_replay::client_core_replay_bench`).
pub(crate) fn counters() -> (u64, u64) {
    (
        BYTES.try_with(Cell::get).unwrap_or(0),
        ALLOCS.try_with(Cell::get).unwrap_or(0),
    )
}
// SAFETY: every call forwards unchanged to `System`; counting touches only a
// const-initialised, destructor-free thread local.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        System.realloc(ptr, layout, new_size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

struct Recording {
    local: usize,
    seed: u64,
    members: bool,
    /// `(cycle, now_ms, opcode, payload)` in apply order.
    packets: Vec<(i32, i64, u8, Vec<u8>)>,
}
/// `CLIENT910_ENTITY_BENCH` (a text recording, format above) when set, else
/// the checked-in dev-server stream `fixtures/replays/entity-stream`.
fn recording() -> anyhow::Result<Recording> {
    let Some(path) = std::env::var_os("CLIENT910_ENTITY_BENCH") else {
        return fixture_recording();
    };
    let text = std::fs::read_to_string(path)?;
    let mut r = Recording {
        local: 0,
        seed: 0,
        members: false,
        packets: vec![],
    };
    for l in text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
    {
        match l[0] {
            "login" => (r.local, r.seed, r.members) = (l[1].parse()?, l[2].parse()?, l[3] == "1"),
            "packet" => r.packets.push((
                l[1].parse()?,
                l[2].parse()?,
                l[4].parse()?,
                if l[5] == "-" {
                    vec![]
                } else {
                    (0..l[5].len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&l[5][i..i + 2], 16))
                        .collect::<Result<_, _>>()?
                },
            )),
            _ => anyhow::bail!("bad recording line {l:?}"),
        }
    }
    Ok(r)
}
/// Observer pid 1's complete stream from `npm run export-fixtures`
/// (`entity-stream`: login, a second player, two NPCs, walk/run ticks,
/// VARP_SMALL/VARP_LARGE incl. negative values, VARBIT_SMALL). Server tick
/// `t` is logic cycle `30 t` (600 ms / 20 ms). Only frames the entity feed
/// owns are kept; UI packets go to other owners.
fn fixture_recording() -> anyhow::Result<Recording> {
    let rows = crate::test_support::replay_json("entity-stream", "frames.json");
    let mut packets = vec![];
    for row in rows.as_array().ok_or_else(|| anyhow::anyhow!("rows"))? {
        let tick = row["tick"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("tick"))?;
        for raw in row["frames"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("frames"))?
        {
            let wire: Vec<u8> = raw
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("frame"))?
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let (frame, used) = crate::net::decode_frame(&wire)?
                .ok_or_else(|| anyhow::anyhow!("incomplete frame"))?;
            anyhow::ensure!(used == wire.len(), "trailing frame bytes");
            if crate::protocol910::live::Feed::default().enqueue(frame.opcode, &[]) {
                packets.push((
                    tick as i32 * 30,
                    10_000 + tick * 600,
                    frame.opcode,
                    frame.payload,
                ));
            }
        }
    }
    Ok(Recording {
        local: 1,
        seed: 910,
        members: true,
        packets,
    })
}
fn pack() -> crate::cache::Pack {
    crate::test_support::require_pack("client.config.js5")
}
/// Advances the logic cycles between two recorded packets the way the live
/// loop does (var polling, then actor logic).
fn advance(game: &mut crate::client_game::ClientGame, cycle: i32, now: i64) -> anyhow::Result<()> {
    while game.cycle < cycle {
        game.cycle += 1;
        game.poll_vars(|| now)
            .map_err(|e| anyhow::anyhow!("poll: {e:?}"))?;
        game.update_actors()
            .map_err(|e| anyhow::anyhow!("actors: {e:?}"))?;
    }
    Ok(())
}
fn install_pending_map(
    game: &mut crate::client_game::ClientGame,
    pack: &crate::cache::Pack,
) -> anyhow::Result<()> {
    if game.runtime.map_request.is_some() {
        let map = game.runtime.prepare_map(pack)?;
        game.runtime
            .install_map(map)
            .map_err(|e| anyhow::anyhow!("install: {e:?}"))?;
    }
    Ok(())
}

#[derive(Default)]
struct Stat {
    micros: Vec<f64>,
    bytes: Vec<u64>,
    allocs: Vec<u64>,
}

#[test]
#[ignore = "diagnostic: per-opcode apply time/heap table; run with --release --nocapture (CLIENT910_ENTITY_BENCH overrides the checked-in stream)"]
fn entity_apply_bench() -> anyhow::Result<()> {
    use std::hash::{Hash, Hasher};
    let rec = recording()?;
    let pack = pack();
    let rounds: usize = std::env::var("CLIENT910_ENTITY_BENCH_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let mut stats: std::collections::BTreeMap<u8, Stat> = Default::default();
    let mut digest = None;
    for _ in 0..rounds {
        let mut game = crate::client_game::ClientGame::login(
            &pack,
            rec.local,
            crate::protocol910::live::Feed::default(),
            rec.seed,
            rec.members,
        )?;
        let mut chain = std::collections::hash_map::DefaultHasher::new();
        for (cycle, now, opcode, payload) in &rec.packets {
            let (now, opcode) = (*now, *opcode);
            advance(&mut game, *cycle, now)?;
            anyhow::ensure!(game.runtime.feed.enqueue(opcode, payload));
            let (b0, a0) = counters();
            let start = std::time::Instant::now();
            let applied = game.apply_next(now);
            let elapsed = start.elapsed().as_secs_f64() * 1e6;
            let (b1, a1) = counters();
            let applied = applied.map_err(|e| anyhow::anyhow!("apply {opcode}: {e:?}"))?;
            let s = stats.entry(opcode).or_default();
            s.micros.push(elapsed);
            s.bytes.push(b1 - b0);
            s.allocs.push(a1 - a0);
            format!("{applied:?}").hash(&mut chain);
            format!("{:?}", game.runtime.feed.state).hash(&mut chain);
            game.random.state.hash(&mut chain);
            install_pending_map(&mut game, &pack)?;
        }
        let d = chain.finish();
        assert!(digest.is_none_or(|p| p == d), "replay is not deterministic");
        digest = Some(d);
    }
    println!(
        "entity_apply_bench digest {:016x} rounds {rounds}",
        digest.unwrap()
    );
    println!("opcode  n     mean_us  p50_us   mean_bytes  mean_allocs");
    for (op, s) in &stats {
        let mut m = s.micros.clone();
        m.sort_by(f64::total_cmp);
        let n = m.len() as f64;
        println!(
            "{:<7} {:<5} {:<8.1} {:<8.1} {:<11.0} {:.1}",
            crate::proto::server::name(*op),
            s.micros.len() / rounds,
            m.iter().sum::<f64>() / n,
            m[m.len() / 2],
            s.bytes.iter().sum::<u64>() as f64 / n,
            s.allocs.iter().sum::<u64>() as f64 / n,
        );
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn entity_apply_rejects_atomically() -> anyhow::Result<()> {
    use crate::proto::server::{NPC_INFO, PLAYER_INFO, VARBIT_SMALL, VARP_LARGE, VARP_SMALL};
    use std::hash::{Hash, Hasher};
    let rec = recording()?;
    let pack = pack();
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        rec.local,
        crate::protocol910::live::Feed::default(),
        rec.seed,
        rec.members,
    )?;
    let mut outcomes = std::collections::hash_map::DefaultHasher::new();
    let (mut rejected, mut accepted) = (0, 0);
    let mut exercised = std::collections::BTreeSet::new();
    for (cycle, now, opcode, payload) in &rec.packets {
        advance(&mut game, *cycle, *now)?;
        if [PLAYER_INFO, NPC_INFO, VARP_SMALL, VARP_LARGE, VARBIT_SMALL].contains(opcode) {
            exercised.insert(*opcode);
            let mut variants: Vec<Vec<u8>> =
                (0..payload.len()).map(|n| payload[..n].to_vec()).collect();
            for extra in [0u8, 0xff] {
                let mut long = payload.clone();
                long.push(extra);
                variants.push(long);
            }
            for variant in variants {
                let feed = game.runtime.feed.clone();
                let random = game.random.clone();
                let (applied, transmit) = (game.packets_applied, game.runtime.varp_transmit_num);
                assert!(game.runtime.feed.enqueue(*opcode, &variant));
                match game.apply_next(*now) {
                    Err(error) => {
                        rejected += 1;
                        format!("{opcode} {} {error:?}", variant.len()).hash(&mut outcomes);
                        assert_eq!(
                            game.runtime.feed.state, feed.state,
                            "{opcode} {variant:02x?}"
                        );
                        assert_eq!(
                            game.runtime.feed.front().map(|f| &f.payload),
                            Some(&variant)
                        );
                        assert_eq!(game.runtime.feed.blocked.as_ref(), Some(&error));
                        assert_eq!(
                            game.random.state, random.state,
                            "rejected packet drew randoms"
                        );
                        assert_eq!(game.runtime.varp_transmit_num, transmit);
                    }
                    Ok(a) => {
                        accepted += 1;
                        format!(
                            "{opcode} {} {a:?} {:?}",
                            variant.len(),
                            game.runtime.feed.state
                        )
                        .hash(&mut outcomes);
                    }
                }
                game.runtime.feed = feed;
                game.random = random;
                (game.packets_applied, game.runtime.varp_transmit_num) = (applied, transmit);
            }
        }
        assert!(game.runtime.feed.enqueue(*opcode, payload));
        game.apply_next(*now)
            .map_err(|e| anyhow::anyhow!("apply {opcode}: {e:?}"))?;
        install_pending_map(&mut game, &pack)?;
    }
    // The stream must actually carry every packet family under test, and
    // truncations must be rejected (not silently accepted).
    assert_eq!(exercised.into_iter().collect::<Vec<_>>(), {
        let mut all = vec![PLAYER_INFO, NPC_INFO, VARP_SMALL, VARP_LARGE, VARBIT_SMALL];
        all.sort_unstable();
        all
    });
    assert!(
        rejected > 0 && accepted > 0,
        "rejected {rejected} accepted {accepted}"
    );
    println!(
        "entity_apply_rejects_atomically rejected {rejected} accepted {accepted} outcome digest {:016x} final state digest {:016x}",
        outcomes.finish(),
        {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{:?} {}", game.runtime.feed.state, game.random.state).hash(&mut h);
            h.finish()
        }
    );
    Ok(())
}
