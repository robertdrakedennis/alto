use super::*;
use crate::{
    ui_components::{Arg, Interface},
    ui_draw::Frame,
    ui_loop::{Input, Transmits},
};
use rs910_symbols::{component, interface, obj};

#[derive(Default)]
struct Hooks {
    events: Vec<(i32, [i32; 2], Option<i32>)>,
}
impl Executor for Hooks {
    fn run(&mut self, _: &mut Store, _: &mut State, r: Request, _: usize) -> Result<()> {
        self.events.push((
            r.script_id()?,
            r.mouse,
            r.drop.as_ref().map(|c| c.borrow().f.parentlayer),
        ));
        Ok(())
    }
}
fn scene() -> (Store, State, Frame, Ref, Ref) {
    let root = Rc::new(RefCell::new(Component::default()));
    {
        let f = &mut root.borrow_mut().f;
        f.parentlayer = 65536;
        f.layer = -1;
        f.width = 200;
        f.height = 120;
    }
    let c = Rc::new(RefCell::new(Component::default()));
    {
        let mut b = c.borrow_mut();
        let f = &mut b.f;
        f.parentlayer = 65537;
        f.layer = 65536;
        f.x = 10;
        f.y = 20;
        f.width = 20;
        f.height = 10;
        f.hashook = true;
        f.dragdeadtime = 1;
        f.dragdeadzone = 5;
        b.draggable = Some(root.clone());
        b.hooks.insert("ondrag", vec![Arg::Int(1)]);
        b.hooks.insert("ondragcomplete", vec![Arg::Int(2)]);
    }
    let target = Rc::new(RefCell::new(Component::default()));
    {
        let mut b = target.borrow_mut();
        b.f.parentlayer = 65538;
        b.f.layer = 65536;
        b.f.x = 100;
        b.f.y = 50;
        b.f.width = 30;
        b.f.height = 30;
        b.default_active[0] = 1 << 21;
    }
    let mut store = Store::default();
    store.interfaces.insert(
        1,
        Interface::new(vec![Some(root), Some(c.clone()), Some(target.clone())]),
    );
    let mut state = State::default();
    state.life.top = 1;
    state.layout.canvas = [200, 120];
    (store, state, Frame::default(), c, target)
}
fn cycle(
    store: &mut Store,
    state: &mut State,
    frame: &mut Frame,
    hooks: &mut Hooks,
    mouse: [i32; 2],
    held: bool,
    click: bool,
) -> Option<[i32; 2]> {
    begin_cycle(state, mouse);
    let input = Input {
        mouse,
        left_held: held,
        click: click.then_some(mouse),
        ..Default::default()
    };
    crate::ui_loop::update_top_level(store, state, frame, &input, &Transmits::default()).unwrap();
    finish_drag(store, state, hooks, held).unwrap()
}
#[test]
fn drag_threshold_hooks_clamping_and_release_use_live_component_graph() {
    let (mut store, mut state, mut frame, c, _) = scene();
    let mut hooks = Hooks::default();
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [15, 25],
        true,
        true,
    );
    assert!(!state.interaction.drag.active);
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [20, 25],
        true,
        false,
    );
    assert!(
        hooks.events.is_empty(),
        "exact deadzone boundary must remain a click"
    );
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [26, 25],
        true,
        false,
    );
    assert_eq!(hooks.events, vec![(1, [21, 20], None)]);
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [110, 60],
        false,
        false,
    );
    assert_eq!(hooks.events.last(), Some(&(2, [105, 55], Some(65538))));
    assert!(state.interaction.drag.component.is_none());
    assert!(
        state.interaction.outgoing.is_empty(),
        "explicit draggable parent alone does not authorize IF_BUTTOND"
    );
    c.borrow_mut().default_active[0] = 1 << 18;
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [15, 25],
        true,
        true,
    );
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [999, 999],
        true,
        false,
    );
    assert_eq!(hooks.events.last(), Some(&(1, [180, 110], None)));
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [110, 60],
        false,
        false,
    );
    assert_eq!(state.interaction.outgoing.len(), 17);
    assert_eq!(
        state.interaction.outgoing[0],
        crate::proto::client::IF_BUTTOND
    );
}
#[test]
fn short_click_and_detached_drag_have_distinct_release_behavior() {
    let (mut store, mut state, mut frame, c, _) = scene();
    let mut hooks = Hooks::default();
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [15, 25],
        true,
        true,
    );
    assert_eq!(
        cycle(
            &mut store,
            &mut state,
            &mut frame,
            &mut hooks,
            [15, 25],
            false,
            false
        ),
        Some([15, 25])
    );
    assert!(hooks.events.is_empty());
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [15, 25],
        true,
        true,
    );
    c.borrow_mut().f.hide = true;
    cycle(
        &mut store,
        &mut state,
        &mut frame,
        &mut hooks,
        [30, 40],
        true,
        false,
    );
    assert!(state.interaction.drag.component.is_none());
    assert!(hooks.events.is_empty());
}
#[test]
fn onop_runs_before_permission_check_and_packet_object_lookup() {
    struct Mutate;
    impl Executor for Mutate {
        fn run(&mut self, _: &mut Store, _: &mut State, r: Request, _: usize) -> Result<()> {
            assert_eq!(r.opindex, 1);
            assert_eq!(r.opbase, Some("Backpack".encode_utf16().collect()));
            let c = r.component.unwrap();
            let mut c = c.borrow_mut();
            c.default_active[0] = 2;
            c.f.invobject = obj::ABYSSAL_WHIP.id();
            Ok(())
        }
    }
    let (mut store, mut state, _, c, _) = scene();
    c.borrow_mut().hooks.insert("onop", vec![Arg::Int(3)]);
    dispatch(
        &mut store,
        &mut state,
        &mut Mutate,
        Action::Op {
            op: 1,
            parent: 65537,
            child: -1,
            base: Some("Backpack".encode_utf16().collect()),
        },
    )
    .unwrap();
    assert_eq!(
        state.interaction.outgoing,
        vec![111, 0xb7, 0x10, 0xff, 0x7f, 0, 1, 0, 1]
    );
}

