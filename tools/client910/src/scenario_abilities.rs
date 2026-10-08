//! Native bar selection, book drag and keyboard activation through a recorded
//! ordinary client session. Existing basic rules run unchanged; the recording
//! target has extra life and fixed rolls to keep one target through each style.
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use super::*;
use rs910_protocol::server_prot::{ScriptArg, UiEvent};
use rs910_symbols::{component, enums, interface, param, script, structs, varbit, varp};
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/abilities";
const FIRST_CYCLE: i32 = 1;
const NO_CHILD: i32 = -1;
const NO_GRAPHIC: i32 = -1;
const NO_ACTION: i32 = 0;
const FIRST_BAR: i32 = 1;
const SECOND_BAR: i32 = 2;
const BOOK_MAGIC: i32 = 6;
const ACTION_BOOK_BITS: i32 = 4;
const INITIAL_UI_READY: i32 = 180;
const DRAG_PRESS_CYCLE: i32 = 440;
const DRAG_TARGET_CYCLE: i32 = 454;
const DRAG_REPLY_READY: i32 = 520;
const BOOK_RESTORED_READY: i32 = 610;
const MAGIC_KEY_CYCLE: i32 = 980;
const FIRST_BAR_READY: i32 = 1120;
const MELEE_KEY_CYCLE: i32 = 1330;
const RANGED_KEY_CYCLE: i32 = 1780;
const KEY_PACKET_TAIL: i32 = 3;
const FINAL_CLOSED_READY: i32 = 2520;
const TAB_READY: [(i32, i32); 6] = [
    (320, 3),
    (2020, 3),
    (2120, 1),
    (2200, 2),
    (2280, 4),
    (2360, 5),
];
const NATIVE_DIGIT_ONE: i8 = 16;
const NATIVE_DIGIT_TWO: i8 = 17;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const LOW_BYTE: usize = 0;
const SECOND_BYTE: usize = 1;
const THIRD_BYTE: usize = 2;
const HIGH_BYTE: usize = 3;
const EMPTY_WIRE_SLOT: u16 = u16::MAX;
const LOW_BYTE_ADDEND: u8 = 128;
const PACKED_GROUP_BITS: u32 = 16;
const OVERLAY_SUB: i32 = 1;
const LAST_RECORDED_CYCLE: i32 = 2529;

fn gameplay(bytes: &[u8], pings: &mut Vec<Vec<u8>>) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter(|(opcode, payload)| {
            if *opcode == crate::proto::client::PING_STATISTICS {
                pings.push(payload.clone());
                false
            } else {
                *opcode != crate::proto::client::NO_TIMEOUT
            }
        })
        .collect())
}

/// Recorded protocol ordering, independent of the drag packet builder.
fn expected_drag(action: i32) -> Vec<u8> {
    let mut payload = EMPTY_WIRE_SLOT.to_be_bytes().to_vec();
    let mut packed = |value: i32| {
        let bytes = value.to_le_bytes();
        payload.extend([
            bytes[THIRD_BYTE],
            bytes[HIGH_BYTE],
            bytes[LOW_BYTE],
            bytes[SECOND_BYTE],
        ]);
    };
    packed(component::magic_book::ACTIONS.packed());
    let empty = EMPTY_WIRE_SLOT.to_le_bytes();
    payload.extend(empty);
    payload.extend([
        empty[LOW_BYTE].wrapping_add(LOW_BYTE_ADDEND),
        empty[SECOND_BYTE],
    ]);
    let bytes = component::action_bar_setup::SLOT_1.packed().to_le_bytes();
    payload.extend([
        bytes[THIRD_BYTE],
        bytes[HIGH_BYTE],
        bytes[LOW_BYTE],
        bytes[SECOND_BYTE],
    ]);
    payload.extend((action as u16).to_le_bytes());
    payload
}

fn bar_value(replay: &Replay, variable: i32) -> anyhow::Result<i32> {
    replay
        .game()
        .runtime
        .feed
        .state
        .varps
        .as_ref()
        .context("player variables")?
        .get(variable)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
}

fn selected(replay: &Replay) -> anyhow::Result<i32> {
    replay
        .game()
        .varbit_value(varbit::SELECTED_ACTION_BAR.id() as u16)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
}

/// A sub-interface is reachable only while its parent group is in the rooted graph.
fn reachable(replay: &Replay, group: i32) -> bool {
    let ui = replay.ui();
    let mut groups = BTreeSet::from([ui.state.life.top]);
    loop {
        let previous = groups.len();
        for &(parent, child) in &ui.state.layout.subs {
            if groups.contains(&((parent as u32 >> PACKED_GROUP_BITS) as i32)) {
                groups.insert(child);
            }
        }
        if groups.len() == previous {
            return groups.contains(&group);
        }
    }
}

