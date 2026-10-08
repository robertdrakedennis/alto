//! Cache backed consumable/onop replay.  This stays opt-in because it needs
//! the revision-910 pack and the exported server session corpus.

use crate::ui_runtime::*;
use rs910_symbols::component::{gameplay_settings, minimap};
use rs910_symbols::{inv, script, structs, varbit};

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn consumable_drink_replay_updates_dose_and_run_energy() -> anyhow::Result<()> {
    replay_consumables("consumable-drink")
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn potion_preferences_replay_uses_cache_checkbox() -> anyhow::Result<()> {
    replay_consumables("consumable-settings")
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn inventory_destroy_empty_siblings_use_shared_toggle() -> anyhow::Result<()> {
    // Verbatim cache port, no Wiki. Inventory category (DB 1303, enum 14569
    // index 10) type-3 checkboxes share one dispatch path:
    // - Containers 365:15/16 are empty in the cache; 2957 builds the category
    //   list, 2970 builds the settings list, 2830 builds each row and calls
    //   10418->10419 for type 3 (checkbox construction, graphic struct 41495).
    // - Getter chain 2524->2526 maps each setting struct to its varbit
    //   (41566->1727, 41561->1723, 41565->1728; vial 41563->26774).
    // - Shared Toggle onop is 10422; category navigation onop is 2930.
    // - Sibling bits live in player varp 5863 bits 0..7 (hex 010016e7...);
    //   vial is player varp 5967 bit 30 (26774 hex 0100174f021e1e00).
    //   Selection is player varp 8172 (42101 bits 0..9, 42102 bits 10..19).
    // New rows are data + parametrized assertions through that shared path,
    // never new branches per setting. Sprite/backpack/text effects are only
    // asserted where the consumer already exists (vial sprites 18541/18543 in
    // the existing vial replay); the three rows below are state-only and stay
    // `partial` because their gameplay consumers (cooking/bowl/beer/mix/decant
    // disposal) are absent. This test does not build those consumers.
    struct InventoryRow {
        slot: i32,
        struct_id: i32,
        varbit: u16,
        label: &'static str,
    }
    // Immediate vial neighbours (slots 8/9/11 around slot-10 vial), all
    // controlType 3 with defaults 7513=3/7524=0/7537=0 from ParamConfig.
    // Mirrors the server worker's INVENTORY_TOGGLE_TABLE exactly.
    const ROWS: &[InventoryRow] = &[
        InventoryRow {
            slot: 8,
            struct_id: structs::PIE_DISH_DESTROY_OPTION.id(),
            varbit: varbit::PIE_DISH_DESTROY.id() as u16,
            label: "Destroy empty pie dishes when eating",
        },
        InventoryRow {
            slot: 9,
            struct_id: structs::BEER_GLASS_DESTROY_OPTION.id(),
            varbit: varbit::BEER_GLASS_DESTROY.id() as u16,
            label: "Destroy empty beer glasses when drinking",
        },
        InventoryRow {
            slot: 11,
            struct_id: structs::MIXING_VIAL_DESTROY_OPTION.id(),
            varbit: varbit::MIXING_VIAL_DESTROY.id() as u16,
            label: "Destroy empty vials when mixing potions",
        },
    ];
    let rows = crate::test_support::replay_json("consumable-settings", "frames.json");
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.engine.outgoing.clear();
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    for row in rows
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("consumable rows must be an array"))?
        .iter()
        .take(2)
    {
        for raw in row["frames"].as_array().map_or(&[][..], |v| v.as_slice()) {
            let bytes: Vec<u8> = raw
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let (frame, used) =
                crate::net::decode_frame(&bytes)?.ok_or_else(|| anyhow::anyhow!("frame"))?;
            anyhow::ensure!(used == bytes.len(), "trailing consumable frame bytes");
            if game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                game.apply_next(game.cycle as i64)
                    .map_err(|e| anyhow::anyhow!("server frame: {e:?}"))?;
                if game.runtime.map_request.is_some() {
                    let map = game.runtime.prepare_map(&pack)?;
                    game.runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("map: {e:?}"))?;
                }
            } else if let Some(event) =
                crate::session::parse_ui_event(frame.opcode, &frame.payload)?
            {
                clock.with(&mut game, |vars| ui.packet(vars, &event))?;
            }
        }
        game.poll_vars(|| clock.0)
            .map_err(|e| anyhow::anyhow!("var poll: {e:?}"))?;
        for _ in 0..40 {
            game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
    }
    // Inventory category selected: varp 8172 low ten bits == 10 (varbit 42101),
    // interface 365 open. Read through the retained getter, not a constant.
    anyhow::ensure!(
        game.varbit_value(varbit::GAMEPLAY_SETTINGS_CATEGORY.id() as u16)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?
            == 10,
        "Inventory category not selected (8172 bits 0..9)"
    );
    anyhow::ensure!(
        ui.store
            .get(gameplay_settings::OPTIONS.packed(), 0)?
            .is_some(),
        "365:16 settings list missing"
    );
    fn hook_int(hook: &[crate::ui_components::Arg], index: usize) -> anyhow::Result<i32> {
        match hook.get(index) {
            Some(crate::ui_components::Arg::Int(value)) => Ok(*value),
            _ => anyhow::bail!("checkbox argument {index}: {hook:?}"),
        }
    }
    let mut proof = Vec::new();
    for row in ROWS {
        let component = gameplay_settings::OPTIONS.packed();
        let node = ui
            .store
            .get(component, row.slot)?
            .ok_or_else(|| anyhow::anyhow!("Inventory row missing {}:{}", component, row.slot))?;
        {
            let node = node.borrow();
            anyhow::ensure!(
                node.f.invobject == -1,
                "row {} object {}",
                row.slot,
                node.f.invobject
            );
            let op = node
                .ops
                .as_ref()
                .and_then(|ops| ops.first())
                .and_then(Option::as_ref)
                .map(|s| String::from_utf16_lossy(s));
            anyhow::ensure!(
                op.as_deref() == Some("Toggle"),
                "row {} cached op: {op:?}",
                row.slot
            );
            let hook = node
                .hooks
                .get("onop")
                .ok_or_else(|| anyhow::anyhow!("row {} onop missing", row.slot))?;
            anyhow::ensure!(
                matches!(hook.first(), Some(crate::ui_components::Arg::Int(id)) if *id == script::SETTINGS_CHECKBOX_OP.id()),
                "row {} shared Toggle handler: {hook:?}",
                row.slot
            );
        }
        let (before_value, before_bit, base_varp, start, end, domain) =
            clock.with(&mut game, |vars| {
                let node = ui
                    .store
                    .get(component, row.slot)?
                    .ok_or_else(|| anyhow::anyhow!("row"))?;
                let hook = node
                    .borrow()
                    .hooks
                    .get("onop")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("hook"))?;
                let value = hook_int(&hook, 7)?;
                // Shared construction args: 10419 installs (365:15, sub-id, "", "", value, 41495).
                anyhow::ensure!(
                    hook_int(&hook, 0)? == script::SETTINGS_CHECKBOX_OP.id(),
                    "handler"
                );
                anyhow::ensure!(
                    hook_int(&hook, 3)? == gameplay_settings::OPTIONS_FRAME.packed(),
                    "container 365:15"
                );
                anyhow::ensure!(
                    hook_int(&hook, 8)? == structs::SETTINGS_CHECKBOX_GRAPHICS.id(),
                    "graphic struct"
                );
                let bit = vars.get_bit(row.varbit, false)?;
                let (base, start, end) = vars.bit_base(row.varbit)?;
                let domain = vars.bit_domain(row.varbit)?;
                Ok((value, bit, base, start, end, domain))
            })?;
        // Getter (2526) and hook agree before the click; single-bit player varbit.
        anyhow::ensure!(
            before_value == before_bit,
            "row {} hook {before_value} != varbit {before_bit}",
            row.slot
        );
        anyhow::ensure!(domain == 0, "row {} player domain", row.slot);
        anyhow::ensure!(start == end, "row {} single-bit checkbox", row.slot);
        // Drive the real cache onop through the retained UI (no per-row branch).
        ui.engine.outgoing.clear();
        ui.diagnostics.errors.clear();
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: component,
                child: row.slot,
                base: None,
            });
        clock.tick(&mut game, &mut ui)?;
        anyhow::ensure!(
            ui.diagnostics.errors.is_empty(),
            "row {} hook errors: {:?}",
            row.slot,
            ui.diagnostics.errors
        );
        let expected_wire = crate::ui_interaction::button_packet(1, component, row.slot, -1);
        anyhow::ensure!(
            !expected_wire.is_empty() && ui.engine.outgoing == expected_wire,
            "row {} IF_BUTTON1 wire mismatch: {:?}",
            row.slot,
            ui.engine.outgoing
        );
        let (after_value, after_bit) = clock.with(&mut game, |vars| {
            let node = ui
                .store
                .get(component, row.slot)?
                .ok_or_else(|| anyhow::anyhow!("row"))?;
            let hook = node
                .borrow()
                .hooks
                .get("onop")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("hook"))?;
            Ok((hook_int(&hook, 7)?, vars.get_bit(row.varbit, false)?))
        })?;
        // Shared 10422 flips the hook optimistically; the server-owned varbit
        // does not move until the authoritative ack (partial gap, not a failure).
        anyhow::ensure!(
            after_value == 1 - before_value,
            "row {} hook did not toggle {before_value}->{after_value}",
            row.slot
        );
        anyhow::ensure!(
            after_bit == before_bit,
            "row {} local onop must not persist server varbit",
            row.slot
        );
        // Simulate the authoritative ack the missing server worker would send:
        // set the single bit in the base varp through the real server path,
        // poll the interfaces, then tick so getter 2526
        // observes it. This proves state-per-tick + CS2 var semantics without
        // building the absent gameplay consumer.
        let old_varp = game
            .runtime
            .feed
            .state
            .varps
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("varps"))?
            .get(base_varp)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let toggled = 1 - before_bit;
        let cleared = old_varp & !(1i32 << (start as u32));
        let new_varp = cleared | (toggled << (start as u32));
        game.install_server_varp(base_varp, new_varp, clock.0)
            .map_err(|e| anyhow::anyhow!("server varp: {e:?}"))?;
        game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        let (final_value, final_bit) = clock.with(&mut game, |vars| {
            let node = ui
                .store
                .get(component, row.slot)?
                .ok_or_else(|| anyhow::anyhow!("row"))?;
            let hook = node
                .borrow()
                .hooks
                .get("onop")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("hook"))?;
            Ok((hook_int(&hook, 7)?, vars.get_bit(row.varbit, false)?))
        })?;
        anyhow::ensure!(
            final_bit == toggled,
            "row {} varbit after ack {final_bit} != {toggled}",
            row.slot
        );
        anyhow::ensure!(
            final_value == toggled,
            "row {} hook after ack {final_value} != {toggled}",
            row.slot
        );
        anyhow::ensure!(
            game.varbit_value(row.varbit)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?
                == toggled,
            "row {} retained varbit readback",
            row.slot
        );
        proof.push(serde_json::json!({
            "slot": row.slot, "struct": row.struct_id, "varbit": row.varbit,
            "varp": base_varp, "start": start, "end": end,
            "label": row.label, "status": "partial (state-only; gameplay consumer absent)",
            "hookBefore": before_value, "bitBefore": before_bit,
            "wire": expected_wire, "hookAfterLocal": after_value, "bitAfterLocal": after_bit,
            "varpOld": old_varp, "varpNew": new_varp,
            "hookAfterAck": final_value, "bitAfterAck": final_bit,
            "anchors": {"component": "365:16", "container": "365:15", "onop": 10422, "build": [2957, 2970, 2830, 10418, 10419], "getter": [2524, 2526], "categoryOnop": 2930, "categoryVarp": 8172},
        }));
    }
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "cached Inventory hook errors: {:?}",
        ui.diagnostics.errors
    );
    std::fs::write(
        crate::test_support::proof_dir("consumable-settings")
            .join("consumable-inventory-proof.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "source": "real cache 365:15/16 onop 10422 through retained UI; getter 2524->2526; varp 5863 bits + vial 5967/8172 reference; no Wiki",
            "rows": proof,
            "thinVerified": "vial slot 10 (41563->26774/varp 5967 bit30, sprites 18541/18543) stays covered by potion_preferences_replay_uses_cache_checkbox",
            "partial": "slots 8/9/11 state-only; pie/beer/mixing disposal consumers absent and not built",
        }))?,
    )?;
    Ok(())
}
fn replay_consumables(scenario: &str) -> anyhow::Result<()> {
    let root = crate::test_support::proof_dir(scenario);
    let rows = crate::test_support::replay_json(scenario, "frames.json");
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.engine.outgoing.clear();
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut proof = Vec::new();
    let mut checkbox_graphics = std::collections::BTreeMap::new();
    for row in rows
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("consumable rows must be an array"))?
    {
        let click = row.get("click").filter(|v| !v.is_null());
        let mut click_wire = Vec::new();
        let mut onop_handler = String::new();
        if let Some(click) = click {
            let component = click["component"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("click component"))?
                as i32;
            let slot = click["slot"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("click slot"))? as i32;
            let operation = click["operation"].as_i64().unwrap_or(1) as i32;
            let item_id = click["itemId"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("click itemId"))? as i32;
            let node = ui
                .store
                .get(component, slot)?
                .ok_or_else(|| anyhow::anyhow!("drink component {component}:{slot}"))?;
            let node = node.borrow();
            anyhow::ensure!(
                node.f.invobject == item_id,
                "cached object {} != click item {}",
                node.f.invobject,
                item_id
            );
            let is_drink = click["kind"].as_str().unwrap_or("Drink") == "Drink";
            let drink = node
                .ops
                .as_ref()
                .and_then(|ops| ops.first())
                .and_then(Option::as_ref)
                .map(|s| String::from_utf16_lossy(s));
            if is_drink {
                anyhow::ensure!(
                    drink.as_deref() == Some("Drink"),
                    "cached op for {component}:{slot}: {drink:?}"
                );
            }
            if click["kind"].as_str() == Some("category") {
                anyhow::ensure!(
                    matches!(
                        node.hooks.get("onop").and_then(|h| h.first()),
                        Some(crate::ui_components::Arg::Int(id)) if *id == script::SETTINGS_CATEGORY_OP.id()
                    ),
                    "category handler"
                );
            }
            if click["kind"].as_str() == Some("Toggle") {
                anyhow::ensure!(
                    drink.as_deref() == Some("Toggle"),
                    "cached setting operation: {drink:?}"
                );
                anyhow::ensure!(
                    matches!(
                        node.hooks.get("onop").and_then(|h| h.first()),
                        Some(crate::ui_components::Arg::Int(id)) if *id == script::SETTINGS_CHECKBOX_OP.id()
                    ),
                    "checkbox handler"
                );
            }
            onop_handler = format!("{:?}", node.hooks.get("onop"));
            drop(node);
            ui.engine.outgoing.clear();
            ui.state
                .interaction
                .actions
                .push_back(crate::ui_interaction::Action::Op {
                    op: operation,
                    parent: component,
                    child: slot,
                    base: None,
                });
            clock.tick(&mut game, &mut ui)?;
            // The real cache onop1620 must execute before the wire action:
            // dim the slot to100 and install timer1621 to restore it.
            let clicked = ui.store.get(component, slot)?.unwrap();
            let clicked = clicked.borrow();
            if is_drink {
                anyhow::ensure!(
                    clicked.f.trans == 100,
                    "cached Drink onop did not dim the slot"
                );
                anyhow::ensure!(
                    matches!(
                        clicked.hooks.get("ontimer").and_then(|v| v.first()),
                        Some(crate::ui_components::Arg::Int(id)) if *id == script::ITEM_SLOT_UNDIM.id()
                    ),
                    "cached Drink onop did not install its timer"
                );
            }
            click_wire = click["wire"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("click wire"))?
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            anyhow::ensure!(
                ui.engine.outgoing == click_wire,
                "Drink IF_BUTTON wire mismatch: {:?}",
                ui.engine.outgoing
            );
        }
        for raw in row["frames"].as_array().map_or(&[][..], |v| v.as_slice()) {
            let bytes: Vec<u8> = raw
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let (frame, used) = crate::net::decode_frame(&bytes)?
                .ok_or_else(|| anyhow::anyhow!("incomplete consumable frame"))?;
            anyhow::ensure!(used == bytes.len(), "trailing consumable frame bytes");
            if game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                game.apply_next(game.cycle as i64)
                    .map_err(|e| anyhow::anyhow!("server frame: {e:?}"))?;
                if game.runtime.map_request.is_some() {
                    let map = game.runtime.prepare_map(&pack)?;
                    game.runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("map: {e:?}"))?;
                }
            } else if let Some(event) =
                crate::session::parse_ui_event(frame.opcode, &frame.payload)?
            {
                clock.with(&mut game, |vars| ui.packet(vars, &event))?;
            }
        }
        game.poll_vars(|| clock.0)
            .map_err(|e| anyhow::anyhow!("var poll: {e:?}"))?;
        for _ in 0..40 {
            game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        let expected = &row["expected"];
        if expected.is_object() {
            let inv = inv::BACKPACK.id();
            let slot = expected["slot"].as_i64().unwrap_or(0) as i32;
            let item = expected["itemId"].as_i64().unwrap_or(-1) as i32;
            anyhow::ensure!(
                ui.engine.inv_cache.slot_type(inv, slot, false) == item,
                "dose inventory mismatch"
            );
            let mut visual_component = -1i32;
            let mut visual_object = -1i32;
            let mut visual_op = String::new();
            if let Some(click) = click.filter(|c| c["kind"].as_str().unwrap_or("Drink") == "Drink")
            {
                let component = click["component"].as_i64().unwrap() as i32;
                let actual = ui
                    .store
                    .get(component, click["slot"].as_i64().unwrap() as i32)?
                    .ok_or_else(|| anyhow::anyhow!("visual consumable node"))?;
                let actual = actual.borrow();
                visual_component = component;
                visual_object = actual.f.invobject;
                visual_op = actual
                    .ops
                    .as_ref()
                    .and_then(|ops| ops.first())
                    .and_then(Option::as_ref)
                    .map(|s| String::from_utf16_lossy(s))
                    .unwrap_or_default();
                anyhow::ensure!(
                    visual_object == item,
                    "visual dose inventory mismatch component={} slot={} actual={} expected={}",
                    component,
                    click["slot"],
                    visual_object,
                    item
                );
                anyhow::ensure!(actual.f.trans == 0, "cached item opacity did not recover");
            }
            let energy = expected["energy"].as_i64().unwrap_or(0) as i32;
            anyhow::ensure!(
                game.ui_variables.queries.run_energy == energy,
                "energy mismatch"
            );
            let node = ui
                .store
                .get(minimap::RUN_ENERGY_TEXT.packed(), -1)?
                .ok_or_else(|| anyhow::anyhow!("energy orb component missing"))?;
            let text = node.borrow().f.text.clone().unwrap_or_default();
            anyhow::ensure!(
                String::from_utf16_lossy(&text) == format!("{energy}%"),
                "energy orb text"
            );
            let orb_text = String::from_utf16_lossy(&text);
            let mut checkbox_value = None;
            let mut checkbox_graphic = None;
            if expected["inventorySettings"].as_bool() == Some(true) {
                let node = ui
                    .store
                    .get(gameplay_settings::OPTIONS.packed(), 10)?
                    .ok_or_else(|| anyhow::anyhow!("vial checkbox missing at {}", row["label"]))?;
                let node = node.borrow();
                let hook = node
                    .hooks
                    .get("onop")
                    .ok_or_else(|| anyhow::anyhow!("vial checkbox hook missing"))?;
                let integer = |index: usize| match hook.get(index) {
                    Some(crate::ui_components::Arg::Int(value)) => Ok(*value),
                    _ => Err(anyhow::anyhow!("checkbox argument {index}: {hook:?}")),
                };
                anyhow::ensure!(
                    integer(0)? == script::SETTINGS_CHECKBOX_OP.id(),
                    "vial checkbox hook"
                );
                let value = integer(7)?;
                let expected_value = i32::from(expected["destroyVials"].as_bool().unwrap());
                anyhow::ensure!(
                    value == expected_value,
                    "checkbox state at {}: {value} expected {expected_value}; hook {hook:?}",
                    row["label"]
                );
                let graphic = ui
                    .store
                    .get(integer(3)?, integer(4)?)?
                    .ok_or_else(|| anyhow::anyhow!("checkbox graphic missing"))?;
                checkbox_value = Some(value);
                let sprite = graphic.borrow().f.graphic;
                anyhow::ensure!(sprite >= 0, "checkbox must have a real sprite");
                if let Some(previous) = checkbox_graphics.insert(value, sprite) {
                    anyhow::ensure!(
                        previous == sprite,
                        "checkbox state must use consistent sprites"
                    );
                }
                checkbox_graphic = Some(sprite);
            }
            proof.push(serde_json::json!({"checkboxValue":checkbox_value,"checkboxGraphic":checkbox_graphic,"label":row["label"],"component":visual_component,"onop":onop_handler,"op1":visual_op,"invobject":visual_object,"orbText":orb_text,"wire":click_wire,"energy":energy,"slot":slot,"itemId":item}));
        }
    }
    if !checkbox_graphics.is_empty() {
        anyhow::ensure!(
            checkbox_graphics.len() == 2 && checkbox_graphics.get(&0) != checkbox_graphics.get(&1),
            "checked and unchecked sprites must differ"
        );
    }
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "cached consumable hook errors: {:?}",
        ui.diagnostics.errors
    );
    std::fs::write(
        root.join("consumable-ui-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    Ok(())
}