/// The button dispatch queues `IF_BUTTON<op>` only for ops 1-10, each with
/// the trailer `p2_alt3(invobject)`, `p2_alt2(slot)`, `p4(parent)`. Bank
/// slot 0 of 517
/// holding coins 995: `995+128 = 0x0463`, `0+128 = 0x80`.
#[test]
fn if_buttons_follow_the_recorded_opcodes_and_trailer() {
    let ids = [111u8, 68, 86, 29, 70, 83, 63, 50, 66, 67];
    for (op, id) in (1..=10).zip(ids) {
        assert_eq!(
            button_packet(
                op,
                component::bank_window::SLOTS.packed(),
                0,
                obj::COINS.id()
            ),
            [id, 0x63, 0x03, 0x00, 0x80, 0x02, 0x05, 0x00, 0xB8],
            "op {op}"
        );
    }
    for op in [0, 11, -1] {
        assert!(
            button_packet(
                op,
                component::bank_window::SLOTS.packed(),
                0,
                obj::COINS.id()
            )
            .is_empty(),
            "op {op}"
        );
    }
}

#[test]
fn linked_clan_channel_operation_emits_if_player_packet() {
    let (mut store, mut state, _, c, _) = scene();
    {
        let mut component = c.borrow_mut();
        component.default_active[0] = 2;
        component.f.link = Some("Bob".encode_utf16().collect());
        component.group_kind = Some(3);
    }
    dispatch(
        &mut store,
        &mut state,
        &mut Hooks::default(),
        Action::Op {
            op: 1,
            parent: 65537,
            child: -1,
            base: None,
        },
    )
    .unwrap();
    assert_eq!(
        state.interaction.outgoing,
        vec![104, 12, b'B', b'o', b'b', 0, 255, 255, 255, 125, 0, 1, 0, 1]
    );
}

/// The target-button packet through the live
/// `Action::Target` dispatch: only in target mode; the `onopt` hook runs
/// with the selected component as `drop` before `IF_BUTTONT` (58) is queued
/// as `p4_alt1(parentlayer)` + `p2(invobject)` +
/// `p2_alt2(activeComponentInvobject)` + `p2_alt2(id)` +
/// `p2_alt2(activeComponentId)` + `p4_alt1(activeComponentParentLayer)`.
/// `p4_alt1` is little-endian; `p2_alt2` is hi, `lo + 128`. The original
/// client sends no ctrl byte.
#[test]
fn if_buttont_packet_bytes_follow_the_recorded_order() {
    let (mut store, mut state, _, _, target) = scene();
    {
        let mut b = target.borrow_mut();
        b.f.invobject = 0x1234;
        b.f.id = 0x2345;
        b.hooks.insert("onopt", vec![Arg::Int(9)]);
    }
    let action = Action::Target {
        parent: 65538,
        child: -1,
    };
    let mut hooks = Hooks::default();
    // `targetModeActive` false: nothing runs or is sent.
    dispatch(&mut store, &mut state, &mut hooks, action.clone()).unwrap();
    assert!(state.interaction.outgoing.is_empty() && hooks.events.is_empty());

    let t = &mut state.interaction.target;
    t.active = true;
    t.parent = 65537;
    t.child = -1;
    t.object = 0x5678;
    dispatch(&mut store, &mut state, &mut hooks, action).unwrap();
    assert_eq!(hooks.events, [(9, [0, 0], Some(65537))]);
    let payload = [
        0x02, 0x00, 0x01, 0x00, 0x12, 0x34, 0x56, 0xF8, 0x23, 0xC5, 0xFF, 0x7F, 0x01, 0x00, 0x01,
        0x00,
    ];
    assert_eq!(
        state.interaction.outgoing,
        [&[crate::proto::client::IF_BUTTONT][..], &payload].concat()
    );
    // The menu-side builder the protocol goldens use writes the same bytes.
    assert_eq!(
        crate::ui_player_options::build_if_buttont(
            58,
            &crate::ui_player_options::TargetedButtonUse {
                target_parentlayer: 65538,
                target_invobject: 0x1234,
                target_id: 0x2345,
                active_invobject: 0x5678,
                active_id: -1,
                active_parentlayer: 65537,
            }
        ),
        Some((crate::proto::client::IF_BUTTONT, payload))
    );
}

