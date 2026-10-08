use super::*;
use crate::{
    animation_assets::AnimationAssets,
    billboard::BillboardStore,
    cache::{self, Pack},
    config::NpcStore,
    npc_draw::{Cache, Fade, Look, PickOptions, Sources, Spec},
    particle::EmitterStore,
    protocol910::pack_types,
    texture::MaterialStore,
    title_world::{NpcInputs, Rebuild, SquareFiles, TitleWorld},
};
use rs910_core::animation_random::AnimationRandom;

fn entry(level: u16, x: u16, z: u16, type_id: u16) -> [u8; 4] {
    let position = (level << 14 | x << 7 | z).to_be_bytes();
    let kind = type_id.to_be_bytes();
    [position[0], position[1], kind[0], kind[1]]
}

/// The entry layout, the 511-entry cap and a partial entry.
#[test]
fn entries_decode_from_four_bytes_each() {
    let mut bytes = Vec::new();
    bytes.extend(entry(2, 5, 63, 0x1234));
    bytes.extend(entry(0, 0, 0, 7));
    assert_eq!(
        decode(&bytes).unwrap(),
        [
            MapNpc {
                level: 2,
                x: 5,
                z: 63,
                type_id: 0x1234
            },
            MapNpc {
                level: 0,
                x: 0,
                z: 0,
                type_id: 7
            },
        ]
    );
    assert!(decode(&[]).unwrap().is_empty());
    assert!(matches!(
        decode(&bytes[..7]),
        Err(MapError::Truncated { .. })
    ));
    let long: Vec<u8> = (0..600).flat_map(|n| entry(0, 1, 1, n)).collect();
    let list = decode(&long).unwrap();
    assert_eq!(list.len(), MAX_ENTRIES);
    assert_eq!(list[510].type_id, 510);
}

