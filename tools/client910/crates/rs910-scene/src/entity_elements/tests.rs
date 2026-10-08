use super::*;
use crate::entities910::combat::{Bar as CombatBar, Combat};

struct Fake {
    sprites: BTreeMap<i32, Rc<Sprite>>,
}
fn sprite(w: i32, h: i32, left: i32) -> Rc<Sprite> {
    Rc::new(Sprite {
        paletted: None,
        size: [w, h],
        padding: [left, 0, 0, 0],
        argb: vec![-1; (w * h) as usize],
    })
}
impl Resources for Fake {
    fn config_sprite(&mut self, group: i32) -> Option<Rc<Sprite>> {
        self.sprites.get(&group).cloned()
    }
    fn head_icon(&mut self, group: i32, frame: i32) -> Option<Rc<Sprite>> {
        self.sprites.get(&(group * 100 + frame)).cloned()
    }
    fn hint_arrow(&mut self, _: i32) -> Option<Rc<Sprite>> {
        Some(sprite(24, 20, 0))
    }
    fn has_font(&mut self, font: FontRef) -> bool {
        font == FontRef::Config { id: 7, mono: true }
    }
    fn string_width(&mut self, _: FontRef, text: &str) -> i32 {
        text.chars().count() as i32 * 6
    }
    fn ascent(&mut self, _: FontRef) -> i32 {
        10
    }
    fn descent(&mut self, _: FontRef) -> i32 {
        3
    }
}

fn hit_type() -> Hit {
    Hit {
        damagefont: 7,
        damagecolour: 0xffffff,
        damagecolour_set: true,
        classgraphic: 1,
        leftgraphic: -1,
        middlegraphic: 2,
        rightgraphic: -1,
        sticktime: 100,
        damageformat: "%1".into(),
        fadeat: 50,
        ..Hit::default()
    }
}

fn bar_type() -> Bar {
    Bar {
        full: 10,
        empty: 11,
        sticktime: 300,
        ..Bar::default()
    }
}