const LOOK: crate::ui_minimenu::PopupLook = crate::ui_minimenu::PopupLook {
    ascent: 12,
    descent: 4,
    custom: false,
};

#[test]
fn popup_edges_grouped_selection_and_mouse_leave() {
    let mut m = crate::ui_minimenu::MiniMenu::default();
    m.reset();
    for op in 1..=3 {
        m.add_option(crate::ui_minimenu::Entry {
            op: format!("Op {op}"),
            target: Some("Panel".into()),
            cursor: -1,
            action: 57,
            obj_id: -1,
            entity_id: op,
            tile_x: 0,
            tile_z: 65536,
            enabled: true,
            has_arrow: false,
            sub_id: 1,
            force_submenu: false,
            detail: None,
        });
    }
    m.update(false, 200, false);
    m.open_at([198, 199], [200, 200], LOOK, false, 0, |s| {
        s.len() as i32 * 6
    });
    let [x, y, w, h] = m.popup.bounds;
    assert!(x + w <= 200);
    assert_eq!(y, 200 - (4 * 16 + 21));
    assert_eq!(h, 86);
    let row = m.popup_rows(false)[0].clone();
    assert!(m.popup_hit([x + 2, row.baseline - 13], false).is_none());
    assert_eq!(
        m.popup_input([x + 2, row.baseline], true, [200, 200], 0, |s| s.len()
            as i32
            * 6)
            .unwrap()
            .op,
        "Op 3"
    );
    m.update(false, 50, false);
    assert!(m.grouped);
    m.open_at([0, 0], [200, 200], LOOK, false, 0, |s| s.len() as i32 * 6);
    let row = m
        .popup_rows(false)
        .into_iter()
        .find(|r| r.group.is_some())
        .unwrap();
    m.popup_input([-5, row.baseline], false, [200, 200], 0, |s| {
        s.len() as i32 * 6
    });
    assert!(m.popup.expanded.is_some());
    m.popup_input([500, 500], false, [200, 200], 0, |s| s.len() as i32 * 6);
    assert!(!m.open);
}

#[test]
fn server_event_permissions_decode_and_override_dynamic_children() {
    // /read: g2_alt2(from), g4(mask), g2_alt3(to), g4_alt1(parent).
    let bytes = [0, 129, 0, 0, 0, 2, 129, 0, 55, 0, 197, 5];
    let event = crate::server_prot::parse_ui_event(crate::proto::server::IF_SETEVENTS, &bytes)
        .unwrap()
        .unwrap();
    assert_eq!(
        event,
        crate::server_prot::UiEvent::SetEvents {
            packed: component::game_window::LAYOUT_LOCK_BUTTON.packed() as u32,
            from: 1,
            to: 1,
            mask: 2
        }
    );
    let mut state = State::default();
    let mut store = Store::default();
    crate::ui_lifecycle::packet(&mut store, &mut state, &event, &mut Hooks::default()).unwrap();
    let c = Rc::new(RefCell::new(Component::default()));
    c.borrow_mut().f.parentlayer = component::game_window::LAYOUT_LOCK_BUTTON.packed();
    c.borrow_mut().f.id = 1;
    assert_eq!(state.layout.active_mask(&c), 2);
    assert!(
        crate::server_prot::parse_ui_event(crate::proto::server::IF_SETEVENTS, &bytes[..11])
            .is_err()
    );
    let param = [5, 197, 0, 55, 0, 1, 0, 135, 0, 129];
    let event = crate::server_prot::parse_ui_event(crate::proto::server::IF_SETTARGETPARAM, &param)
        .unwrap()
        .unwrap();
    crate::ui_lifecycle::packet(&mut store, &mut state, &event, &mut Hooks::default()).unwrap();
    assert_eq!(
        state.layout.active_mask(&c),
        2,
        "target parameter must preserve operation permissions"
    );
    assert_eq!(
        state.layout.active_params
            [&((i64::from(component::game_window::LAYOUT_LOCK_BUTTON.packed()) << 32) + 1)],
        7
    );
}

