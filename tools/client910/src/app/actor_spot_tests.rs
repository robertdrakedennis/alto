use super::ActorSpot;

#[test]
fn slot_offsets_follow_add_spot_anims() {
    let mut actor = crate::entities910::Player {
        angle: 4096,
        ..Default::default()
    };
    actor.actor.scene.decoration_offset = 7;
    let mut spot = crate::entities910::animation_state::Spot {
        id: 1,
        height: 3,
        delay: 1,
        ..Default::default()
    };
    let bas = crate::protocol910::bas_types::Bas {
        slot_transforms: Some(vec![None, Some(vec![10, 20, 30])]),
        slot_offsets: Some(vec![None, Some(vec![1, 2, 3])]),
        ..Default::default()
    };
    let a = ActorSpot::new(&actor, &spot, Some(&bas));
    // Both offset tables sum; with no wear angle the slot takes the
    // actor's angle; the y offset combines the fixed -5 with the height term.
    assert_eq!(a.slot, Some(([11, 22, 33], 4096)));
    assert_eq!(a.y_offset, -12);
    assert_eq!(a.height, 3);
    actor.actor.wear_angles = Some(vec![-1, 8192]);
    assert_eq!(
        ActorSpot::new(&actor, &spot, Some(&bas)).slot,
        Some(([11, 22, 33], 8192))
    );
    // A slot without a transform row, or no slot, takes the plain path.
    spot.delay = 0;
    assert_eq!(ActorSpot::new(&actor, &spot, Some(&bas)).slot, None);
    spot.delay = -1;
    assert_eq!(ActorSpot::new(&actor, &spot, Some(&bas)).slot, None);
}
