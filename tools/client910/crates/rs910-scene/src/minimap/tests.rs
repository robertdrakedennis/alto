use super::*;

#[test]
fn base_plan_ends_with_the_chunk_noise_fill() {
    // The noise channel at a random value of 0.5 is
    // 4 per channel, added (blend 2) over the whole tile area.
    let terrain = crate::protocol910::terrain::Terrain::new(8, 8).unwrap();
    let scene = crate::scene::Scene::new(9, 4, 8, 8);
    let plan = Minimap::default().base_plan(&scene, &terrain, 0, None);
    assert_eq!(plan.size, 8 * 4 + 96);
    match plan.marks.last() {
        Some(Mark::Add { rect, colour }) => {
            assert_eq!(*rect, [48, 48, 32, 32]);
            assert_eq!(*colour, 0x040404);
        }
        _ => panic!("the last base mark is the additive noise fill"),
    }
}

#[test]
fn tile_visibility_rules() {
    let mut t = crate::protocol910::terrain::Terrain::new(4, 4).unwrap();
    // A bridge tile (level-0 flag 2) is always visible.
    let i = t.tile(0, 1, 1);
    t.tiles[i].flags = 2;
    assert!(tile_visible(&t, 1, 3, 1, 1));
    // Flag 0x10 hides the level.
    let i = t.tile(2, 2, 2);
    t.tiles[i].flags = 0x10;
    assert!(!tile_visible(&t, 2, 2, 2, 2));
    // Otherwise the effective level must equal the player's.
    assert!(tile_visible(&t, 1, 1, 3, 3));
    assert!(!tile_visible(&t, 1, 2, 3, 3));
    // Flag 8 collapses the level to 0.
    let i = t.tile(2, 3, 3);
    t.tiles[i].flags = 8;
    assert!(tile_visible(&t, 0, 2, 3, 3));
}