#[test]
fn headbar_hitmark_and_chat_follow_draw2d_entity_elements() {
    let hitmarks = BTreeMap::from([(0, hit_type())]);
    let headbars = BTreeMap::from([(0, bar_type())]);
    let read = |_: bool, _: i32| None;
    let frame = Frame {
        loop_cycle: 1000,
        scene_cycle: 1000,
        viewport: [100, 50, 400, 300],
        hitmark_positions: &[[0, 0], [0, 20]],
        headbar_gap: 7,
        player_chat_visible: true,
        npc_chat_visible: true,
        chat_effects: 0,
        public_chat_filter: 0,
        friend_test: &|_: &str| false,
        emoji: None,
        logic_rate: 50,
        hitmarks: &hitmarks,
        headbars: &headbars,
        read_var: &read,
    };
    let mut e = Player {
        fine_x: 1000.0,
        fine_z: 1000.0,
        ..Default::default()
    };
    let mut combat = Combat::new(4);
    // Hit type 0, damage 25, expiring at loopCycle + sticktime (no delay).
    combat.hits[0] = [0, 25, -1, -1, 1000 + 100];
    // Headbar 0: installed at cycle 990 with a 128/255 fill, no duration.
    combat.bars.push(CombatBar {
        id: 0,
        updates: vec![[990, 128, 128, 0]],
    });
    e.combat = Some(combat);
    e.chat = Some(crate::entities910::chat::Chat {
        text: Some("Hello".into()),
        colour: 0,
        effect: 0,
        total: 100,
        time: 50,
    });
    let mut fake = Fake {
        sprites: BTreeMap::from([
            (1, sprite(20, 22, 1)),
            (2, sprite(4, 22, 0)),
            (10, sprite(30, 5, 0)),
            (11, sprite(30, 5, 0)),
        ]),
    };
    let project = |_: i32, _: f32, _: f32, h: i32| [200.0, 150.0 - h as f32];
    let mut entities = [Entity {
        path: &mut e,
        kind: Kind::Npc {
            index: 3,
            head_icons: None,
            bar_partner: 0,
        },
        height: 100,
        skip: false,
        deferred: false,
    }];
    let elements = draw(&frame, &mut entities, &[], &project, &mut fake);
    let draws = elements.entities;
    // Headbar at the entity height: projection y 50 + view_y 50 - b12
    // ascent 10 - bar height 5; x = 200 + 100 - 30/2. The full sprite is
    // clipped to 30 * 128 / 255 = 15 pixels.
    match &draws[0] {
        Draw::Sprite { pos, colour, .. } => {
            assert_eq!(*pos, [285, 85]);
            assert_eq!(*colour, -1);
        }
        other => panic!("{other:?}"),
    }
    match &draws[1] {
        Draw::SetBounds(b) => assert_eq!(*b, [285, 85, 300, 90]),
        other => panic!("{other:?}"),
    }
    assert!(matches!(draws[3], Draw::ResetBounds([100, 50, 500, 350])));
    // Hitmark at height/2: projection y 100. "25" is 12 wide, so
    // 12/4+1 = 4 middles span 16; end = 20 + 2 + 16 = 38 and
    // x = 200 + 100 - 19 = 281, y = 100 + 50 - 12. Remaining 100 cycles
    // with fadeat 50: alpha = (100 << 8) / 50 = 512, opaque.
    let class = draws
        .iter()
        .position(|d| matches!(d, Draw::Sprite { sprite, .. } if sprite.size == [20, 22]))
        .unwrap();
    match &draws[class] {
        Draw::Sprite { pos, colour, .. } => {
            assert_eq!(*pos, [281 - 1, 138]);
            assert_eq!(*colour, -1);
        }
        _ => unreachable!(),
    }
    assert_eq!(
        draws
            .iter()
            .filter(|d| matches!(d, Draw::Sprite { sprite, .. } if sprite.size == [4, 22]))
            .count(),
        4
    );
    let text = draws
        .iter()
        .find_map(|d| match d {
            Draw::Text {
                text,
                pos,
                font,
                colour,
                ..
            } if text == "25" => Some((*pos, *font, *colour)),
            _ => None,
        })
        .unwrap();
    // text_x = (16 - 12) / 2 + 22 = 24 -> 305; y = 138 + 15.
    assert_eq!(text.0, [305, 153]);
    assert_eq!(text.1, FontRef::Config { id: 7, mono: true });
    assert_eq!(text.2, 0xffffff | 0xff000000u32 as i32);
    // Chat: centred b12 text at the entity projection.
    let chat = elements
        .chats
        .iter()
        .find_map(|d| match d {
            Draw::Text {
                text, pos, colour, ..
            } if text == "Hello" => Some((*pos, *colour)),
            _ => None,
        })
        .unwrap();
    assert_eq!(chat.0, [100 + 200 - 15, 50 + 50]);
    assert_eq!(chat.1, 16776960 | 0xff000000u32 as i32);
}

#[test]
fn expired_hitmarks_clear_and_emptied_headbars_unlink() {
    let hitmarks = BTreeMap::from([(0, hit_type())]);
    let headbars = BTreeMap::from([(0, bar_type())]);
    let read = |_: bool, _: i32| None;
    let frame = Frame {
        loop_cycle: 5000,
        scene_cycle: 5000,
        viewport: [0, 0, 100, 100],
        hitmark_positions: &[],
        headbar_gap: 7,
        player_chat_visible: true,
        npc_chat_visible: true,
        chat_effects: 0,
        public_chat_filter: 0,
        friend_test: &|_: &str| false,
        emoji: None,
        logic_rate: 50,
        hitmarks: &hitmarks,
        headbars: &headbars,
        read_var: &read,
    };
    let mut e = Player::default();
    let mut combat = Combat::new(2);
    // A typeless (-1) hit still pending is cleared to -1 by the draw.
    combat.hits[1] = [-1, 5, -1, -1, 4000];
    combat.bars.push(CombatBar {
        id: 0,
        updates: vec![[1000, 255, 255, 0]],
    });
    e.combat = Some(combat);
    let project = |_: i32, _: f32, _: f32, _: i32| [10.0, 10.0];
    let mut entities = [Entity {
        path: &mut e,
        kind: Kind::Player {
            index: 1,
            head_ids: [-1; 8],
            head_groups: [-1; 8],
            partner: 0,
        },
        height: 200,
        skip: false,
        deferred: false,
    }];
    let elements = draw(
        &frame,
        &mut entities,
        &[],
        &project,
        &mut Fake {
            sprites: BTreeMap::new(),
        },
    );
    assert!(elements.entities.is_empty() && elements.chats.is_empty());
    let combat = e.combat.as_ref().unwrap();
    assert_eq!(combat.hits[1][4], -1);
    assert!(combat.bars.is_empty());
}

