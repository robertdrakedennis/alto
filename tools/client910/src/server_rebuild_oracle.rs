//! The actual TypeScript server encoder -> recorded original-client and production readers.
use crate::protocol910::{
    self, live,
    rebuild_state::{self, Config, Kind, World},
};
use std::io::{Read, Write};
fn int(r: &mut impl Read) -> std::io::Result<i32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}
#[test]
fn server_rebuild() -> anyhow::Result<()> {
    let mut input = std::io::Cursor::new(rs910_core::test_support::frozen::bytes(
        "server-rebuild/input.bin",
    ));
    let mut result = Vec::new();
    let count = int(&mut input)?;
    for _ in 0..count {
        let x = int(&mut input)?;
        let z = int(&mut input)?;
        let level = int(&mut input)?;
        let pid = int(&mut input)?;
        let force = int(&mut input)? != 0;
        let login = int(&mut input)? != 0;
        let n = int(&mut input)?;
        let mut bytes = vec![0; n as usize];
        input.read_exact(&mut bytes)?;
        let prior = live::State {
            initialized: !login,
            ..Default::default()
        };
        let map = protocol910::Context {
            base_x: 0,
            base_z: 0,
            width: 104,
            height: 104,
            local: pid as usize,
            bridges: vec![],
        };
        let world = World {
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
        // The packet-order test deliberately uses an empty map index; map/cache
        // selection and terrain installation have their own real-cache recorded replay.
        let c = Config {
            prior: &world,
            old_map: &map,
            appearance: None,
            login,
            land_groups: &Default::default(),
            loc_sizes: &Default::default(),
            scene: None,
        };
        let got = rebuild_state::decode_normal(&bytes, &prior, &c)
            .map_err(|e| anyhow::anyhow!("server rebuild: {e:?}"))?;
        let legacy = crate::session::parse_rebuild_normal(&bytes)?;
        assert_eq!(
            (legacy.zone_x as i32, legacy.zone_z as i32),
            (x >> 3, z >> 3)
        );
        assert_eq!(legacy.force, force);
        assert_eq!(legacy.has_high_res_block, login);
        assert_eq!((got.world.region_x, got.world.region_z), (x >> 3, z >> 3));
        let player = got.state.players.players[pid as usize].as_ref();
        let row = [
            got.bytes as i32,
            got.initial_bits.map_or(-1, |v| v as i32),
            got.world.region_x,
            got.world.region_z,
            got.world.base_x,
            got.world.base_z,
            got.world.npc_bits as i32,
            got.world.width,
            got.world.height,
            got.world.map_squares.len() as i32,
            got.world.group_count as i32,
            player.map_or(-1, |p| p.level),
            player.map_or(0, |p| p.fine_x.to_bits() as i32),
            player.map_or(0, |p| p.fine_z.to_bits() as i32),
        ];
        if login {
            assert_eq!(player.unwrap().level, level);
        }
        for v in row {
            result.write_all(&v.to_be_bytes())?;
        }
    }
    let mut tail = [0];
    assert_eq!(input.read(&mut tail)?, 0);
    rs910_core::test_support::frozen::assert_stream("server-rebuild/recording", &result);
    Ok(())
}