/// FNV-1a over the values of a square's entries.
fn digest(entries: &[MapNpc]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for e in entries {
        for byte in [e.level, e.x, e.z]
            .into_iter()
            .chain(e.type_id.to_be_bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// The squares of the cache whose group lists an NPC file, with their decoded entries.
fn squares_with_lists(pack: &Pack) -> Vec<(u32, Vec<MapNpc>)> {
    let index = pack.read_archive_index(crate::map::MAP_ARCHIVE).unwrap();
    let mut out = Vec::new();
    for &group in &index.group_id {
        let count = index.file_count_for_group(group).unwrap();
        let listed =
            (0..count).any(|i| index.file_id_for_group_index(group, i).unwrap() == cache::NPC_FILE);
        if listed {
            let files = pack.read_group(crate::map::MAP_ARCHIVE, group).unwrap();
            out.push((group, decode(&files[&cache::NPC_FILE]).unwrap()));
        }
    }
    out
}

fn fixture_path() -> std::path::PathBuf {
    rs910_core::test_support::client_dir().join("crates/rs910-scene/fixtures/map-npcs.tsv")
}

/// Every NPC list of the cache decodes; the digest holds each square's group, entry count and
/// a hash of the entries' values. Regenerate it for an intended change with
/// `RS910_UPDATE_CORPUS=1 cargo test -p rs910-scene map_npcs`.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn every_list_of_the_cache_decodes_to_the_committed_digest() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let types = pack_types::load(&pack, true).unwrap().npc_types();
    let squares = squares_with_lists(&pack);
    let mut rows = String::from("# group\tentries\tdigest of the entries' values\n");
    for (group, list) in &squares {
        rows.push_str(&format!("{group}\t{}\t{:016x}\n", list.len(), digest(list)));
        for e in list {
            let kind = types.get(&e.type_id).expect("a listed NPC type exists");
            assert!(
                kind.respawndir.is_some(),
                "type {} faces a direction",
                e.type_id
            );
            assert!(e.x < 64 && e.z < 64 && e.level < 4);
        }
    }
    if std::env::var_os("RS910_UPDATE_CORPUS").is_some() {
        std::fs::write(fixture_path(), &rows).unwrap();
    }
    assert_eq!(rows, std::fs::read_to_string(fixture_path()).unwrap());
    // Twelve squares list an empty file (they still take a slot); thirty-two hold entries.
    assert_eq!(squares.len(), 44);
    assert_eq!(squares.iter().filter(|(_, l)| !l.is_empty()).count(), 32);
}

struct Stage {
    pack: Pack,
    types: std::collections::BTreeMap<i32, NpcType>,
    land: std::collections::BTreeSet<i32>,
}

fn stage() -> Stage {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let index = pack.read_archive_index(crate::map::MAP_ARCHIVE).unwrap();
    let land = index
        .group_id
        .iter()
        .copied()
        .filter(|&g| {
            let count = index.file_count_for_group(g).unwrap();
            (0..count).any(|i| index.file_id_for_group_index(g, i).unwrap() == cache::LAND_FILE)
        })
        .map(|g| g as i32)
        .collect();
    Stage {
        types: pack_types::load(&pack, true).unwrap().npc_types(),
        pack,
        land,
    }
}

impl Stage {
    /// The title rebuild around zone `(region_x, region_z)` and its squares' files.
    fn rebuild(&self, world: &mut TitleWorld, region: [i32; 2]) -> (Rebuild, Vec<SquareFiles>) {
        // The camera is local to the scene base.
        let camera = [
            (region[0] - (world.base[0] >> 3)) << 12,
            (region[1] - (world.base[1] >> 3)) << 12,
        ];
        let rebuild = world
            .rebuild(camera, 0, 4, &mut |g| self.land.contains(&g))
            .unwrap()
            .expect("the region changed");
        let files = crate::title_world::read_squares(&self.pack, &rebuild.squares);
        (rebuild, files)
    }

    fn enter(
        &self,
        world: &mut TitleWorld,
        npcs: &mut Npcs,
        random: &mut AnimationRandom,
        region: [i32; 2],
    ) -> usize {
        let (rebuild, files) = self.rebuild(world, region);
        world
            .update_npcs(
                &rebuild,
                &files,
                npcs,
                NpcInputs {
                    types: &self.types,
                    cycle: 77,
                    textures: true,
                    random: &mut || random.next(),
                },
            )
            .unwrap()
            .placed
    }
}

/// Zone (324, 412) is the middle of squares (39..=41, 51): three lists of 82, 113 and 55 entries.
const TOWN: [i32; 2] = [324, 412];

/// A title rebuild stands up the lists of its squares: each NPC is the entry its index names,
/// stands on that entry's tile, faces the opposite of its stored direction and carries its
/// type's size and turn rate; the NPC list's slot and snapshot lists agree.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_title_rebuild_stands_up_the_lists_of_its_squares() {
    let stage = stage();
    let lists: std::collections::BTreeMap<i32, Vec<MapNpc>> = squares_with_lists(&stage.pack)
        .into_iter()
        .map(|(g, l)| (g as i32, l))
        .collect();
    let mut world = TitleWorld::default();
    let mut npcs = Npcs::default();
    let mut random = AnimationRandom::new(1);
    let placed = stage.enter(&mut world, &mut npcs, &mut random, TOWN);
    assert!(placed > 100, "{placed} NPCs");
    assert_eq!(placed, npcs.entities.len());
    assert_eq!(npcs.slots, npcs.snapshot);
    assert_eq!(npcs.slots.len(), placed);
    let base = world.base;
    for (&index, npc) in &npcs.entities {
        // The slot is the square's place in first-use order; the entry number is above it.
        let (slot, number) = (index & 63, index >> 6);
        let group = [39, 40, 41]
            .iter()
            .map(|&mx| mx | 51 << 7)
            .nth(slot)
            .expect("three squares, three slots");
        let entry = lists[&group][number];
        let kind = &stage.types[&entry.type_id];
        let tile = [
            (group & 0x7F) * 64 + i32::from(entry.x) - base[0],
            (group >> 7) * 64 + i32::from(entry.z) - base[1],
        ];
        assert_eq!([npc.path.x[0], npc.path.z[0]], tile);
        assert_eq!(npc.type_id, entry.type_id);
        assert_eq!(npc.path.level, i32::from(entry.level));
        assert_eq!(npc.path.size, kind.size);
        assert_eq!(npc.turn_speed, kind.turnspeed << 3);
        assert_eq!(npc.update_serial, 77);
        assert_eq!(npc.fade_alpha, 0, "a map NPC does not fade in");
        let facing = ((kind.respawndir.unwrap() + 4) << 11) & 0x3FFF;
        assert_eq!((npc.path.angle, npc.path.desired_angle), (facing, facing));
        let half = kind.size * 256;
        assert_eq!(
            [npc.path.fine_x as i32, npc.path.fine_z as i32],
            [tile[0] * 512 + half, tile[1] * 512 + half]
        );
        assert!(tile[0] >= 0 && tile[0] + kind.size < world.map_size);
        assert!(tile[1] >= 0 && tile[1] + kind.size < world.map_size);
    }
}

/// The placed NPCs draw: their cache models build and stand on their tiles.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn placed_npcs_draw() {
    let stage = stage();
    let mut world = TitleWorld::default();
    let mut npcs = Npcs::default();
    let mut random = AnimationRandom::new(2);
    stage.enter(&mut world, &mut npcs, &mut random, TOWN);
    let store = NpcStore::load(&stage.pack).unwrap();
    let materials = MaterialStore::load(&stage.pack).unwrap();
    let billboards = BillboardStore::load(&stage.pack).unwrap();
    let emitters = EmitterStore::load(&stage.pack).unwrap();
    let mut assets = AnimationAssets::load(&stage.pack).unwrap();
    let sources = Sources {
        pack: &stage.pack,
        materials: &materials,
        billboards: &billboards,
        emitters: &emitters,
        bases: None,
        detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
    };
    let mut cache = Cache::default();
    let mut drawn = 0;
    for npc in npcs.entities.values().take(40) {
        let config = store.get(npc.type_id as u32).unwrap();
        let look = Look {
            antimacro: None,
            shadow: None,
            overlay_height: -1,
            pick: PickOptions::of(config),
        };
        let spec = Spec {
            config,
            custom: None,
            bas: -1,
            bas_type: None,
            path: &npc.path,
            node: Default::default(),
            walk: None,
            overlays: vec![],
            wear_angles: None,
            angle: npc.path.angle,
            look: &look,
            shadow_node: None,
            fade: Fade::default(),
            fade_in: config.fade_in,
            decoration_offset: 0,
            terrain: None,
            cycle: 100,
        };
        if let Some(body) = cache.draw(&mut assets, &sources, spec).unwrap() {
            assert!(body.body.face_count > 0);
            drawn += 1;
        }
    }
    assert!(drawn >= 20, "{drawn} of 40 drew");
}