/// An NPC row picks its headbar sprites from the
/// player in the stale `highResolutionsIndices` slot at its row.
#[test]
fn npc_headbar_sprites_follow_the_stale_player_slot() {
    let hitmarks = BTreeMap::new();
    let headbars = BTreeMap::from([(
        0,
        Bar {
            emptylocalpartner: 20,
            fulllocalpartner: 21,
            ..bar_type()
        },
    )]);
    let read = |_: bool, _: i32| None;
    let frame = Frame {
        loop_cycle: 1000,
        scene_cycle: 1000,
        viewport: [0, 0, 400, 300],
        hitmark_positions: &[],
        headbar_gap: 7,
        player_chat_visible: true,
        npc_chat_visible: true,
        chat_effects: 0,
        logic_rate: 50,
        hitmarks: &hitmarks,
        headbars: &headbars,
        read_var: &read,
        public_chat_filter: 0,
        friend_test: &|_: &str| false,
        emoji: None,
    };
    let mut fake = Fake {
        sprites: BTreeMap::from([
            (10, sprite(30, 5, 0)),
            (11, sprite(30, 5, 0)),
            (20, sprite(40, 6, 0)),
            (21, sprite(40, 6, 0)),
        ]),
    };
    let project = |_: i32, _: f32, _: f32, _: i32| [100.0, 100.0];
    for (bar_partner, width) in [(0, 30), (1, 40)] {
        let mut e = Player::default();
        let mut combat = Combat::new(1);
        combat.bars.push(CombatBar {
            id: 0,
            updates: vec![[990, 255, 255, 0]],
        });
        e.combat = Some(combat);
        let mut entities = [Entity {
            path: &mut e,
            kind: Kind::Npc {
                index: 3,
                head_icons: None,
                bar_partner,
            },
            height: 100,
            skip: false,
            deferred: false,
        }];
        let draws = draw(&frame, &mut entities, &[], &project, &mut fake).entities;
        match &draws[0] {
            Draw::Sprite { sprite, .. } => assert_eq!(sprite.size[0], width),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn multimark_selects_visible_type_by_varp() {
    let mut parent = hit_type();
    parent.multivarp = 5;
    parent.multimark = Some(vec![9, -1, 8]);
    let hitmarks = BTreeMap::from([
        (0, parent),
        (
            8,
            Hit {
                damagefont: 3,
                ..hit_type()
            },
        ),
        (9, hit_type()),
    ]);
    let headbars = BTreeMap::new();
    let read = |bit: bool, id: i32| (!bit && id == 5).then_some(7);
    let frame = Frame {
        loop_cycle: 0,
        scene_cycle: 0,
        viewport: [0; 4],
        hitmark_positions: &[],
        headbar_gap: 0,
        player_chat_visible: true,
        npc_chat_visible: true,
        chat_effects: 0,
        public_chat_filter: 0,
        friend_test: &|_: &str| false,
        emoji: None,
        logic_rate: 50,
        hitmarks: &hitmarks,
        headbars: &headbars,
        read_var: &read,
    };
    // Value 7 is outside 0..len-1: the trailing default (8) is chosen.
    assert_eq!(visible_hitmark(&frame, 0).unwrap().damagefont, 3);
    let read1 = |_: bool, _: i32| Some(1);
    let frame1 = Frame {
        read_var: &read1,
        ..frame
    };
    assert!(visible_hitmark(&frame1, 0).is_none());
    assert_eq!(
        format_damage(
            &Hit {
                damageformat: "-%1-".into(),
                damagescaleto: 10,
                ..hit_type()
            },
            7
        ),
        "-70-"
    );
}

#[test]
fn npc_scene_flags_defer_the_lower_priority_npc_on_a_shared_tile() {
    use crate::protocol910::npc::Npcs;
    let players = crate::protocol910::Players::default();
    let mut npcs = Npcs::default();
    for (index, x) in [(1usize, 10), (2, 10), (3, 12)] {
        let mut n = crate::entities910::Npc::new([0; 4]);
        n.type_id = 1;
        n.path.tele(x, 10);
        npcs.entities.insert(index, n);
        npcs.slots.push(index);
    }
    // NPC 2 is a follower: 64 instead of 128 under draw order 0.
    let types = |n: &crate::entities910::Npc| {
        Some(NpcSceneType {
            follower: n.update_serial == 7,
            ..Default::default()
        })
    };
    npcs.entities.get_mut(&2).unwrap().update_serial = 7;
    npc_scene_flags(&players, &mut npcs, &types, [104, 104], 0, 0, &[3]);
    let p = |i: usize| npcs.entities[&i].path.actor.scene.priority;
    // Default deferred=true: 0 + (5-1)<<2 + 128 + 256 + 1.
    assert_eq!(p(1), 16 + 128 + 256 + 1);
    assert_eq!(p(2), 16 + 64 + 256 + 1);
    assert_eq!(p(3), 16 + 128 + 256 + 1 + 2048);
    let d = |i: usize| npcs.entities[&i].path.actor.scene.deferred;
    assert!(!d(1));
    assert!(d(2));
    assert!(!d(3));
}

#[test]
fn npc_head_icons_stack_above_the_bar_gap_and_hint_arrows_blink() {
    let hitmarks = BTreeMap::new();
    let headbars = BTreeMap::new();
    let read = |_: bool, _: i32| None;
    let frame = Frame {
        loop_cycle: 10,
        scene_cycle: 10,
        viewport: [0, 0, 400, 300],
        hitmark_positions: &[],
        headbar_gap: 7,
        player_chat_visible: true,
        npc_chat_visible: true,
        chat_effects: 0,
        public_chat_filter: 0,
        friend_test: &|_: &str| false,
        emoji: None,
        logic_rate: 50,
        hitmarks: &hitmarks,
        headbars: &headbars,
        read_var: &read,
    };
    let mut e = Player::default();
    let icons = (
        [4, -1, 4, -1, -1, -1, -1, -1],
        [1, -1, 0, -1, -1, -1, -1, -1],
    );
    let mut fake = Fake {
        sprites: BTreeMap::from([(401, sprite(16, 12, 0)), (400, sprite(20, 10, 0))]),
    };
    let project = |_: i32, _: f32, _: f32, _: i32| [100.0, 100.0];
    // Blink 1000 at logic rate 50: half = 25 cycles; cycle 10 is visible.
    let arrows = [HintArrow {
        hint_type: 1,
        target: 9,
        sprite: 0,
        blink: 1000,
    }];
    let mut entities = [Entity {
        path: &mut e,
        kind: Kind::Npc {
            index: 9,
            head_icons: Some(&icons),
            bar_partner: 0,
        },
        height: 200,
        skip: false,
        deferred: false,
    }];
    let out = draw(&frame, &mut entities, &arrows, &project, &mut fake).entities;
    let pos: Vec<[i32; 2]> = out
        .iter()
        .map(|d| match d {
            Draw::Sprite { pos, .. } => *pos,
            other => panic!("{other:?}"),
        })
        .collect();
    // y starts at 100 - ascent 10 - (gap 7 + 2) = 81; icon slot 0 (16x12)
    // centred, then slot 2 (20x10), then the arrow 12 px left of centre.
    assert_eq!(pos, vec![[92, 69], [90, 57], [88, 55 - 20]]);
    // Cycle 30 falls in the dark half of the blink.
    let frame = Frame {
        loop_cycle: 30,
        ..frame
    };
    let mut e = Player::default();
    let mut entities = [Entity {
        path: &mut e,
        kind: Kind::Npc {
            index: 9,
            head_icons: None,
            bar_partner: 0,
        },
        height: 200,
        skip: false,
        deferred: false,
    }];
    assert!(draw(&frame, &mut entities, &arrows, &project, &mut fake)
        .entities
        .is_empty());
}

#[test]
fn chat_colours_follow_client_draw2d_entity_elements() {
    assert_eq!(chat_colour(0, 0, 0), 16776960);
    assert_eq!(chat_colour(5, 0, 0), 16777215);
    assert_eq!(chat_colour(6, 3, 0), 16711680);
    assert_eq!(chat_colour(6, 13, 0), 16776960);
    assert_eq!(chat_colour(9, 0, 10), 10 * 1280 + 16711680);
    assert_eq!(chat_colour(9, 0, 120), 20 * 5 + 65280);
    assert_eq!(chat_colour(10, 0, 60), 16711935 - 10 * 327680);
    assert_eq!(chat_colour(11, 0, 20), 16777215 - 20 * 327685);
    assert_eq!(chat_colour(42, 0, 0), 16776960);
}

#[test]
fn overlapping_chat_lines_stack_upwards() {
    // The later of two coincident lines moves one
    // line up; a line without horizontal overlap stays.
    let mut positions = [[100, 200], [102, 201], [400, 200]];
    for n in 0..positions.len() {
        stack_chat_line(&mut positions, &[30, 30, 30], n, 17);
    }
    assert_eq!(positions[0], [100, 200]);
    assert_eq!(positions[1], [102, 183]);
    assert_eq!(positions[2], [400, 200], "no horizontal overlap");
}

#[test]
fn chat_wave_and_shake_offsets() {
    let (x, y) = chat_effect_offsets(1, 3, 10, 0);
    assert!(x.is_none());
    let y = y.unwrap();
    assert_eq!(y[0], ((10.0_f64 / 5.0).sin() * 5.0) as i32);
    assert_eq!(y[2], ((10.0_f64 / 5.0 + 1.0).sin() * 5.0) as i32);
    let (x2, y2) = chat_effect_offsets(2, 2, 7, 0);
    assert_eq!(x2.unwrap()[1], ((7.0_f64 / 5.0 + 0.2).sin() * 5.0) as i32);
    assert_eq!(
        y2.unwrap()[1],
        ((7.0_f64 / 5.0 + 1.0 / 3.0).sin() * 5.0) as i32
    );
    // Shake amplitude 7 - progress/8 clamps at zero.
    let (_, y3) = chat_effect_offsets(3, 4, 1, 100);
    assert!(y3.unwrap().iter().all(|v| *v == 0));
    assert_eq!(chat_effect_offsets(0, 4, 1, 0), (None, None));
}

#[test]
fn player_chat_follows_public_filter_friend_test_and_emoji() {
    // Which player chat lines are collected, and the emoji substitution.
    let (hitmarks, headbars) = (BTreeMap::new(), BTreeMap::new());
    let read = |_: bool, _: i32| None;
    let friends = |name: &str| name == "Friend";
    let emoji = |text: &str| text.replace(":)", "<img=1>");
    let frame = |filter: i32| Frame {
        loop_cycle: 0,
        scene_cycle: 0,
        viewport: [0, 0, 500, 300],
        hitmark_positions: &[],
        headbar_gap: 0,
        player_chat_visible: true,
        npc_chat_visible: true,
        chat_effects: 0,
        public_chat_filter: filter,
        friend_test: &friends,
        emoji: Some(&emoji),
        logic_rate: 50,
        hitmarks: &hitmarks,
        headbars: &headbars,
        read_var: &read,
    };
    let chats = |filter: i32, name: &str| -> Vec<String> {
        let mut e = Player::default();
        e.appearance.name = Some(name.into());
        e.chat = Some(crate::entities910::chat::Chat {
            text: Some("hi :)".into()),
            colour: 0,
            effect: 0,
            total: 100,
            time: 100,
        });
        let mut entities = [Entity {
            path: &mut e,
            kind: Kind::Player {
                index: 1,
                head_ids: [-1; 8],
                head_groups: [-1; 8],
                partner: 0,
            },
            height: 0,
            skip: false,
            deferred: false,
        }];
        let project = |_: i32, _: f32, _: f32, _: i32| [100.0, 100.0];
        let mut fake = Fake {
            sprites: BTreeMap::new(),
        };
        draw(&frame(filter), &mut entities, &[], &project, &mut fake)
            .chats
            .into_iter()
            .filter_map(|d| match d {
                Draw::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    };
    assert_eq!(chats(0, "Stranger"), vec!["hi <img=1>".to_string()]);
    assert_eq!(chats(3, "Stranger").len(), 1);
    assert!(chats(1, "Stranger").is_empty());
    assert_eq!(chats(1, "Friend").len(), 1);
    assert!(chats(2, "Friend").is_empty());
}
