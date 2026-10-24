use super::*;

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
