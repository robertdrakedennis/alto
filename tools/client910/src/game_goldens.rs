//! Tests of the rs910-game modules `game_runtime` and `cutscene` that need
//! client910 (the scene build: `map`, `rebuild`, `maploader`; `ClientGame`;
//! `test_support`), so they stay in this package (tools/README.md "Tests").
//! Moved with unchanged bodies from each module's `tests` (Phase 3.2); each
//! submodule globs the module it tests, like the `use super::*` it came from.

mod cutscene;

mod game_runtime {
    use crate::cache::Pack;
    use crate::game_scene::GameScene;
    use crate::protocol910::live::Feed;

    /// Joins two independently recorded producers using real map requests,
    /// including every supported build-area size. Input bit fixtures come from the
    /// existing recorded player oracle, not a new expected-state implementation.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_map_installation_matches_cpu_terrain() -> anyhow::Result<()> {
        let root = rs910_core::test_support::repo_root();
        let pack = Pack::open(root.join("server/data/pack"));
        let fixtures =
            std::fs::read_to_string(root.join("tools/client910/fixtures/phase-g/players.txt"))?;
        let hex = fixtures.lines().next().unwrap().split_once(' ').unwrap().1;
        let initial = (0..hex.len())
            .step_by(2)
            .map(|n| u8::from_str_radix(&hex[n..n + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, true)?;
        let flo = crate::flo::FloStore::load(&pack)?;
        let tables = crate::maploader::FloTables::from_store(&flo);
        let materials = crate::texture::MaterialStore::load(&pack)?;
        let locs = crate::config::LocStore::load(&pack)?;
        let areas = "0,1,2,3,4,5";
        let mut bytes_compared = 0;
        for (step, area) in areas.split(',').enumerate() {
            let area: u8 = area.parse()?;
            let mut packet = if step == 0 { initial.clone() } else { vec![] };
            // Normal-map rebuild wire fields; count deliberately has
            // unused trailing slots, which the client still processes in the LAND pass.
            {
                let v = 402u16;
                packet.extend([(v >> 8) as u8, (v as u8).wrapping_add(128)]);
            }
            packet.extend([5, 128 - 81, area]);
            packet.extend([1, (402u16 as u8).wrapping_add(128), 127]);
            assert!(game
                .runtime
                .feed
                .enqueue(crate::proto::server::REBUILD_NORMAL, &packet));
            let applied = game
                .apply_next(1000 + step as i64)
                .map_err(|e| anyhow::anyhow!("rebuild packet: {e:?}"))?
                .unwrap();
            assert!(applied.rebuild.as_ref().unwrap().1.rebased);
            let request = game.runtime.map_request.as_ref().unwrap().world.clone();
            let prepared = game.runtime.prepare_map(&pack)?;
            let rebuilt = match crate::rebuild::rebuild_world(
                &pack,
                &tables,
                &materials,
                &locs,
                &request,
                &crate::rebuild::BuildPrefs::default(),
            ) {
                Ok(built) => built,
                Err(error) if area == 5 => {
                    // The recorded run of this same 256-tile window fails after a
                    // wrapped index aliases a null owner.
                    anyhow::ensure!(
                        error
                            .to_string()
                            .starts_with("floor batch owner missing at reused index "),
                        "unexpected large-map failure: {error:#}"
                    );
                    assert!(game.runtime.map_request.is_some());
                    println!("PASS negative map area 5: {error}; map remains unacknowledged");
                    break;
                }
                Err(error) => return Err(error),
            };
            game.verify_scene(&prepared, &rebuilt)?;
            bytes_compared +=
                prepared.data.terrain.heights.len() * 4 + prepared.data.terrain.tiles.len();
            game.capture_scene(rebuilt.scene_graph.as_ref().unwrap());
            // The map must still be blocked until the owning adapter installs it.
            assert!(game.runtime.map_request.is_some());
            game.runtime
                .install_map(prepared)
                .map_err(|e| anyhow::anyhow!("install: {e:?}"))?;
            assert!(game.runtime.map_request.is_none());
            assert_eq!(game.runtime.map.width, request.width);
            println!(
                "PASS map area {area}: {}x{}, base {},{}, {} loc snapshots",
                request.width,
                request.height,
                request.base_x,
                request.base_z,
                game.scene_locs.len()
            );
        }
        println!("Map installation: {bytes_compared} terrain/flag bytes compared");
        Ok(())
    }
}
