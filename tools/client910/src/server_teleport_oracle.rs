//! Stateful actual-server packets through production rebuild/player decoders.
use crate::{
    cache::Pack,
    protocol910::{
        self, live,
        rebuild_state::{self, Config, Kind, World},
    },
};
use std::io::{Read, Write};
fn int(r: &mut impl Read) -> std::io::Result<i32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn teleports() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let pack = Pack::open(root.join("server/data/pack"));
    let inputs = crate::entity_runtime::Inputs::load(&pack, true, false, 50)?;
    let mut input = std::io::Cursor::new(rs910_core::test_support::frozen::bytes(
        "server-teleports/input.bin",
    ));
    let mut result = Vec::new();
    for _ in 0..int(&mut input)? {
        let pid = int(&mut input)? as usize;
        let count = int(&mut input)?;
        let mut s = live::State::default();
        let mut w = World {
            base_x: 0,
            base_z: 0,
            region_x: -1,
            region_z: -1,
            width: 104,
            height: 104,
            area: Some(0),
            last_kind: Kind::Normal,
            npc_bits: 0,
            map_squares: vec![],
            groups: vec![],
            group_count: 0,
        };
        for frame in 0..count {
            let op = int(&mut input)?;
            let login = int(&mut input)? != 0;
            let level = int(&mut input)?;
            let x = int(&mut input)?;
            let z = int(&mut input)?;
            let n = int(&mut input)?;
            let mut bytes = vec![0; n as usize];
            input.read_exact(&mut bytes)?;
            let c = protocol910::Context {
                base_x: w.base_x,
                base_z: w.base_z,
                width: 104,
                height: 104,
                local: pid,
                bridges: vec![],
            };
            let (consumed, bits) = if op == 88 {
                let conf = Config {
                    prior: &w,
                    old_map: &c,
                    appearance: Some(&inputs.appearance.appearance),
                    login,
                    land_groups: &Default::default(),
                    loc_sizes: &Default::default(),
                    scene: None,
                };
                let d = rebuild_state::decode_normal(&bytes, &s, &conf)
                    .map_err(|e| anyhow::anyhow!("rebuild {frame}: {e:?}"))?;
                s = d.state;
                w = d.world;
                (d.bytes as i32, d.initial_bits.map_or(-1, |v| v as i32))
            } else {
                let d = protocol910::decode_with_appearance(
                    &bytes,
                    &s.players,
                    &c,
                    Some(&inputs.appearance.appearance),
                )
                .map_err(|e| anyhow::anyhow!("player {frame}: {e:?}"))?;
                s.players = d.state;
                (d.bytes as i32, d.bit_pos as i32)
            };
            let p = s.players.players[pid].as_ref().unwrap();
            if op == 122 || login {
                assert_eq!(
                    (w.base_x + p.x[0], w.base_z + p.z[0], p.level),
                    (x, z, level),
                    "packet {frame}"
                );
                assert_eq!(p.route_length, 0, "teleport must not append walking");
            }
            let mut row = vec![
                consumed,
                bits,
                w.base_x,
                w.base_z,
                w.region_x,
                w.region_z,
                s.players.current_level,
                p.level,
                p.occlude_level,
                p.fine_x.to_bits() as i32,
                p.fine_z.to_bits() as i32,
                p.route_length as i32,
                p.steps_remaining,
                p.seq_trigger,
                p.actor.scene.priority,
                p.appearance.bas,
            ];
            for i in 0..10 {
                row.extend([p.x[i], p.z[i], p.speeds[i] as i32]);
            }
            for values in [
                &s.players.high_indices,
                &s.players.low_indices,
                &s.players.update_ids,
            ] {
                row.push(values.len() as i32);
                row.extend(values.iter().map(|v| *v as i32));
            }
            row.extend(s.players.nsn.iter().map(|v| *v as i32));
            row.extend(s.players.speeds.iter().map(|v| *v as i32));
            row.push(p.appearance.model.is_some() as i32);
            let hash = p.appearance.model.as_ref().map_or(0, |m| m.hash);
            row.extend([(hash >> 32) as i32, hash as i32]);
            result.write_all(&(row.len() as i32).to_be_bytes())?;
            for v in row {
                result.write_all(&v.to_be_bytes())?;
            }
        }
    }
    let mut tail = [0];
    assert_eq!(input.read(&mut tail)?, 0);
    rs910_core::test_support::frozen::assert_stream("server-teleports/recording", &result);
    Ok(())
}