fn icon(replay: &mut Replay, packed: i32, name: &str, key: i8) -> anyhow::Result<()> {
    let ui = &mut replay.core.session.as_mut().unwrap().ui;
    let icon = ui.store.get(packed, NO_CHILD)?.context("native bar slot")?;
    let icon = icon.borrow();
    assert_ne!(icon.f.graphic, NO_GRAPHIC, "{name} graphic");
    let label = icon
        .f
        .opbase
        .as_ref()
        .map(|text| String::from_utf16_lossy(text))
        .unwrap_or_default();
    assert!(label.contains(name), "{name} label: {label}");
    assert!(icon.f.hasKeybinds, "{name} native key table");
    assert!(
        icon.keys
            .as_ref()
            .is_some_and(|keys| keys.iter().flatten().any(|keys| keys.contains(&key))),
        "{name} key {key}"
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_ability_bar_session_drags_native_book_switches_layouts_and_activates_keys(
) -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    assert_eq!(
        trace.last_cycle(),
        LAST_RECORDED_CYCLE,
        "qualified capture cutoff"
    );
    let mut replay = Replay::start(&trace)?;
    let cache = &replay.ui().state.configs;
    let action = cache
        .structure(structs::WRACK_ABILITY.id())?
        .integer(param::COMBAT_ACTION_CODE.id(), NO_ACTION)?;
    assert_eq!(
        cache
            .enumeration(enums::MAGIC_ACTIONS.id())
            .integer(action)?,
        structs::WRACK_ABILITY.id()
    );
    const ABSENT_IMPACT_SPOT: i32 = -1;
    let expected_impact = cache
        .structure(structs::WRACK_ABILITY.id())?
        .integer(param::SPELL_IMPACT_SPOT.id(), ABSENT_IMPACT_SPOT)?;
    anyhow::ensure!(
        expected_impact > ABSENT_IMPACT_SPOT,
        "native Wrack impact effect is absent"
    );
    let mut received_impact = false;
    let expected_shortcut = (action << ACTION_BOOK_BITS) | BOOK_MAGIC;
    let drag = expected_drag(action);
    let mut drags = Vec::new();
    let mut native_keys = BTreeSet::new();
    let mut replay_pings = Vec::new();
    let mut recorded_pings = Vec::new();
    let mut adrenaline = false;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: gameplay packets"
        );
        assert_eq!(replay.game().cycle, cycle);
        for (opcode, payload) in client_frames(&out.written)? {
            if opcode == crate::proto::client::IF_BUTTOND {
                drags.push(payload);
            } else if opcode == crate::proto::client::IF_BUTTON1 {
                let packed =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                for (at, parent) in [
                    (MAGIC_KEY_CYCLE, component::action_bar::SLICE_SLOT.packed()),
                    (MELEE_KEY_CYCLE, component::action_bar::SLICE_SLOT.packed()),
                    (
                        RANGED_KEY_CYCLE,
                        component::action_bar::PIERCING_SHOT_SLOT.packed(),
                    ),
                ] {
                    if (at..=at + KEY_PACKET_TAIL).contains(&cycle) && packed == parent {
                        native_keys.insert(at);
                    }
                }
            }
        }
        if cycle == INITIAL_UI_READY {
            icon(
                &mut replay,
                component::action_bar::SLICE_SLOT.packed(),
                "Slice",
                NATIVE_DIGIT_ONE,
            )?;
            icon(
                &mut replay,
                component::action_bar::PIERCING_SHOT_SLOT.packed(),
                "Piercing Shot",
                NATIVE_DIGIT_TWO,
            )?;
        }
        if cycle == DRAG_PRESS_CYCLE {
            let picked = replay
                .ui()
                .state
                .interaction
                .drag
                .component
                .as_ref()
                .context("native pointer pickup")?
                .borrow();
            assert_eq!(
                (picked.f.parentlayer, picked.f.id),
                (component::magic_book::ACTIONS.packed(), action)
            );
            assert!(picked.hooks.contains_key("ondragcomplete"));
        }
        if cycle == DRAG_TARGET_CYCLE {
            assert!(
                replay.ui().state.interaction.drag.active,
                "native drag threshold"
            );
            let target = replay
                .ui()
                .state
                .interaction
                .drop_target
                .as_ref()
                .context("native Setup drop target")?
                .borrow();
            assert_eq!(
                (target.f.parentlayer, target.f.id),
                (component::action_bar_setup::SLOT_1.packed(), NO_CHILD)
            );
        }
        if cycle == DRAG_REPLY_READY {
            assert_eq!(selected(&replay)?, SECOND_BAR);
            assert_eq!(
                bar_value(&replay, varp::ACTION_BAR_2_SLOT_1_ACTION.id())?,
                expected_shortcut
            );
            assert_eq!(
                bar_value(&replay, varp::ACTION_BAR_2_SLOT_1_ITEM.id())?,
                NO_CHILD
            );
        }
        if cycle == BOOK_RESTORED_READY {
            assert!(!reachable(&replay, interface::ACTION_BAR_SETUP.id()));
            assert!(
                replay
                    .ui()
                    .state
                    .life
                    .subs
                    .get(component::game_window::MODAL_FRAME_SLOT.packed())
                    .is_none(),
                "the native close packet unlinks the root"
            );
            let retained = replay
                .ui()
                .state
                .life
                .subs
                .get(component::hero_window::FOURTH_PANE.packed())
                .context("registered overlay pane")?;
            assert_eq!(
                (retained.borrow().kind, retained.borrow().id),
                (OVERLAY_SUB, interface::ACTION_BAR_SETUP.id()),
                "kind-one panes remain registered after root unlink"
            );
            assert!(reachable(&replay, interface::MAGIC_BOOK.id()));
            assert!(replay.ui().state.layout.subs.contains(&(
                component::game_window::MAGIC_SLOT.packed(),
                interface::MAGIC_BOOK.id()
            )));
            icon(
                &mut replay,
                component::action_bar::SLICE_SLOT.packed(),
                "Wrack",
                NATIVE_DIGIT_ONE,
            )?;
        }
        if cycle == FIRST_BAR_READY {
            assert_eq!(selected(&replay)?, FIRST_BAR);
            icon(
                &mut replay,
                component::action_bar::SLICE_SLOT.packed(),
                "Slice",
                NATIVE_DIGIT_ONE,
            )?;
            icon(
                &mut replay,
                component::action_bar::PIERCING_SHOT_SLOT.packed(),
                "Piercing Shot",
                NATIVE_DIGIT_TWO,
            )?;
        }
        for &(at, tab) in &TAB_READY {
            if cycle == at {
                assert_eq!(
                    replay
                        .game()
                        .varbit_value(varbit::POWERS_WINDOW_TAB.id() as u16)
                        .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                    tab,
                    "native Powers tab at {cycle}"
                );
                assert!(reachable(&replay, interface::ACTION_BAR_SETUP.id()));
            }
        }
        if cycle == FINAL_CLOSED_READY {
            assert!(
                !reachable(&replay, interface::HERO_WINDOW.id()),
                "Escape closes the ordinary Hero modal"
            );
            assert!(
                !reachable(&replay, interface::ACTION_BAR_SETUP.id()),
                "Setup is no longer rooted"
            );
            assert!(
                replay.ui().state.layout.subs.contains(&(
                    component::game_window::MAGIC_SLOT.packed(),
                    interface::MAGIC_BOOK.id()
                )),
                "the docked Magic owner is restored"
            );
        }
        if (MAGIC_KEY_CYCLE..MELEE_KEY_CYCLE).contains(&cycle) {
            received_impact |= replay
                .game()
                .runtime
                .feed
                .state
                .npcs
                .entities
                .values()
                .filter(|entity| entity.type_id == rs910_symbols::npc::CHICKEN.id())
                .any(|entity| {
                    entity
                        .path
                        .animation
                        .spots
                        .iter()
                        .any(|spot| spot.id == expected_impact)
                });
        }
        adrenaline |= bar_value(&replay, varp::ADRENALINE_FINE.id())? > NO_ACTION;
    }
    assert!(
        received_impact,
        "cache-qualified Wrack impact did not reach the actual recorded NPC spot owner"
    );
    assert!(
        replay.ui().diagnostics.errors.is_empty(),
        "native session hooks: {:?}",
        replay.ui().diagnostics.errors
    );
    assert_eq!(replay_pings, recorded_pings, "ordered ping reports");
    assert_eq!(drags, vec![drag], "exact native IF_BUTTOND payload");
    assert_eq!(
        native_keys,
        BTreeSet::from([MAGIC_KEY_CYCLE, MELEE_KEY_CYCLE, RANGED_KEY_CYCLE])
    );
    assert!(
        adrenaline,
        "basic/auto attack adrenaline must reach the ordinary variable owner"
    );
    let mut cooldowns = BTreeSet::new();
    for frame in arrivals(&trace)? {
        if let Some(UiEvent::RunScript(call)) =
            crate::session::parse_ui_event(frame.opcode, &frame.payload)?
        {
            if call.script_id == script::ABILITY_COOLDOWN_SCHEDULE.id() {
                if let Some(ScriptArg::Int(definition)) = call.args.first() {
                    cooldowns.insert(*definition);
                }
            }
        }
    }
    for definition in [
        structs::SLICE_ABILITY,
        structs::PIERCING_SHOT_ABILITY,
        structs::WRACK_ABILITY,
        structs::GLOBAL_ABILITY_COOLDOWN,
    ] {
        assert!(
            cooldowns.contains(&definition.id()),
            "native cooldown for {definition:?}"
        );
    }
    assert_eq!(
        bar_value(&replay, varp::ACTION_BAR_2_SLOT_1_ACTION.id())?,
        expected_shortcut,
        "edited layout retained after later tab/close transitions"
    );
    Ok(())
}