#[test]
fn map_element_decode_and_tests() {
    // sprite 5, text, flags (minimap), varbit 7 range 1..3 (opcode 9), scale 50, align X serial 2 -> index 0.
    let bytes = [
        1, 0, 5, 3, b'H', b'i', 0, 7, 2, 9, 0, 7, 255, 255, 0, 0, 0, 1, 0, 0, 0, 3, 28, 50, 29, 2,
        0,
    ];
    let e = MapElement::decode(&bytes).unwrap();
    assert_eq!(
        (
            e.sprite,
            e.text.as_deref(),
            e.show_on_minimap,
            e.varbit,
            e.varp,
            e.range,
            e.scale,
            e.align
        ),
        (5, Some("Hi"), true, 7, -1, [1, 3], 50, [0, 2])
    );
    let read = |bit: bool, id: i32| -> Result<i32> {
        assert!(bit && id == 7);
        Ok(2)
    };
    assert!(e.variable_test(&read).unwrap());
    let read = |_: bool, _: i32| -> Result<i32> { Ok(4) };
    assert!(!e.variable_test(&read).unwrap());
    // multime (opcode 27): varbit 9, default 40, entries [10, 20]; value 1 -> 20, value 5 -> 40.
    let m = MapElement::decode(&[27, 0, 9, 255, 255, 0, 40, 1, 0, 10, 0, 20, 0]).unwrap();
    assert_eq!(m.multime, vec![10, 20, 40]);
    assert_eq!(m.multi(&|_, _| Ok(1)).unwrap(), Some(20));
    assert_eq!(m.multi(&|_, _| Ok(5)).unwrap(), Some(40));
    // Polygon bounds follow postDecode's else-if chain.
    let p = MapElement::decode(&[
        15, 2, 0, 3, 0, 4, 255, 255, 255, 254, 0, 0, 0, 0, 0, 0, 0, 0,
    ])
    .unwrap();
    assert_eq!(p.polygon, vec![3, 4, -1, -2]);
    assert_eq!(p.bounds, [-1, -2, i32::MIN, i32::MIN]);
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn world_map_elements_pack_lookup() {
    let pack = crate::test_support::require_pack("client.worldmap.js5");
    let areas = load_area_metadata(&pack).unwrap();
    assert!(!areas.is_empty());
    // Lumbridge (3222, 3218) lies on the surface map.
    let area = area_for(&areas, 3222, 3218).expect("surface area");
    let elements = WorldMapElements::load(&pack, &area.name, true).unwrap();
    assert!(
        !elements.coords.is_empty(),
        "no elements for {:?}",
        area.name
    );
    assert_eq!(elements.coords.len(), elements.elements.len());
    let store = MapElementStore::load(&pack).unwrap();
    assert!(elements
        .elements
        .iter()
        .any(|e| store.get(*e).is_some_and(|t| t.show_on_minimap)));
}

#[test]
fn overlays_pick_dot_frames_and_offsets() {
    let dot = |i: i32| {
        Rc::new(Sprite {
            paletted: None,
            size: [4, 4],
            padding: [0; 4],
            argb: vec![i; 16],
        })
    };
    let mut m = Minimap {
        cached_level: 1,
        ..Minimap::default()
    };
    m.pack = Some(Pack::open("/nonexistent"));
    m.mapdots = (0..9).map(dot).collect();
    m.mapflag = vec![dot(20), dot(21)];
    m.defaults = Some(crate::avatar::GraphicsDefaults {
        mapflag_offset: [3, 4],
        ..Default::default()
    });
    let players = vec![
        // Same team -> frame 4; hidden -> skipped; no model -> skipped; other level -> skipped; partner 2 -> frame 6; plain -> 2.
        PlayerDot {
            index: 1,
            has_model: true,
            hidden: false,
            level: 1,
            fine: [64 * 128 + 64, 40 * 128],
            transmog_npc: -1,
            partner: 0,
            suppress_partner: false,
            team: 3,
            friend: false,
            clan: false,
        },
        PlayerDot {
            index: 2,
            has_model: true,
            hidden: true,
            level: 1,
            fine: [0, 0],
            transmog_npc: -1,
            partner: 0,
            suppress_partner: false,
            team: 0,
            friend: false,
            clan: false,
        },
        PlayerDot {
            index: 3,
            has_model: false,
            hidden: false,
            level: 1,
            fine: [0, 0],
            transmog_npc: -1,
            partner: 0,
            suppress_partner: false,
            team: 0,
            friend: false,
            clan: false,
        },
        PlayerDot {
            index: 4,
            has_model: true,
            hidden: false,
            level: 0,
            fine: [0, 0],
            transmog_npc: -1,
            partner: 0,
            suppress_partner: false,
            team: 0,
            friend: false,
            clan: false,
        },
        PlayerDot {
            index: 5,
            has_model: true,
            hidden: false,
            level: 1,
            fine: [0, 0],
            transmog_npc: -1,
            partner: 2,
            suppress_partner: false,
            team: 0,
            friend: false,
            clan: false,
        },
        PlayerDot {
            index: 6,
            has_model: true,
            hidden: false,
            level: 1,
            fine: [0, 0],
            transmog_npc: -1,
            partner: 0,
            suppress_partner: true,
            team: 0,
            friend: false,
            clan: false,
        },
        PlayerDot {
            index: 7,
            has_model: true,
            hidden: false,
            level: 1,
            fine: [0, 0],
            transmog_npc: -1,
            partner: 0,
            suppress_partner: false,
            team: 0,
            friend: true,
            clan: false,
        },
        PlayerDot {
            index: 8,
            has_model: true,
            hidden: false,
            level: 1,
            fine: [0, 0],
            transmog_npc: -1,
            partner: 0,
            suppress_partner: false,
            team: 0,
            friend: false,
            clan: true,
        },
    ];
    // Obj stacks: level 1 at absolute (3205, 3210) drawn; level 0 skipped.
    let stacks = [(1_i64 << 28) | (3210 << 14) | 3205, (3210 << 14) | 3205];
    let read = |_: bool, _: i32| -> Result<i32> { Ok(0) };
    let out = m.overlays(&OverlayInput {
        base: [3200, 3200],
        player: [32 * 128, 32 * 128],
        level: 1,
        cycle: 0,
        local_size: 1,
        local_team: 3,
        stacks: &stacks,
        npcs: &[],
        players: &players,
        read: &read,
        flag: MinimapFlag {
            flag: [10, 12],
            ..MinimapFlag::default()
        },
    });
    let frames: Vec<i32> = out.iter().map(|o| o.sprite.argb[0]).collect();
    assert_eq!(frames, vec![0, 4, 6, 7, 3, 5, 21]);
    // Stack: (5 * 4 + 2 - 32, 10 * 4 + 2 - 32); player 0: 64 - 32, 40 - 32.
    assert_eq!(out[0].d, [-10, 10]);
    assert_eq!(out[1].d, [32, 8]);
    // Flag: mapFlag true -> frame 1, offsets negated, alignment 1/1.
    assert_eq!(
        (out[6].d, out[6].align, out[6].offset),
        ([10 * 4 + 2 - 32, 12 * 4 + 2 - 32], [1, 1], [-3, -4])
    );
}

#[test]
fn hint_arrow_tile_packet_retains_and_clears_slot() {
    let sprite = Rc::new(Sprite {
        paletted: None,
        size: [4, 4],
        padding: [0; 4],
        argb: vec![1; 16],
    });
    let mut m = Minimap {
        hintarrows: vec![sprite.clone(), sprite],
        ..Minimap::default()
    };
    let bytes = [0x22, 1, 1, 0x0c, 0x8a, 0x0c, 0x94, 0, 0, 200, 0, 0, 0, 0];
    m.apply_hint_arrow(&bytes, [3200, 3200]).unwrap();
    assert_eq!(
        m.hint_arrow_state[1]
            .as_ref()
            .map(|a| (a.hint_type, a.level, a.fine, a.distance_tiles)),
        Some((2, Some(1), Some([10 * 512 + 256, 20 * 512 + 256]), 200))
    );
    let arrow = m.hint_arrow_state[1].as_ref().unwrap();
    assert_eq!(arrow.hint_type, 2);
    assert_eq!(arrow.fine, Some([10 * 512 + 256, 20 * 512 + 256]));
    assert_eq!(arrow.level, Some(1));
    assert_eq!((arrow.height, arrow.model), (0, 0));
    // A new base shifts the retained tile offset.
    m.rebase_hint_arrows([3208, 3192]);
    assert_eq!(
        m.hint_arrow_state[1].as_ref().unwrap().fine,
        Some([2 * 512 + 256, 28 * 512 + 256])
    );
    m.apply_hint_arrow(&[0x20; 14], [3208, 3192]).unwrap();
    assert!(m.hint_arrow_state[1].is_none());
}

/// NPC/player targets keep the blink rate and the
/// trailing model id; tile targets keep `g1 << 2` height and the model;
/// other types read only the model; an out-of-range sprite is ignored.
#[test]
fn hint_arrow_packet_keeps_model_height_and_blink() {
    let sprite = Rc::new(Sprite {
        paletted: None,
        size: [4, 4],
        padding: [0; 4],
        argb: vec![1; 16],
    });
    let mut m = Minimap {
        hintarrows: vec![sprite.clone(), sprite],
        ..Minimap::default()
    };
    m.apply_hint_arrow(
        &[0x01, 1, 0, 7, 0, 25, 0, 0, 0, 0, 0, 0, 0x1b, 0x3a],
        [0, 0],
    )
    .unwrap();
    let npc = m.hint_arrow_state[0].as_ref().unwrap();
    assert_eq!((npc.npc_index, npc.blink, npc.model), (Some(7), 25, 0x1b3a));
    m.apply_hint_arrow(&[0x42, 0, 1, 0, 3, 0, 4, 9, 0, 5, 0, 0, 0x1b, 0x3a], [0, 0])
        .unwrap();
    let tile = m.hint_arrow_state[2].as_ref().unwrap();
    assert_eq!(
        (tile.height, tile.distance_tiles, tile.model),
        (36, 5, 0x1b3a)
    );
    assert_eq!(tile.fine, Some([3 * 512 + 256, 4 * 512 + 256]));
    m.apply_hint_arrow(&[0x67, 0, 0, 0, 0x1b, 0x3a, 0, 0, 0, 0, 0, 0, 0, 0], [0, 0])
        .unwrap();
    assert_eq!(m.hint_arrow_state[3].as_ref().unwrap().model, 0x1b3a);
    m.apply_hint_arrow(&[0x81, 9, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], [0, 0])
        .unwrap();
    assert!(m.hint_arrow_state[4].is_none());
}

#[test]
fn distant_hint_arrow_builds_a_rotatable_edge_overlay() {
    let sprite = Rc::new(Sprite {
        paletted: None,
        size: [4, 4],
        padding: [0; 4],
        argb: vec![1; 16],
    });
    let edge = Rc::new(Sprite {
        paletted: None,
        size: [6, 6],
        padding: [0; 4],
        argb: vec![2; 36],
    });
    let mut m = Minimap {
        pack: Some(Pack::open("/nonexistent")),
        hintarrows: vec![sprite.clone(), sprite.clone()],
        hintarrow_minimap: vec![sprite.clone(), sprite],
        hintarrow_edges: vec![edge.clone(), edge],
        ..Minimap::default()
    };
    let bytes = [0x22, 1, 1, 0x0c, 0x8a, 0x0c, 0x94, 0, 0, 20, 0, 0, 0, 0];
    m.apply_hint_arrow(&bytes, [3200, 3200]).unwrap();
    let read = |_: bool, _: i32| -> Result<i32> { Ok(0) };
    let overlays = m.overlays(&OverlayInput {
        base: [3200, 3200],
        player: [0, 0],
        level: 1,
        cycle: 0,
        local_size: 1,
        local_team: 0,
        stacks: &[],
        npcs: &[],
        players: &[],
        read: &read,
        flag: MinimapFlag::default(),
    });
    assert_eq!(overlays.len(), 1);
    assert!(overlays[0].edge_arrow);
    assert!(overlays[0]
        .edge_sprite
        .as_ref()
        .is_some_and(|sprite| Rc::ptr_eq(sprite, &m.hintarrow_edges[1])));
}

#[test]
fn msi_decode_and_tint_apply() {
    let t = MsiType::decode(&[1, 0, 5, 2, 0x12, 0x34, 0x56, 3, 0]).unwrap();
    assert_eq!(
        t,
        MsiType {
            sprite: 5,
            tint: 0x123456,
            scale_to_loc: true
        }
    );
    assert_eq!(MsiType::decode(&[1, 0, 5, 4, 0]).unwrap().sprite, -1);
}

/// Setting the minimap flag and the per-frame arrival test: the flag clears when the local player's tile,
/// `(trans - (size - 1) * 256) >> 9`, reaches it, not before.
#[test]
fn minimap_flag_clears_when_the_local_player_reaches_it() {
    let mut flag = MinimapFlag::default();
    assert_eq!((flag.flag, flag.map_flag), ([-1, -1], true));
    flag.set([10, 12]);
    assert_eq!((flag.flag, flag.map_flag), ([10, 12], false));
    flag.arrive([9 * 512 + 256, 12 * 512 + 256], 1);
    assert_eq!(flag.flag, [10, 12], "one tile short");
    // A size-2 actor's trans is its centre, half a tile past its origin.
    flag.arrive([10 * 512 + 512, 12 * 512 + 512], 2);
    assert_eq!(flag.flag, [-1, -1]);
    assert!(!flag.map_flag, "mapFlag stays as the click left it");
}