/// Squares keep their slots and indices for the whole run: the camera leaves the town (the
/// NPCs outside the new area go, the rest keep their tiles under the new base) and comes back
/// to exactly the NPCs it first stood up.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_square_keeps_its_indices_when_the_camera_returns() {
    let stage = stage();
    let mut world = TitleWorld::default();
    let mut npcs = Npcs::default();
    let mut random = AnimationRandom::new(3);
    stage.enter(&mut world, &mut npcs, &mut random, TOWN);
    let first: std::collections::BTreeMap<usize, [i32; 2]> = npcs
        .entities
        .iter()
        .map(|(&i, n)| {
            (
                i,
                [n.path.x[0] + world.base[0], n.path.z[0] + world.base[1]],
            )
        })
        .collect();
    // One square east: the areas overlap by forty tiles.
    let away = [TOWN[0] + 8, TOWN[1]];
    stage.enter(&mut world, &mut npcs, &mut random, away);
    let kept: Vec<_> = first
        .keys()
        .filter(|i| npcs.entities.contains_key(i))
        .collect();
    assert!(
        !kept.is_empty() && kept.len() < first.len(),
        "{} kept",
        kept.len()
    );
    for &index in &kept {
        let npc = &npcs.entities[index];
        assert_eq!(
            [npc.path.x[0] + world.base[0], npc.path.z[0] + world.base[1]],
            first[index],
            "NPC {index} keeps its absolute tile"
        );
    }
    assert!(
        npcs.entities.len() > kept.len(),
        "the new squares stand up their lists"
    );
    stage.enter(&mut world, &mut npcs, &mut random, TOWN);
    let again: std::collections::BTreeMap<usize, [i32; 2]> = npcs
        .entities
        .iter()
        .map(|(&i, n)| {
            (
                i,
                [n.path.x[0] + world.base[0], n.path.z[0] + world.base[1]],
            )
        })
        .collect();
    assert_eq!(again, first);
    assert_eq!(npcs.slots.len(), npcs.snapshot.len());
}