#[derive(Debug)]
struct VisibleStatus {
    node: i32,
    graphic: i32,
    text: String,
    active: bool,
}

fn status_component_visible(
    replay: &mut Replay,
    first: rs910_ui::ui_components::Ref,
) -> anyhow::Result<bool> {
    const MAX_VISIBLE_ANCESTORS: usize = 256;
    let ui = &mut replay.core.session.as_mut().context("session")?.ui;
    let mut component = first;
    let mut seen = BTreeSet::new();
    for _ in 0..MAX_VISIBLE_ANCESTORS {
        anyhow::ensure!(
            seen.insert(std::rc::Rc::as_ptr(&component) as usize),
            "native status ancestry has a cycle"
        );
        if !rs910_ui::ui_hooks::attached(&mut ui.store, &component)? {
            return Ok(false);
        }
        let state = component.borrow();
        if state.runtime_entry_hidden().unwrap_or(state.f.hide) {
            return Ok(false);
        }
        let runtime_parent = state.runtime_parent();
        let parent = state.f.layer;
        let group = (state.f.parentlayer as u32 >> PACKED_GROUP_BITS) as i32;
        drop(state);
        if let Some(parent) = runtime_parent {
            component = parent;
        } else if parent != NO_CHILD {
            let Some(parent) = ui.store.get(parent, NO_CHILD)? else {
                return Ok(false);
            };
            component = parent;
        } else {
            if group == ui.state.life.top {
                return Ok(true);
            }
            let Some(&(parent, _)) = ui
                .state
                .layout
                .subs
                .iter()
                .find(|&&(_, child)| child == group)
            else {
                return Ok(false);
            };
            let Some(parent) = ui.store.get(parent, NO_CHILD)? else {
                return Ok(false);
            };
            component = parent;
        }
    }
    anyhow::bail!("native status ancestry exceeds its recorded bound")
}

fn beneficial_status(
    replay: &mut Replay,
    definition: i32,
) -> anyhow::Result<Option<VisibleStatus>> {
    const MAX_NATIVE_STATUS_LINKS: usize = 50;
    const GRAPHIC_CHILD: i32 = 0;
    const TEXT_CHILD: i32 = 1;
    const NATIVE_ACTIVE: i32 = 1;
    const NATIVE_INACTIVE: i32 = 0;
    const NO_EXTENT: i32 = 0;
    let ui = &mut replay.core.session.as_mut().context("session")?.ui;
    let root = ui
        .store
        .get(component::status_buffs::ICONS.packed(), NO_CHILD)?
        .context("beneficial status owner")?;
    let mut link = root
        .borrow()
        .param_int(param::STATUS_LIST_HEAD.id(), NO_CHILD)?;
    let mut seen = BTreeSet::new();
    for _ in 0..MAX_NATIVE_STATUS_LINKS {
        if link == NO_CHILD {
            return Ok(None);
        }
        anyhow::ensure!(seen.insert(link), "native status list has a cycle");
        let node_ref = ui
            .store
            .get(link, NO_CHILD)?
            .context("native linked status node")?;
        let node = node_ref.borrow();
        if node.param_int(param::STATUS_ENTRY_DEFINITION.id(), NO_CHILD)? == definition {
            let active = node
                .param_int(param::STATUS_ENTRY_ACTIVE_VISIBLE.id(), NATIVE_INACTIVE)?
                == NATIVE_ACTIVE;
            drop(node);
            let graphic_ref = ui.store.get(link, GRAPHIC_CHILD)?;
            let text_ref = ui.store.get(link, TEXT_CHILD)?;
            let (Some(graphic_ref), Some(text_ref)) = (graphic_ref, text_ref) else {
                // Linking precedes the throttled callback that creates children.
                // A linked unmaterialized node is present, not expired.
                return Ok(Some(VisibleStatus {
                    node: link,
                    graphic: NO_GRAPHIC,
                    text: String::new(),
                    active: false,
                }));
            };
            let graphic_state = graphic_ref.borrow();
            let graphic = graphic_state.f.graphic;
            let graphic_sized =
                graphic_state.f.width > NO_EXTENT && graphic_state.f.height > NO_EXTENT;
            drop(graphic_state);
            let text_state = text_ref.borrow();
            let text = text_state
                .f
                .text
                .as_ref()
                .map(|value| String::from_utf16_lossy(value))
                .unwrap_or_default();
            let text_sized = text_state.f.width > NO_EXTENT && text_state.f.height > NO_EXTENT;
            drop(text_state);
            return Ok(Some(VisibleStatus {
                node: link,
                graphic,
                text,
                active: active
                    && graphic_sized
                    && text_sized
                    && status_component_visible(replay, node_ref)?
                    && status_component_visible(replay, graphic_ref)?
                    && status_component_visible(replay, text_ref)?,
            }));
        }
        link = node.param_int(param::STATUS_LIST_NEXT.id(), NO_CHILD)?;
    }
    anyhow::bail!("native status list exceeds its recorded native bound")
}