#[test]
fn key_operations_obey_modifiers_repeat_deadlines_release_and_console() {
    let (mut store, mut state, mut frame, c, _) = scene();
    {
        let mut c = c.borrow_mut();
        c.f.hasKeybinds = true;
        c.default_active[0] = 2;
        c.hooks.insert("onop", vec![Arg::Int(3)]);
        c.keys = Some(vec![Some(vec![20])]);
        c.key_mods = Some(vec![Some(vec![1])]);
        c.key_delays = Some(vec![3]);
        c.key_rates = Some(vec![2]);
    }
    let mut hooks = Hooks::default();
    let mut step = |cycle: i32, key: bool, ctrl: bool, console: bool| {
        let mut held = vec![false; 112];
        held[20] = key;
        held[82] = ctrl;
        let input = Input {
            cycle,
            held_keys: held,
            console_open: console,
            ..Default::default()
        };
        crate::ui_loop::update_top_level_with(
            &mut store,
            &mut state,
            &mut frame,
            &input,
            &Transmits::default(),
            Some(&mut hooks),
        )
        .unwrap();
        hooks.events.len()
    };
    assert_eq!(step(1, true, false, false), 0);
    assert_eq!(step(2, true, true, false), 1);
    assert_eq!(step(6, true, true, false), 1);
    assert_eq!(step(7, true, true, false), 2);
    assert_eq!(step(10, true, true, true), 2);
    assert_eq!(step(11, false, false, false), 2);
    assert_eq!(step(12, true, true, false), 3);
}

#[test]
fn recorded_drag_reference() -> Result<()> {
    let mut output = String::new();
    for scenario in 0..64 {
        let (mut store, mut state, _, c, target) = scene();
        let root = store.get(65536, -1)?.unwrap();
        root.borrow_mut().f.scrollx = 7;
        root.borrow_mut().f.scrolly = 11;
        {
            let mut c = c.borrow_mut();
            c.f.width = 20 + scenario % 5;
            c.f.height = 10 + scenario % 3;
            c.f.dragdeadtime = scenario % 4;
            c.f.dragdeadzone = scenario % 7;
            c.default_active[0] = if scenario % 2 == 0 { 1 << 23 } else { 0 };
        }
        pickup(&mut store, &mut state, Some(c), [5, 5])?;
        let mut hooks = Hooks::default();
        for (step, mouse) in [[15, 25], [20, 25], [26, 40], [-30, 900], [110, 60]]
            .into_iter()
            .enumerate()
        {
            state.interaction.drag.mouse = mouse;
            state.interaction.drag_ready = !(scenario % 5 == 0 && step == 3);
            state.interaction.drag.parent_ready = true;
            state.interaction.drag_origin = [10, 20];
            state.interaction.drag.bounds = [3, 4, 100, 70];
            state.interaction.drop_target =
                (step == 4 && scenario & 4 != 0).then(|| target.clone());
            finish_drag(&mut store, &mut state, &mut hooks, step < 4)?;
            let events = hooks
                .events
                .iter()
                .map(|(id, xy, drop)| format!("{id}:{}:{}:{}", xy[0], xy[1], drop.unwrap_or(-1)))
                .collect::<Vec<_>>()
                .join("|");
            output.push_str(&format!(
                "{scenario},{step},{},{},{},{},{events}\n",
                state.interaction.drag.component.is_some() as i32,
                state.interaction.drag.active as i32,
                state.interaction.drag_cycle,
                state.interaction.outgoing.len() / 17
            ));
        }
    }
    rs910_core::test_support::frozen::assert_stream("ui-interaction/recorded", output.as_bytes());
    Ok(())
}

