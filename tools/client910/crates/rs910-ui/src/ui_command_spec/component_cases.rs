//! Interface component fixtures, resources and model customisation cases.

use rs910_core::fault::Fault;

use std::{collections::BTreeMap, rc::Rc};

use super::{c, hex, rec, units, Case, ComField, Obs, Set, World, C0, C1, IFACE};

/// A font 494 of fixed 6-pixel advances; line height 12, and metrics 3 and
/// 4, ascent 9, descent 2.
pub(super) fn font_bytes() -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend([6u8; 256]);
    b.extend([10u8; 256]);
    b.extend([0u8; 256]);
    b.extend([0, 0, 0, 0]);
    b.extend(vec![0u8; 1024]);
    b.extend([12, 3, 4, 9, 2, 1]);
    b
}

pub(super) struct FontSource;

impl crate::ui_fonts::Source for FontSource {
    fn load(&mut self, _: crate::ui_fonts::Archive, _: i32) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn fetch(&mut self, a: crate::ui_fonts::Archive, id: i32) -> anyhow::Result<Option<Vec<u8>>> {
        Ok((matches!(a, crate::ui_fonts::Archive::Metrics) && id == 494).then(font_bytes))
    }
    fn sprite_file(&mut self, _: i32) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

pub(super) fn test_fonts(w: &mut World) {
    w.props.fonts = Some(crate::ui_fonts::Fonts::new(
        Box::new(FontSource),
        None,
        None,
    ));
}

/// Interface 77 with a text component 0 ("Hello world", font 494, width 200).
pub(super) fn text_component(w: &mut World) {
    test_fonts(w);
    Set::Iface(IFACE, 2).apply(w, false);
    let com = w.com(IFACE, 0);
    let mut c = com.borrow_mut();
    c.f.textfont = 494;
    c.f.text = Some(units("Hello world"));
    c.f.width = 200;
    c.f.textLineHeight = 0;
}

/// A paletted 3x2 sprite on a 12x10 canvas (left 1, top 2): the
/// sprite data trailer layout.
pub(super) struct SpriteSource;

impl crate::ui_sprites::Source for SpriteSource {
    fn file(&mut self, id: i32) -> anyhow::Result<Option<Vec<u8>>> {
        Ok((id == 900).then(|| {
            vec![
                0, 1, 1, 0, 1, 0, 1, // flags, indices
                0xFF, 0, 0, // palette[1]
                0, 12, 0, 10, 1, // canvas 12x10, 2 colours
                0, 1, 0, 2, 0, 3, 0, 2, // left, top, width, height
                0, 1, // one sprite
            ]
        }))
    }
}

pub(super) fn graphic_component(w: &mut World) {
    w.props.sprites = Some(crate::ui_sprites::Resources::new(Box::new(SpriteSource)));
    Set::Iface(IFACE, 2).apply(w, false);
    w.com(IFACE, 0).borrow_mut().f.graphic = 900;
}

/// `if_*`/`cc_*` component commands on interface 77.
pub(super) fn interfaces() -> Vec<Case> {
    let iface = Set::Iface(IFACE, 2);
    let mut v = vec![];
    // if_debug_button1..10 trigger the component's operation: the default
    // active mask enables op n, so the in-game client sends IF_BUTTONn.
    for op in 1..=10 {
        let name: &'static str = Box::leak(format!("if_debug_button{op}").into_boxed_str());
        v.push(
            rec(name)
                .set(iface)
                .set(Set::Com(IFACE, 0, ComField::Events(1 << op)))
                .set(Set::Com(
                    IFACE,
                    0,
                    ComField::InvObject(rs910_symbols::obj::COINS.id()),
                ))
                .set(Set::State(18))
                .i(&[C0, -1])
                .obs(Obs::Out),
        );
    }
    let named = |cmd| {
        rec(cmd)
            .set(iface)
            .set(Set::Com(IFACE, 0, ComField::Name("debug_iface:root")))
            .set(Set::Com(IFACE, 1, ComField::Name("debug_iface:button")))
            .set(Set::Com(IFACE, 1, ComField::ServerTriggers(0x2A)))
    };
    v.extend([
        named("if_debug_getcomcount").i(&[IFACE]),
        named("if_debug_getcomcount").i(&[78]),
        named("if_debug_getcomname").i(&[C1]),
        named("if_debug_getcomname").i(&[78 << 16]),
        named("if_debug_getname").i(&[IFACE]),
        named("if_debug_getservertriggers").i(&[C1]),
        // The original indexes `components` with the whole packed id: only
        // interface 0 reaches a component by value.
        rec("if_debug_getcomname")
            .set(Set::Iface(0, 2))
            .set(Set::Com(0, 1, ComField::Name("root:button")))
            .i(&[1]),
        rec("if_debug_getservertriggers")
            .set(Set::Iface(0, 2))
            .set(Set::Com(0, 1, ComField::ServerTriggers(0x2A)))
            .i(&[1]),
        rec("if_debug_getopenifid").set(Set::TopIf(548)).i(&[0]),
    ]);
    // NPC head / model and player head / model setters.
    for (cmd, ints) in [
        ("if_setnpchead", &[3, C1][..]),
        ("if_setnpcmodel", &[3, C1][..]),
        ("if_setplayerhead_self", &[C1][..]),
        ("if_setplayermodel", &[5, C1][..]),
        ("cc_setnpchead", &[3][..]),
        ("cc_setnpcmodel", &[3][..]),
        ("cc_setplayerhead_self", &[][..]),
        ("cc_setplayermodel", &[5][..]),
    ] {
        v.push(
            rec(cmd)
                .set(iface)
                .set(Set::Active(IFACE, 1))
                .set(Set::Uid(17))
                .i(ints)
                .obs(Obs::Com(IFACE, 1)),
        );
    }
    // Setting an NPC head also drops an existing customisation.
    v.push(
        c("if_setnpchead")
            .with(|w| {
                npc_types(w);
                let npc = crate::config::decode_npc(3, &[1, 3, 0, 10, 127, 255, 0, 12, 0]).unwrap();
                let custom = crate::ui_models::NpcCustomisation::new(&npc, true).unwrap();
                w.com(IFACE, 1).borrow_mut().npc_customisation = Some(custom);
            })
            .i(&[3, C1])
            .wo(Obs::Com(IFACE, 1), "2,3,null,null,nocustom"),
    );
    // Secondary active component (the `.sec()` operand flag).
    v.push(
        rec("cc_setnpcmodel")
            .sec()
            .set(iface)
            .set(Set::Active(IFACE, 0))
            .i(&[7])
            .obs(Obs::Com(IFACE, 0)),
    );
    // Link setters (group user kind ids).
    v.extend([
        rec("if_setlinkfriend")
            .set(iface)
            .set(Set::Friends(&["Ann", "Ben"]))
            .set(Set::FriendsState(2))
            .i(&[1, C1])
            .obs(Obs::Com(IFACE, 1)),
        rec("if_setlinkfriend")
            .set(iface)
            .set(Set::Friends(&["Ann", "Ben"]))
            .set(Set::FriendsState(1))
            .i(&[1, C1])
            .obs(Obs::Com(IFACE, 1)),
        rec("cc_setlinkfriend")
            .set(iface)
            .set(Set::Active(IFACE, 1))
            .set(Set::Friends(&["Ann", "Ben"]))
            .set(Set::FriendsState(2))
            .i(&[0])
            .obs(Obs::Com(IFACE, 1)),
        rec("if_setlinkfriendchat")
            .set(iface)
            .set(Set::ClanChat(&["Cat", "Dan"]))
            .i(&[1, C1])
            .obs(Obs::Com(IFACE, 1)),
        rec("cc_setlinkfriendchat")
            .set(iface)
            .set(Set::Active(IFACE, 1))
            .set(Set::ClanChat(&["Cat", "Dan"]))
            .i(&[0])
            .obs(Obs::Com(IFACE, 1)),
        // The link is the player group member's display name (kind 1) or the
        // banned list entry (kind 2).
        c("if_setlinkplayergroup")
            .with(|w| {
                Set::Iface(IFACE, 2).apply(w, false);
                w.props.game.player_group = Some((vec!["Cat".into()], vec!["Dan".into()]));
            })
            .i(&[0, 1, C1])
            .wo(
                Obs::Com(IFACE, 1),
                &format!("1,0,{},1,nocustom", hex("Cat")),
            ),
        c("cc_setlinkplayergroup")
            .with(|w| {
                Set::Iface(IFACE, 2).apply(w, false);
                Set::Active(IFACE, 1).apply(w, false);
                w.props.game.player_group = Some((vec!["Cat".into()], vec!["Dan".into()]));
            })
            .i(&[0, 0])
            .wo(
                Obs::Com(IFACE, 1),
                &format!("1,0,{},2,nocustom", hex("Dan")),
            ),
    ]);
    // Hook setters that parse and discard (gamepad, pinch, discard hook):
    // script id, then args by the signature, an int array when it ends in
    // 'Y'.
    for cmd in [
        "if_setongamepadaxis",
        "if_setongamepadbutton",
        "if_setongamepadbuttonheld",
        "if_setongamepadtrigger",
        "if_setonhorizontalpinch",
        "if_setonverticalpinch",
        "if_discardGestureHook",
    ] {
        v.push(rec(cmd).set(iface).i(&[1234, 5, C1]).s(&["x", "is"]));
    }
    for cmd in [
        "cc_setongamepadaxis",
        "cc_setongamepadbutton",
        "cc_setongamepadbuttonheld",
        "cc_setongamepadtrigger",
        "cc_setonhorizontalpinch",
        "cc_setonverticalpinch",
        "cc_discardGestureHook",
    ] {
        v.push(
            rec(cmd)
                .set(iface)
                .set(Set::Active(IFACE, 1))
                .i(&[1234, 7, 8, 9, 2])
                .s(&["iY"]),
        );
    }
    v.push(rec("if_setongamepadaxis").set(iface).i(&[-1, C1]).s(&[""]));
    // Resume a pause button (p4_alt3 parentlayer, p2_alt2 id).
    for (cmd, ints) in [
        ("if_resume_pausebutton", &[C1][..]),
        ("cc_resume_pausebutton", &[][..]),
    ] {
        v.push(
            rec(cmd)
                .set(iface)
                .set(Set::Active(IFACE, 1))
                .set(Set::Com(IFACE, 1, ComField::Events(1)))
                .i(ints)
                .obs(Obs::Out)
                .obs(Obs::Pressed),
        );
    }
    v.push(
        rec("if_resume_pausebutton")
            .set(iface)
            .i(&[C1])
            .obs(Obs::Out)
            .obs(Obs::Pressed)
            .note("no pausebutton bit in the active mask"),
    );
    // Character position at an index and character index at a position
    // on "Hello world" (one 66px line of 6px advances, line height 12):
    // index 5 → ("Hello" = 30, ascent 9); x 20 → the loop
    // stops at 4 chars (width 24), 20 - 18 < 24 - 20 → index 3.
    v.extend([
        c("if_getcharposatindex")
            .with(text_component)
            .i(&[5, C0])
            .wi(&[30, 9]),
        c("cc_getcharposatindex")
            .with(|w| {
                text_component(w);
                Set::Active(IFACE, 0).apply(w, false);
            })
            .i(&[5])
            .wi(&[30, 9]),
        c("if_getcharindexatpos")
            .with(text_component)
            .i(&[20, 5, C0])
            .wi(&[3]),
        c("cc_getcharindexatpos")
            .with(|w| {
                text_component(w);
                Set::Active(IFACE, 0).apply(w, false);
            })
            .i(&[20, 5])
            .wi(&[3]),
    ]);
    // The graphic size is the padded 12x10 canvas of sprite 900.
    v.extend([
        c("if_getgraphicdimensions")
            .with(graphic_component)
            .i(&[C0])
            .wi(&[12, 10]),
        c("cc_getgraphicdimensions")
            .with(|w| {
                graphic_component(w);
                Set::Active(IFACE, 0).apply(w, false);
            })
            .wi(&[12, 10]),
    ]);
    v
}

/// NPC type 3: models {10, -1, 12}; recol pairs (1→2, 3→4) with
/// recolindices {0, -1, 1} (the existing decoder fixture bytes).
pub(super) fn npc_types(w: &mut World) {
    let npc = crate::config::decode_npc(
        3,
        &[
            1, 3, 0, 10, 127, 255, 0, 12, 40, 2, 0, 1, 0, 2, 0, 3, 0, 4, 44, 0, 5, 0,
        ],
    )
    .unwrap();
    w.props.npcs = Some(Rc::new(crate::config::NpcStore::from_map(
        [(3, npc)].into_iter().collect(),
    )));
    Set::Iface(IFACE, 2).apply(w, false);
    Set::Active(IFACE, 1).apply(w, false);
    let com = w.com(IFACE, 1);
    let mut c = com.borrow_mut();
    c.f.modelkind = 6;
    c.f.model = 3;
}

/// NPC type 4 with one head model (opcode 60: count 1, head 20), shown as
/// a head (modelkind 2) on component 77:1.
pub(super) fn npc_head(w: &mut World) {
    npc_types(w);
    let npc = crate::config::decode_npc(4, &[60, 1, 0, 20, 0]).unwrap();
    let mut store = BTreeMap::new();
    store.insert(4, npc);
    w.props.npcs = Some(Rc::new(crate::config::NpcStore::from_map(store)));
    let com = w.com(IFACE, 1);
    let mut c = com.borrow_mut();
    c.f.modelkind = 2;
    c.f.model = 4;
}

pub(super) fn custom(w: &World) -> Result<crate::ui_models::NpcCustomisation, String> {
    w.com(IFACE, 1)
        .borrow()
        .npc_customisation
        .clone()
        .ok_or_else(|| "no NPC customisation".to_owned())
}

/// Custom body / head models and the recolour / retexture setters over an
/// NPC customisation.
pub(super) fn npc_customisation() -> Vec<Case> {
    let wrong_kind = |cmd, ints: &[i32]| {
        rec(cmd)
            .set(Set::Iface(IFACE, 2))
            .set(Set::Active(IFACE, 1))
            .i(ints)
            .note("model kind 1 fails before the NPC type lookup")
    };
    vec![
        wrong_kind("if_npc_setcustombodymodel", &[1, 99, C1]),
        wrong_kind("cc_npc_setcustombodymodel", &[1, 99]),
        wrong_kind(
            "if_npc_setcustombodymodel_transformed",
            &[1, 99, 3, 2, 0, 0, 0, 1, 2, 3, C1],
        ),
        wrong_kind(
            "cc_npc_setcustombodymodel_transformed",
            &[1, 99, 3, 2, 0, 0, 0, 1, 2, 3],
        ),
        wrong_kind("if_npc_setcustomheadmodel", &[1, 99, C1]),
        wrong_kind("cc_npc_setcustomheadmodel", &[1, 99]),
        wrong_kind("if_npc_setcustomrecol", &[1, 555, C1]),
        wrong_kind("cc_npc_setcustomrecol", &[1, 555]),
        wrong_kind("if_npc_setcustomretex", &[1, 555, C1]),
        wrong_kind("cc_npc_setcustomretex", &[1, 555]),
        // Slot 2 - 1 = 1 of the body models {10, -1, 12} → 99.
        c("if_npc_setcustombodymodel")
            .with(npc_types)
            .i(&[2, 99, C1])
            .check(|w| {
                let m = custom(w)?.models;
                (m == [10, 99, 12]).then_some(()).ok_or(format!("{m:?}"))
            }),
        c("cc_npc_setcustombodymodel")
            .with(npc_types)
            .i(&[3, 77])
            .check(|w| {
                let m = custom(w)?.models;
                (m == [10, -1, 77]).then_some(()).ok_or(format!("{m:?}"))
            }),
        // Scale 3 / max(1, 2) = 1.5; offset (1, 2, 3).
        c("if_npc_setcustombodymodel_transformed")
            .with(npc_types)
            .i(&[2, 99, 3, 2, 0, 0, 0, 1, 2, 3, C1])
            .check(|w| {
                let c = custom(w)?;
                (c.models == [10, 99, 12]
                    && c.custom_scale[1] == 1.5
                    && c.custom_offset[1] == [1, 2, 3])
                .then_some(())
                .ok_or(format!(
                    "{:?} {:?} {:?}",
                    c.models, c.custom_scale, c.custom_offset
                ))
            }),
        c("cc_npc_setcustombodymodel_transformed")
            .with(npc_types)
            .i(&[1, 98, 5, 0, 0, 0, 0, 4, 5, 6])
            .check(|w| {
                let c = custom(w)?;
                (c.models == [98, -1, 12]
                    && c.custom_scale[0] == 5.0
                    && c.custom_offset[0] == [4, 5, 6])
                .then_some(())
                .ok_or(format!(
                    "{:?} {:?} {:?}",
                    c.models, c.custom_scale, c.custom_offset
                ))
            }),
        // Slot 13 - 1 = 12 is outside 0..12: invalid state.
        c("if_npc_setcustombodymodel")
            .with(npc_types)
            .i(&[13, 99, C1])
            .throws(Fault::InvalidState),
        // Head model slot 1 - 1 = 0 (applyCustomHeadModel).
        c("if_npc_setcustomheadmodel")
            .with(npc_head)
            .i(&[1, 5, C1])
            .check(|w| {
                let c = custom(w)?;
                (c.models == [5])
                    .then_some(())
                    .ok_or(format!("{:?}", c.models))
            }),
        // NPC 3 has no head models: building its customisation reads the
        // absent head list.
        c("cc_npc_setcustomheadmodel")
            .with(|w| {
                npc_types(w);
                w.com(IFACE, 1).borrow_mut().f.modelkind = 2;
            })
            .i(&[1, 5])
            .throws(Fault::MissingValue),
        c("cc_npc_setcustomheadmodel")
            .with(npc_head)
            .i(&[6, 5])
            .throws(Fault::InvalidState)
            .note("head slot 5 is outside 0..5"),
        // Recol slot 3 → recolindices[2] = 1 → customRecolD[1].
        c("if_npc_setcustomrecol")
            .with(npc_types)
            .i(&[3, 555, C1])
            .check(|w| {
                let d = custom(w)?.custom_recol_d;
                (d == Some(vec![2, 555]))
                    .then_some(())
                    .ok_or(format!("{d:?}"))
            }),
        c("cc_npc_setcustomrecol")
            .with(npc_types)
            .i(&[1, 444])
            .check(|w| {
                let d = custom(w)?.custom_recol_d;
                (d == Some(vec![444, 4]))
                    .then_some(())
                    .ok_or(format!("{d:?}"))
            }),
        // recolour index 1 is -1: invalid state.
        c("if_npc_setcustomrecol")
            .with(npc_types)
            .i(&[2, 555, C1])
            .throws(Fault::InvalidState),
        // NPC 3 has no retexture table: invalid state.
        c("if_npc_setcustomretex")
            .with(npc_types)
            .i(&[1, 555, C1])
            .throws(Fault::InvalidState),
        c("cc_npc_setcustomretex")
            .with(npc_types)
            .i(&[1, 555])
            .throws(Fault::InvalidState),
    ]
}