fn expected_anticipation_graphic(replay: &Replay) -> anyhow::Result<i32> {
    const DISPLAY_ENABLED: i32 = 1;
    const DISPLAY_DISABLED: i32 = 0;
    let definition = replay
        .ui()
        .state
        .configs
        .structure(structs::ANTICIPATION_ABILITY.id())?;
    for (key, expected) in [
        (param::STATUS_DISPLAY_END_TIME, DISPLAY_ENABLED),
        (param::STATUS_DISPLAY_CUSTOM_TEXT, DISPLAY_DISABLED),
        (param::STATUS_DISPLAY_STACKS, DISPLAY_DISABLED),
        (param::STATUS_DISPLAY_ICON_OVERRIDE, DISPLAY_DISABLED),
        (param::STATUS_DISPLAY_ITEM_OVERRIDE, DISPLAY_DISABLED),
    ] {
        anyhow::ensure!(
            definition.integer(key.id(), DISPLAY_DISABLED)? == expected,
            "Anticipation display mode changed: {key:?}"
        );
    }
    let graphic = definition.integer(param::STATUS_DEFINITION_GRAPHIC.id(), NO_GRAPHIC)?;
    anyhow::ensure!(
        graphic != NO_GRAPHIC,
        "Anticipation has no native status graphic"
    );
    Ok(graphic)
}

fn native_status_seconds(text: &str) -> anyhow::Result<i32> {
    const FIRST_POSITIVE_SECOND: i32 = 1;
    const LAST_DECIMAL_SECOND: i32 = 60;
    anyhow::ensure!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "native decimal seconds format: {text:?}"
    );
    let seconds: i32 = text.parse().context("native decimal seconds")?;
    anyhow::ensure!(
        (FIRST_POSITIVE_SECOND..=LAST_DECIMAL_SECOND).contains(&seconds)
            && seconds.to_string() == text,
        "native positive canonical seconds: {text:?}"
    );
    Ok(seconds)
}