/// A real cache component whose version (-1) predates the clickmask byte
/// (read only when `version >= 3`) keeps the field default
/// `clickmask = true`, so the interaction walk hit-tests it through its
/// graphic mask: the
/// transparent margin left of a sprite row's first opaque pixel is NOT
/// hovered, while the opaque pixel is. Likewise a version -1 text keeps
/// `fontmono = true` (read only when `version >= 2`).
/// The expected pixels come from the independent native910 sprite decoder
/// (checked against the recording), not from the client's mask code.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn old_version_graphic_keeps_the_clickmask_default_and_hit_tests_its_mask() {
    let pack = crate::test_support::require_pack("client.interfaces.js5");
    // 101:3, the "Close" button: graphic 537, 26x23, onmouseover hook.
    let bytes = pack
        .read_group("interfaces", interface::FOUND_DOCUMENT.id() as u32)
        .unwrap()
        .remove(&(component::found_document::PAGE.child() as u32))
        .unwrap();
    assert_eq!(
        bytes[0], 0xFF,
        "101:3 is a version -1 (unversioned) component"
    );
    let decoded = Component::decode(component::found_document::PAGE.packed(), &bytes).unwrap();
    assert_eq!(decoded.f.r#type, 5);
    assert!(decoded.f.clickmask, "absent clickmask keeps the default");
    assert!(decoded.f.hashook);
    // 868:55: version -1 text.
    let text = pack
        .read_group("interfaces", interface::JOURNAL_SCROLL.id() as u32)
        .unwrap()
        .remove(&(component::journal_scroll::LINE_46.child() as u32))
        .unwrap();
    assert_eq!(text[0], 0xFF);
    let text = Component::decode(component::journal_scroll::LINE_46.packed(), &text).unwrap();
    assert_eq!(text.f.r#type, 4);
    assert!(text.f.fontmono, "absent fontmono keeps the default");

    // Sprite 537 as the mask reads it: a pixel is opaque when its palette
    // colour is nonzero (`palette[colour[...]]`, palette index 0 = 0).
    let raw = pack.read_group("sprites", 537).unwrap().remove(&0).unwrap();
    let native910::sprite::SpriteSheet::Paletted(sheet) =
        native910::sprite::decode_sprite(&raw).unwrap()
    else {
        panic!("sprite 537 is paletted");
    };
    let sprite = &sheet.sprites[0];
    assert_eq!(
        (
            i32::from(sheet.canvas_width),
            i32::from(sheet.canvas_height)
        ),
        (decoded.f.wsize, decoded.f.hsize),
        "mask size equals the component size, so the mask applies"
    );
    let (w, h) = (usize::from(sprite.width), usize::from(sprite.height));
    // A row whose first opaque pixel is right of the canvas edge.
    let (row, first) = (0..h)
        .find_map(|y| {
            let first = (0..w).find(|&x| sprite.colour[y * w + x] != 0)?;
            (first + usize::from(sprite.padding_left) > 0).then_some((y, first))
        })
        .expect("a row with a transparent left margin");
    let canvas_y = (row + usize::from(sprite.padding_top)) as i32;
    let opaque_x = (first + usize::from(sprite.padding_left)) as i32;

    let root = Rc::new(RefCell::new(Component::default()));
    {
        let f = &mut root.borrow_mut().f;
        f.parentlayer = interface::FOUND_DOCUMENT.id() << 16;
        f.layer = -1;
        f.width = 500;
        f.height = 300;
    }
    let c = Rc::new(RefCell::new(decoded));
    {
        let f = &mut c.borrow_mut().f;
        f.layer = interface::FOUND_DOCUMENT.id() << 16;
        f.x = 10;
        f.y = 20;
        f.width = f.wsize;
        f.height = f.hsize;
    }
    let mut store = Store::default();
    store.interfaces.insert(
        101,
        Interface::new(vec![Some(root), None, None, Some(c.clone())]),
    );
    let mut state = State::default();
    state.life.top = 101;
    state.layout.canvas = [500, 300];
    state.sprites = Some(crate::ui_sprites::Resources::from_pack(pack.clone()).unwrap());
    let mut frame = Frame::default();
    let mut hooks = Hooks::default();
    let hovered_at =
        |x: i32, store: &mut Store, state: &mut State, frame: &mut Frame, hooks: &mut Hooks| {
            c.borrow_mut().f.hovered = false;
            cycle(
                store,
                state,
                frame,
                hooks,
                [10 + x, 20 + canvas_y],
                false,
                false,
            );
            c.borrow().f.hovered
        };
    // Inside the component rectangle but left of the row's mask span.
    assert!(
        !hovered_at(opaque_x - 1, &mut store, &mut state, &mut frame, &mut hooks),
        "transparent margin at canvas ({}, {canvas_y}) must miss like the original",
        opaque_x - 1
    );
    assert!(
        hovered_at(opaque_x, &mut store, &mut state, &mut frame, &mut hooks),
        "first opaque pixel ({opaque_x}, {canvas_y}) hits"
    );
}