fn stored_native_clock(replay: &Replay, key: i32) -> anyhow::Result<i32> {
    match replay.game().ui_variables.client.values.get(&key) {
        Some(rs910_ui::ui_vars::Value::Int(value)) => Ok(*value),
        value => anyhow::bail!("native clock {key} was not stored as an integer: {value:?}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeBuffClockReceipt {
    launch: i32,
    until: i32,
}

#[derive(Debug)]
struct NativeBuffCountdownObservation {
    cycle: i32,
    node: i32,
    seconds: i32,
    clocks: NativeBuffClockReceipt,
}

#[derive(Default, Debug)]
struct NativeBuffCountdown {
    observations: Vec<NativeBuffCountdownObservation>,
    enable_cycle: Option<i32>,
    disable_cycle: Option<i32>,
    duration_arguments: Option<(i32, i32)>,
    first_absent_cycle: Option<i32>,
    world_receipt: Option<(i64, i64, i64, i64)>, // generation, applied, until, original life
    last_world_tick: Option<i64>,
    natural_world_expiry: Option<i64>,
}

impl NativeBuffCountdown {
    // World expiry and native display clocks are independent clock domains.
    fn world_publication(&mut self, row: &serde_json::Value) -> anyhow::Result<()> {
        const ONE_WORLD_TICK: i64 = 1;
        let tick = row["tick"].as_i64().context("world publication tick")?;
        let life = row["life"].as_i64().context("player life receipt")?;
        if let Some(previous) = self.last_world_tick.replace(tick) {
            anyhow::ensure!(
                tick == previous + ONE_WORLD_TICK,
                "every consecutive world publication must be retained"
            );
        }
        let active = &row["anticipation"];
        if !active.is_null() {
            anyhow::ensure!(
                self.natural_world_expiry.is_none(),
                "world condition reappeared after expiry"
            );
            let receipt = (
                active["generation"]
                    .as_i64()
                    .context("condition generation")?,
                active["appliedTick"]
                    .as_i64()
                    .context("condition application tick")?,
                active["untilTick"]
                    .as_i64()
                    .context("condition expiry tick")?,
                life,
            );
            let (_, applied, until, _) = receipt;
            anyhow::ensure!(
                applied <= tick && tick < until,
                "active world condition lies outside its receipt"
            );
            if let Some(previous) = self.world_receipt.replace(receipt) {
                anyhow::ensure!(
                    receipt == previous,
                    "condition replacement/refresh/life change is not natural expiry proof"
                );
            }
        } else if let Some((_, _, until, original_life)) = self.world_receipt {
            anyhow::ensure!(
                life == original_life,
                "player life changed before natural expiry proof"
            );
            if self.natural_world_expiry.is_none() {
                anyhow::ensure!(
                    tick == until,
                    "world condition removed early or a publication was skipped"
                );
                self.natural_world_expiry = Some(tick);
            }
        }
        Ok(())
    }

    fn applied_packet(&mut self, opcode: u8, payload: &[u8], cycle: i32) -> anyhow::Result<()> {
        const DEFINITION_ARGUMENT: usize = 0;
        const ENABLE_ARGUMENT: usize = 1;
        const DURATION_START_ARGUMENT: usize = 1;
        const DURATION_END_ARGUMENT: usize = 2;
        const NATIVE_ENABLED: i32 = 1;
        const NATIVE_DISABLED: i32 = 0;
        let Some(UiEvent::RunScript(call)) = crate::session::parse_ui_event(opcode, payload)?
        else {
            return Ok(());
        };
        let Some(ScriptArg::Int(definition)) = call.args.get(DEFINITION_ARGUMENT) else {
            return Ok(());
        };
        if *definition != structs::ANTICIPATION_ABILITY.id() {
            return Ok(());
        }
        if call.script_id == script::STATUS_ICON_TOGGLE.id() {
            let Some(ScriptArg::Int(enabled)) = call.args.get(ENABLE_ARGUMENT) else {
                anyhow::bail!("native status toggle argument");
            };
            match *enabled {
                NATIVE_ENABLED => {
                    anyhow::ensure!(
                        self.enable_cycle.replace(cycle).is_none(),
                        "fixture must contain one Anticipation activation"
                    );
                }
                NATIVE_DISABLED if self.enable_cycle.is_some() => {
                    anyhow::ensure!(
                        self.disable_cycle.replace(cycle).is_none(),
                        "fixture must contain one Anticipation removal after activation"
                    );
                }
                NATIVE_DISABLED => {} // Startup synchronization before the activation is separate.
                value => anyhow::bail!("native status toggle mode {value}"),
            }
        } else if call.script_id == script::LAUNCH_EFFECT_DURATION_SCHEDULE.id() {
            let (Some(ScriptArg::Int(start)), Some(ScriptArg::Int(end))) = (
                call.args.get(DURATION_START_ARGUMENT),
                call.args.get(DURATION_END_ARGUMENT),
            ) else {
                anyhow::bail!("native launch-relative duration arguments");
            };
            anyhow::ensure!(
                self.duration_arguments.replace((*start, *end)).is_none(),
                "fixture must contain one Anticipation duration receipt"
            );
        }
        Ok(())
    }

    fn observe(&mut self, replay: &mut Replay, expected_graphic: i32) -> anyhow::Result<()> {
        const NATIVE_CLIENT_CYCLES_PER_SECOND: i32 = 50;
        const POSITIVE_SECOND_BIAS: i32 = 1;
        let cycle = replay.game().cycle;
        let status = beneficial_status(replay, structs::ANTICIPATION_ABILITY.id())?;
        if let Some(status) = status {
            anyhow::ensure!(
                self.first_absent_cycle.is_none(),
                "removed Anticipation definition reappeared in the single-activation fixture"
            );
            if !status.active {
                return Ok(());
            }
            anyhow::ensure!(
                status.graphic == expected_graphic,
                "cycle {cycle}: rooted native Anticipation graphic: {status:?}"
            );
            if status.text.is_empty() {
                return Ok(());
            } // Native blank is not a numeric zero or proof of removal.
            let seconds = native_status_seconds(&status.text)?;
            let clocks = NativeBuffClockReceipt {
                launch: stored_native_clock(
                    replay,
                    rs910_symbols::varc::ANTICIPATION_LAUNCH_CLOCK.id(),
                )?,
                until: stored_native_clock(
                    replay,
                    rs910_symbols::varc::ANTICIPATION_EFFECT_UNTIL.id(),
                )?,
            };
            anyhow::ensure!(
                clocks.until > clocks.launch && cycle >= clocks.launch,
                "specific native launch/effect clocks: {clocks:?} at {cycle}"
            );
            let maximum_initial_seconds = POSITIVE_SECOND_BIAS
                + (clocks.until - clocks.launch) / NATIVE_CLIENT_CYCLES_PER_SECOND;
            anyhow::ensure!(
                seconds <= maximum_initial_seconds,
                "native timer exceeds its captured launch-relative duration"
            );
            if let Some(previous) = self.observations.last() {
                anyhow::ensure!(
                    clocks == previous.clocks && status.node == previous.node,
                    "native entry/clocks changed during the single activation"
                );
                anyhow::ensure!(
                    seconds <= previous.seconds,
                    "cycle {cycle}: native timer increased from {} to {seconds}",
                    previous.seconds
                );
            }
            self.observations.push(NativeBuffCountdownObservation {
                cycle,
                node: status.node,
                seconds,
                clocks,
            });
        } else if !self.observations.is_empty() {
            // Server removal follows its own world receipt, even if a loaded
            // client has not counted up to the displayed client-cycle deadline.
            self.first_absent_cycle.get_or_insert(cycle);
        }
        Ok(())
    }

    fn finish(&self) -> anyhow::Result<()> {
        const ADJACENT_OBSERVATIONS: usize = 2;
        const MINIMUM_DISTINCT_POSITIVE_VALUES: usize = 3;
        const MINIMUM_STRICT_DROPS: usize = 2;
        const NATIVE_CLIENT_CYCLES_PER_DURATION_TICK: i32 = 30;
        let first = self
            .observations
            .first()
            .context("no rooted visible native positive countdown")?;
        let last = self
            .observations
            .last()
            .context("no final positive countdown")?;
        let distinct = self
            .observations
            .iter()
            .map(|observation| observation.seconds)
            .collect::<BTreeSet<_>>();
        let drops = self
            .observations
            .windows(ADJACENT_OBSERVATIONS)
            .filter(|pair| pair.first().unwrap().seconds > pair.last().unwrap().seconds)
            .count();
        anyhow::ensure!(
            distinct.len() >= MINIMUM_DISTINCT_POSITIVE_VALUES && drops >= MINIMUM_STRICT_DROPS,
            "native countdown did not numerically decrease enough: {self:?}"
        );
        let enabled = self
            .enable_cycle
            .context("no applied native status enable packet")?;
        let disabled = self
            .disable_cycle
            .context("no applied native status removal packet")?;
        let absent = self
            .first_absent_cycle
            .context("Anticipation did not leave its linked status list")?;
        anyhow::ensure!(
            first.cycle >= enabled && last.cycle < absent && absent >= disabled,
            "native enable/countdown/removal ordering: {self:?}"
        );
        let (start, end) = self
            .duration_arguments
            .context("no applied native launch-relative duration packet")?;
        let duration = end
            .checked_sub(start)
            .context("native duration subtraction overflow")?;
        anyhow::ensure!(
            duration > NO_ACTION,
            "native effect duration must be positive"
        );
        let (_, applied, world_until, _) = self
            .world_receipt
            .context("no independent world condition receipt")?;
        anyhow::ensure!(
            self.natural_world_expiry == Some(world_until),
            "no independent natural world expiry publication"
        );
        anyhow::ensure!(
            i64::from(duration) == world_until - applied,
            "native duration must match the exact world condition receipt"
        );
        let expected_until = first
            .clocks
            .launch
            .checked_add(
                duration
                    .checked_mul(NATIVE_CLIENT_CYCLES_PER_DURATION_TICK)
                    .context("native duration multiplication overflow")?,
            )
            .context("native effect clock addition overflow")?;
        anyhow::ensure!(
            first.clocks.until == expected_until,
            "actual native effect clock must match its particular launch-relative script receipt"
        );
        Ok(())
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_ability_gameplay_spends_adrenaline_stuns_and_shows_native_buffs() -> anyhow::Result<()>
{
    use super::session_replay::{observe, server_trace_file};
    use rs910_symbols::npc;
    const GAMEPLAY_FIXTURE: &str = "fixtures/session-replay/ability-gameplay";
    const FIRST_UPDATE_CURSOR: usize = 0;
    const ONE_APPLIED_UPDATE: usize = 1;
    const TILE_COORDINATE_COUNT: usize = 3;
    const ONE_AUTOMATIC_OBSERVATION: usize = 1;
    const NPC_DEFINITION_INDEX: usize = 1;
    const BAR_TRANSITION_COUNT: usize = 3;
    const NO_WEAPON_BASE: f64 = 0.0;
    const HIT_DAMAGE: usize = 1;
    const HIT_EXPIRES: usize = 4;
    const BAR_END_FILL: usize = 2;
    const FULL_HEALTH_FILL: i32 = 255;
    const COOLDOWN_DEFINITION_ARG: usize = 0;
    const COOLDOWN_DURATION_ARG: usize = 2;
    const WHOLE_FINE_PERCENT: f64 = 1000.0;
    const FINE_THRESHOLD_REQUIRED: i64 = 500;
    const FINE_THRESHOLD_SPEND: i64 = 150;
    const FINE_ULTIMATE_REQUIRED: i64 = 1000;
    const NO_ADRENALINE: i64 = 0;
    const BASIC_ORDINARY_MIN: f64 = 300.0;
    const BASIC_ORDINARY_MAX: f64 = 1200.0;
    const BASIC_CONTROLLED_MIN: f64 = 800.0;
    const BASIC_CONTROLLED_MAX: f64 = 1460.0;
    const THRESHOLD_MIN: f64 = 400.0;
    const THRESHOLD_MAX: f64 = 2000.0;
    const ULTIMATE_MIN: f64 = 2000.0;
    const ULTIMATE_MAX: f64 = 4000.0;
    const UI_SETTLE_CYCLES: i32 = 60;
    let root = rs910_core::test_support::client_dir().join(GAMEPLAY_FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let frames = arrivals(&trace)?;
    let (server_players, server_npcs) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let events: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("ability-events.json"))?)?;
    let plan: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("plan.json"))?)?;
    let life_events: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("life-events.json"))?)?;
    let life_rows = life_events
        .as_array()
        .context("independent ordinary life publications")?;
    let player_publications: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("player-publications.json"))?)?;
    let player_rows = player_publications
        .as_array()
        .context("independent sent player publications")?;
    let mut last_player_world_tick = None;
    let mut ordinary_weapon_observations = FIRST_UPDATE_CURSOR;
    let mut applied_life_rows = FIRST_UPDATE_CURSOR;
    let mut replay = Replay::start(&trace)?;
    let mut replay_pings = Vec::new();
    let mut recorded_pings = Vec::new();
    let mut done = FIRST_UPDATE_CURSOR;
    let mut players = FIRST_UPDATE_CURSOR;
    let mut npcs = FIRST_UPDATE_CURSOR;
    let mut selectors = Vec::new();
    let mut native_operations = BTreeSet::new();
    let mut hit_damage = BTreeSet::new();
    let mut reduced_health = false;
    let mut fine_values = BTreeSet::new();
    let expected_buff_graphic = expected_anticipation_graphic(&replay)?;
    let mut native_buff_countdown = NativeBuffCountdown::default();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: every gameplay packet"
        );
        let next = replay.processed(&frames);
        let fresh = &frames[done..next];
        let player_updates = fresh
            .iter()
            .filter(|f| f.opcode == crate::proto::server::PLAYER_INFO)
            .count();
        let npc_updates = fresh
            .iter()
            .filter(|f| f.opcode == crate::proto::server::NPC_INFO)
            .count();
        assert!(player_updates <= ONE_APPLIED_UPDATE && npc_updates <= ONE_APPLIED_UPDATE, "cycle {cycle}: inspect every applied entity update; a settled snapshot cannot hide an intermediate update");
        let observed = observe(replay.game());
        for frame in fresh {
            native_buff_countdown.applied_packet(frame.opcode, &frame.payload, cycle)?;
            if frame.opcode == crate::proto::server::PLAYER_INFO {
                let expected = server_players
                    .get(players)
                    .context("independent player update")?;
                assert_eq!(
                    observed.local.as_ref().context("local player")?.tile,
                    expected[..TILE_COORDINATE_COUNT],
                    "cycle {cycle}: player update {players}"
                );
                let publication = player_rows
                    .get(players)
                    .context("independent publication for every sent PLAYER_INFO")?;
                assert_eq!(
                    publication["playerInfoOrdinal"].as_u64(),
                    Some(players as u64),
                    "PLAYER_INFO send ordinal"
                );
                last_player_world_tick = publication["tick"].as_i64();
                native_buff_countdown.world_publication(publication)?;
                let automatic = &publication["automatic"];
                if !automatic.is_null() {
                    let category = automatic["category"]
                        .as_i64()
                        .context("equipped cache weapon category")?
                        as i32;
                    let objects = replay
                        .ui()
                        .engine
                        .configs
                        .objs
                        .as_ref()
                        .context("public native object configs")?;
                    let weapon = objects
                        .get(rs910_symbols::obj::BRONZE_2H_SWORD.id() as u32)
                        .context("native bronze two-handed weapon")?;
                    let cache_category = weapon
                        .params
                        .iter()
                        .find_map(|(key, value)| {
                            if *key == param::WEAPON_CATEGORY_REF.id() {
                                if let rs910_config::config::ParamValue::Int(value) = value {
                                    return Some(*value);
                                }
                            }
                            None
                        })
                        .context("native weapon category reference")?;
                    assert_eq!(category, cache_category, "actual native weapon category");
                    let expected_sequence = replay
                        .ui()
                        .state
                        .configs
                        .structure(cache_category)?
                        .integer(param::WEAPON_ATTACK_SEQUENCE.id(), NO_ACTION)?;
                    assert_eq!(
                        automatic["weapon"].as_i64(),
                        Some(i64::from(rs910_symbols::obj::BRONZE_2H_SWORD.id())),
                        "ordinary equipped weapon identity"
                    );
                    assert_eq!(
                        automatic["expectedSequence"].as_i64(),
                        Some(i64::from(expected_sequence)),
                        "independent cache category sequence"
                    );
                    assert_eq!(
                        automatic["tick"].as_i64(),
                        publication["tick"].as_i64(),
                        "auto cue publication tick"
                    );
                    assert_eq!(
                        automatic["revision"], publication["animation"]["revision"],
                        "exact non-ability request survived to publication"
                    );
                    for mode in publication["animation"]["modes"]
                        .as_array()
                        .context("published animation modes")?
                    {
                        assert_eq!(
                            mode.as_i64(),
                            Some(i64::from(expected_sequence)),
                            "ordinary automatic sequence request"
                        );
                    }
                    assert_eq!(observed.local.as_ref().context("actual animated local player")?.main_anim, expected_sequence, "cycle {cycle}: actual native player attack sequence from this ordinary auto publication");
                    ordinary_weapon_observations += ONE_AUTOMATIC_OBSERVATION;
                }
                players += ONE_APPLIED_UPDATE;
            } else if frame.opcode == crate::proto::server::NPC_INFO {
                let expected = server_npcs.get(npcs).context("independent NPC update")?;
                assert_eq!(
                    observed.npcs.len(),
                    expected.len(),
                    "cycle {cycle}: NPC roster"
                );
                for &[index, definition, x, z, level] in expected {
                    let (actual_definition, actual) = observed
                        .npcs
                        .get(&(index as usize))
                        .context("replicated NPC")?;
                    assert_eq!(
                        (*actual_definition, actual.tile),
                        (definition, [x, z, level]),
                        "cycle {cycle}: NPC update {npcs}"
                    );
                }
                for &[index, definition, _, _, _] in expected {
                    if definition != npc::ABILITY_PRACTICE_DUMMY.id() {
                        continue;
                    }
                    let publication = life_rows
                        .get(applied_life_rows)
                        .context("independent life row for every applied NPC info")?;
                    assert_eq!(
                        publication["index"].as_i64(),
                        Some(i64::from(index)),
                        "cycle {cycle}: life publication actor"
                    );
                    assert_eq!(
                        publication["definition"].as_i64(),
                        Some(i64::from(definition)),
                        "life publication definition"
                    );
                    assert_eq!(
                        publication["npcInfoOrdinal"].as_u64(),
                        Some(npcs as u64),
                        "life publication bound to actual NPC_INFO send"
                    );
                    assert_eq!(
                        publication["tick"].as_i64(),
                        last_player_world_tick,
                        "life and local-player publication share this world tick"
                    );
                    let entity = replay
                        .game()
                        .runtime
                        .feed
                        .state
                        .npcs
                        .entities
                        .get(&(index as usize))
                        .context("actual native practice dummy")?;
                    let server_hits = publication["hits"]
                        .as_array()
                        .context("ordinary tick hits")?;
                    let server_bars = publication["headbars"]
                        .as_array()
                        .context("ordinary tick headbars")?;
                    if !server_hits.is_empty() || !server_bars.is_empty() {
                        let combat = entity
                            .path
                            .combat
                            .as_ref()
                            .context("received native damage state")?;
                        for hit in server_hits {
                            let damage = hit["damage"]
                                .as_i64()
                                .context("independent ordinary damage")?
                                as i32;
                            assert!(
                                combat.hits.iter().any(|actual| actual[HIT_DAMAGE] == damage
                                    && actual[HIT_EXPIRES] > cycle),
                                "cycle {cycle}: every published native hit: {hit}"
                            );
                        }
                        for bar in server_bars {
                            let fill =
                                bar["end"].as_i64().context("independent health fill")? as i32;
                            assert!(
                                combat
                                    .bars
                                    .iter()
                                    .flat_map(|actual| &actual.updates)
                                    .any(|actual| actual[BAR_END_FILL] == fill),
                                "cycle {cycle}: every published native headbar: {bar}"
                            );
                        }
                    }
                    applied_life_rows += ONE_APPLIED_UPDATE;
                }
                npcs += ONE_APPLIED_UPDATE;
            }
        }
        done = next;
        let selector = selected(&replay)?;
        if selectors.last() != Some(&selector) {
            selectors.push(selector);
        }
        if cycle
            == plan["stages"]["secondBar"]
                .as_i64()
                .context("second-bar stage")? as i32
                + UI_SETTLE_CYCLES
        {
            assert_eq!(selector, SECOND_BAR);
            icon(
                &mut replay,
                component::action_bar::SLICE_SLOT.packed(),
                "Anticipation",
                NATIVE_DIGIT_ONE,
            )?;
        }
        if cycle
            == plan["stages"]["firstBar"]
                .as_i64()
                .context("first-bar stage")? as i32
                + UI_SETTLE_CYCLES
        {
            assert_eq!(selector, FIRST_BAR);
            icon(
                &mut replay,
                component::action_bar::SLICE_SLOT.packed(),
                "Slice",
                NATIVE_DIGIT_ONE,
            )?;
        }
        fine_values.insert(bar_value(&replay, varp::ADRENALINE_FINE.id())?);
        for (opcode, payload) in client_frames(&out.written)? {
            if opcode == crate::proto::client::IF_BUTTON1 {
                native_operations.insert(i32::from_be_bytes(
                    payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?,
                ));
            }
        }
        for target in replay
            .game()
            .runtime
            .feed
            .state
            .npcs
            .entities
            .values()
            .filter(|n| n.type_id == npc::ABILITY_PRACTICE_DUMMY.id())
        {
            if let Some(combat) = &target.path.combat {
                for hit in &combat.hits {
                    if hit[HIT_EXPIRES] != NO_ACTION {
                        hit_damage.insert(hit[HIT_DAMAGE]);
                    }
                }
                reduced_health |= combat
                    .bars
                    .iter()
                    .flat_map(|bar| &bar.updates)
                    .any(|update| update[BAR_END_FILL] < FULL_HEALTH_FILL);
            }
        }
        native_buff_countdown.observe(&mut replay, expected_buff_graphic)?;
    }
    native_buff_countdown.finish()?;
    assert_eq!(replay_pings, recorded_pings, "ordered masked ping reports");
    assert_eq!(
        players,
        frames
            .iter()
            .filter(|f| f.opcode == crate::proto::server::PLAYER_INFO)
            .count()
    );
    assert_eq!(
        npcs,
        frames
            .iter()
            .filter(|f| f.opcode == crate::proto::server::NPC_INFO)
            .count()
    );
    assert!(
        selectors
            .windows(BAR_TRANSITION_COUNT)
            .any(|w| w == [FIRST_BAR, SECOND_BAR, FIRST_BAR]),
        "native selector transitions: {selectors:?}"
    );
    for packed in [
        component::action_bar::SLICE_SLOT.packed(),
        component::action_bar::PIERCING_SHOT_SLOT.packed(),
        component::action_bar::WRACK_SLOT.packed(),
        component::action_bar::SLOT_4.packed(),
    ] {
        assert!(
            native_operations.contains(&packed),
            "ordinary native ability operation {packed}"
        );
    }
    let mut cooldowns = BTreeSet::new();
    for frame in &frames {
        if let Some(UiEvent::RunScript(call)) =
            crate::session::parse_ui_event(frame.opcode, &frame.payload)?
        {
            if call.script_id == script::ABILITY_COOLDOWN_SCHEDULE.id() {
                if let (Some(ScriptArg::Int(definition)), Some(ScriptArg::Int(duration))) = (
                    call.args.get(COOLDOWN_DEFINITION_ARG),
                    call.args.get(COOLDOWN_DURATION_ARG),
                ) {
                    assert!(*duration > NO_ACTION);
                    cooldowns.insert(*definition);
                }
            }
        }
    }
    for definition in [
        structs::SLICE_ABILITY,
        structs::FORCEFUL_BACKHAND_ABILITY,
        structs::OVERPOWER_ABILITY,
        structs::ANTICIPATION_ABILITY,
        structs::GLOBAL_ABILITY_COOLDOWN,
    ] {
        assert!(
            cooldowns.contains(&definition.id()),
            "native cooldown {definition:?}"
        );
    }
    let rows = events
        .as_array()
        .context("ordinary server ability observations")?;
    let event = |definition: i32, controlled: bool| -> anyhow::Result<&serde_json::Value> {
        rows.iter()
            .find(|e| {
                e["definition"].as_i64() == Some(i64::from(definition))
                    && e["controlled"].as_bool() == Some(controlled)
            })
            .context("required ordinary ability hit")
    };
    let ordinary = event(structs::SLICE_ABILITY.id(), false)?;
    let bonus = event(structs::SLICE_ABILITY.id(), true)?;
    // The launched listener runs after a zero-flight hit installs its control.
    // Threshold/ultimate identity does not imply pre-impact control state.
    let unique_event = |definition: i32| -> anyhow::Result<&serde_json::Value> {
        let mut matching = rows
            .iter()
            .filter(|e| e["definition"].as_i64() == Some(i64::from(definition)));
        let event = matching
            .next()
            .context("required ordinary ability definition")?;
        anyhow::ensure!(
            matching.next().is_none(),
            "fixture has multiple launches for a unique stage"
        );
        Ok(event)
    };
    let threshold = unique_event(structs::FORCEFUL_BACKHAND_ABILITY.id())?;
    let ultimate = unique_event(structs::OVERPOWER_ABILITY.id())?;
    for (e, minimum, maximum) in [
        (ordinary, BASIC_ORDINARY_MIN, BASIC_ORDINARY_MAX),
        (bonus, BASIC_CONTROLLED_MIN, BASIC_CONTROLLED_MAX),
        (threshold, THRESHOLD_MIN, THRESHOLD_MAX),
        (ultimate, ULTIMATE_MIN, ULTIMATE_MAX),
    ] {
        let base = e["weaponBase"].as_f64().context("published weapon base")?;
        let damage = e["damage"].as_f64().context("ordinary hit damage")?;
        assert!(e["accurate"].as_bool() == Some(true) && base > NO_WEAPON_BASE);
        assert!(
            damage >= (base * minimum / WHOLE_FINE_PERCENT).floor()
                && damage <= (base * maximum / WHOLE_FINE_PERCENT).floor(),
            "independently dated damage bounds: {e}"
        );
        assert!(
            hit_damage.contains(&(damage as i32)),
            "actual native NPC hitmark for {e}"
        );
    }
    assert!(bonus["damage"].as_i64() > ordinary["damage"].as_i64());
    let before = threshold["beforeFine"]
        .as_i64()
        .context("threshold eligibility")?;
    let after = threshold["afterFine"].as_i64().context("threshold spend")?;
    assert!(before >= FINE_THRESHOLD_REQUIRED);
    assert_eq!(before - after, FINE_THRESHOLD_SPEND);
    assert!(
        ultimate["beforeFine"]
            .as_i64()
            .context("ultimate eligibility")?
            >= FINE_ULTIMATE_REQUIRED
    );
    assert_eq!(ultimate["afterFine"].as_i64(), Some(NO_ADRENALINE));
    assert!(
        fine_values.contains(&(after as i32)) && fine_values.contains(&(NO_ADRENALINE as i32)),
        "actual native adrenaline updates"
    );
    assert!(reduced_health, "actual received NPC health bar");
    let expected_life_rows = server_npcs
        .iter()
        .take(npcs)
        .flat_map(|roster| roster.iter())
        .filter(|row| row[NPC_DEFINITION_INDEX] == npc::ABILITY_PRACTICE_DUMMY.id())
        .count();
    assert!(
        expected_life_rows > FIRST_UPDATE_CURSOR,
        "recorded practice dummy publications"
    );
    assert_eq!(
        applied_life_rows, expected_life_rows,
        "every captured NPC_INFO publication has an independent exact life row"
    );
    assert!(
        ordinary_weapon_observations > FIRST_UPDATE_CURSOR,
        "no ordinary equipped automatic animation reached the real native player owner"
    );
    assert!(
        replay.ui().diagnostics.errors.is_empty(),
        "native hook errors: {:?}",
        replay.ui().diagnostics.errors
    );
    Ok(())
}

fn native_combat_mode_snapshot(
    replay: &mut Replay,
    procedure: rs910_symbols::ScriptId,
    arguments: &[i32],
) -> anyhow::Result<native910::vm::Snapshot> {
    let session = replay.core.session.as_mut().context("ordinary session")?;
    let runtime = &mut session.ui;
    let game = session.game.as_mut().context("ordinary game")?;
    crate::client_game::with_game(game, |variables| {
        let provider = crate::ui_scripts::Provider {
            scripts: &runtime.scripts,
            definitions: variables.definitions,
        };
        let script = provider
            .get(procedure.id())?
            .context("native combat procedure")?;
        let mut runner = crate::ui_hook_host::Runner {
            pool: &mut runtime.pool,
            provider: &provider,
            engine: &mut runtime.engine,
            domains: crate::ui_hook_host::Domains::Game(variables),
            executions: Vec::new(),
            missing: Vec::new(),
        };
        runner.run_compiled_with_ints(
            &mut runtime.store,
            &mut runtime.state,
            procedure.id(),
            &script,
            crate::ui_hooks::INTERACTIVE_LIMIT,
            arguments,
        )?;
        assert!(runner.missing.is_empty(), "native procedure dependencies");
        let execution = runner.executions.pop().context("native execution")?;
        execution
            .result
            .map_err(|error| anyhow::anyhow!("native combat procedure: {error:?}"))?;
        Ok(execution.snapshot)
    })
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_legacy_login_mode_is_consumed_by_native_combat_ui_in_the_normal_client(
) -> anyhow::Result<()> {
    const LEGACY_MODE: i32 = 3;
    const LEGACY_ACTIVE: i32 = 1;
    const MODE_RESULT_INDEX: usize = 0;
    const AFTER_INITIAL_UI: i32 = INITIAL_UI_READY + FIRST_CYCLE;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    for cycle in FIRST_CYCLE..=INITIAL_UI_READY {
        replay.cycle(&trace, cycle)?;
    }
    let before = native_combat_mode_snapshot(&mut replay, script::COMBAT_MODE_VALUE, &[])?;
    assert_ne!(
        before.ints[MODE_RESULT_INDEX], LEGACY_MODE,
        "historical EoC fixture remains explicit"
    );
    let ironman_before = replay
        .game()
        .varbit_value(varbit::IRONMAN_MODE_ACTIVE.id() as u16)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let packet = std::fs::read(
        rs910_core::test_support::client_fixtures()
            .join("recorded/combat-mode/legacy-login-varp.blk"),
    )?;
    replay.step_live(AFTER_INITIAL_UI, &packet, &mut |_, _| {})?;
    assert_eq!(
        replay
            .game()
            .varbit_value(varbit::LEGACY_COMBAT_ACTIVE.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        LEGACY_ACTIVE
    );
    assert_eq!(
        replay
            .game()
            .varbit_value(varbit::IRONMAN_MODE_ACTIVE.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        ironman_before
    );
    let mode = native_combat_mode_snapshot(&mut replay, script::COMBAT_MODE_VALUE, &[])?;
    assert_eq!(mode.ints, [LEGACY_MODE]);
    let presentation =
        native_combat_mode_snapshot(&mut replay, script::COMBAT_MODE_PRESENTATION, &mode.ints)?;
    assert_eq!(presentation.strings, [Some("Legacy".to_owned())]);
    assert_eq!(replay.game().cycle, AFTER_INITIAL_UI);
    Ok(())
}
