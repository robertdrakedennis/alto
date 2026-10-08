//! Authored interfaces go through the production pack reader, packet parser,
//! lifecycle hooks, pointer input and retained draw path.
use super::ui_imported_replay;
use native910::{
    interface::{self, ComponentBody, HookArg, Hooks},
    isource,
    pack::PackArchive,
    project,
};

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn authored_ui_build_reaches_packet_hooks_pointer_input_and_paint() -> anyhow::Result<()> {
    const NEXT_ID: u32 = 1;
    const ROOT_FILE: u32 = 0;
    const LABEL_FILE: u32 = 1;
    const FIRST_DYNAMIC_CHILD: i32 = 0;
    const NO_PARENT: i32 = -1;
    const WIDTH: u16 = 360;
    const HEIGHT: u16 = 80;
    const CANVAS: [i32; 2] = [640, 480];
    const LOCAL_PLAYER_SLOT: usize = 1;
    const REPLAY_SEED: u64 = 910;
    const COLOUR_CHANNELS: usize = 3;
    const CENTRE_DIVISOR: i32 = 2;
    const LEFT_BUTTON_ACTION: i32 = 0;
    const CLICK_COUNT: i32 = 1;
    const CS2_STACK_CAPACITY: usize = 1000;
    const RETAINED_LONG_VALUE: i64 = -7;
    const OPAQUE_TEXT_TRANSPARENCY: u8 = 0;
    const WHITE_TEXT_COLOUR: i32 = 0x00ff_ffff;
    const INITIAL_TEXT: &str = "Authored interface loaded";
    const CLICKED_TEXT: &str = "Pointer hook executed";
    let base_pack = crate::test_support::require_pack("client.interfaces.js5");
    let root = base_pack.root();
    let scripts = PackArchive::open(&root.join("client.scripts.js5"))?;
    let archive = PackArchive::open(&root.join("client.interfaces.js5"))?;
    let script_id = scripts.group_ids().max().unwrap() + NEXT_ID;
    let click_id = script_id + NEXT_ID;
    let group = archive.group_ids().max().unwrap() + NEXT_ID;
    let address = |file| {
        native910::xref::pack_component(native910::xref::ComponentRef {
            iface: group as i32,
            child: file as i32,
        })
        .unwrap()
    };
    let mut frame_template = None;
    let mut label_template = None;
    let fonts = crate::ui_fonts::Fonts::from_pack(base_pack.clone(), Some(u64::default()))?;
    for id in archive.group_ids() {
        for bytes in archive.group_files(id)?.unwrap_or_default().values() {
            let component = interface::decode_component(bytes, i32::default())?;
            match component.body {
                ComponentBody::Layer { .. } if frame_template.is_none() => {
                    frame_template = Some(component)
                }
                ComponentBody::Text { font, mono, .. }
                    if label_template.is_none() && fonts.get_font(font, false, mono)?.is_some() =>
                {
                    label_template = Some(component)
                }
                _ => {}
            }
        }
        if frame_template.is_some() && label_template.is_some() {
            break;
        }
    }
    let mut frame = frame_template.unwrap();
    let mut label = label_template.unwrap();
    for component in [&mut frame, &mut label] {
        component.x = i16::default();
        component.y = i16::default();
        component.width = WIDTH;
        component.height = HEIGHT;
        component.width_mode = i8::default();
        component.height_mode = i8::default();
        component.x_mode = i8::default();
        component.y_mode = i8::default();
        component.aspect = None;
        component.clientcode = u16::default();
        component.hide = false;
        component.hooks = Hooks::default();
        component.transmits = Default::default();
        component.ops.clear();
    }
    frame.layer = NO_PARENT;
    frame.body = ComponentBody::Layer {
        scroll_width: WIDTH,
        scroll_height: HEIGHT,
    };
    label.layer = address(ROOT_FILE);
    if let ComponentBody::Text { trans, colour, .. } = &mut label.body {
        *trans = OPAQUE_TEXT_TRANSPARENCY;
        *colour = WHITE_TEXT_COLOUR;
    }
    label.hooks.onload = Some(vec![HookArg::Int(script_id as i32)]);
    label.hooks.onclick = Some(vec![HookArg::Int(click_id as i32)]);
    let directory = crate::test_support::proof_dir("ui-authoring");
    let manifest = directory.join("project.txt");
    std::fs::write(&manifest, format!("base-sha256 {}\ninterfaces-sha256 {}\ninames inames.txt\nscript {script_id} initialize initialize.rs2\nscript {click_id} clicked clicked.rs2\ncomponent {group} {ROOT_FILE} frame.ifc\ncomponent {group} {LABEL_FILE} label.ifc\n", project::sha256(&std::fs::read(root.join("client.scripts.js5"))?), project::sha256(&std::fs::read(root.join("client.interfaces.js5"))?)))?;
    std::fs::write(directory.join("inames.txt"), format!("interface {group} workshop\ncomponent {group} {ROOT_FILE} frame\ncomponent {group} {LABEL_FILE} label\n"))?;
    for (name, text) in [("initialize", INITIAL_TEXT), ("clicked", CLICKED_TEXT)] {
        let creation = if name == "initialize" {
            format!("push_constant_string(workshop/frame);\npush_constant_string({});\npush_constant_string({FIRST_DYNAMIC_CHILD});\ncc_create(0);\npush_string_local($fresh);\nstring_length(0);\npop_int_discard(0);\npush_constant_string(\"temporary\");\ncc_setopbase(0);\npush_string_local($fresh);\ncc_setopbase(0);\npush_constant_string({click_id});\npush_string_local($fresh);\npush_constant_string(\"s\");\ncc_setonclick(0);\n", label.type_id)
        } else {
            String::new()
        };
        let parameters = if name == "clicked" {
            "string $ignored"
        } else {
            ""
        };
        let locals = if name == "initialize" {
            "string $fresh;\n"
        } else {
            ""
        };
        let consume_argument = if name == "clicked" {
            "push_string_local($ignored);\nappend(0);\n"
        } else {
            ""
        };
        std::fs::write(directory.join(format!("{name}.rs2")), format!("[clientscript,{name}]({parameters})\n{locals}{creation}push_constant_string(\"{text}\");\n{consume_argument}push_constant_string(workshop/label);\nif_settext(0);\nreturn(0);\n"))?;
    }
    for (name, component) in [("frame", &frame), ("label", &label)] {
        std::fs::write(
            directory.join(format!("{name}.ifc")),
            isource::format_component(&isource::lift_component(component), &Default::default()),
        )?;
    }
    let output = directory.join("build");
    project::build(&manifest, root, &output)?;
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        if !output.join(name).exists() && path.is_file() {
            std::os::unix::fs::symlink(&path, output.join(name))?;
        }
    }
    let pack = crate::cache::Pack::open_with_overlay(&output, None);
    let configs = rs910_config::login_configs::LoginConfigs::read(&pack);
    let mut game = crate::client_game::ClientGame::login_with(
        &pack,
        &configs,
        LOCAL_PLAYER_SLOT,
        crate::protocol910::live::Feed::default(),
        REPLAY_SEED,
        true,
    )?;
    let mut ui = crate::ui_runtime::Runtime::new_with(pack.clone(), configs)?;
    ui.resize(CANVAS)?;
    ui.diagnostics.capture = true;
    ui.target.quiet = true;
    let mut clock = crate::test_support::SimClock::default();
    let mut wire = vec![crate::proto::server::IF_OPENTOP];
    wire.extend(rs910_protocol::server_prot::authoring_open_top_payload(
        group.try_into()?,
    ));
    let (packet, consumed) = crate::net::decode_frame(&wire)?.unwrap();
    assert_eq!(consumed, wire.len());
    let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?.unwrap();
    clock.with(&mut game, |vars| ui.packet(vars, &event))?;
    clock.tick(&mut game, &mut ui)?;
    let text = |ui: &mut crate::ui_runtime::Runtime| -> anyhow::Result<String> {
        let component = ui.store.get(address(LABEL_FILE), NO_PARENT)?.unwrap();
        let value = String::from_utf16_lossy(component.borrow().f.text.as_ref().unwrap());
        Ok(value)
    };
    assert_eq!(text(&mut ui)?, INITIAL_TEXT);
    let created = ui
        .store
        .get(address(ROOT_FILE), FIRST_DYNAMIC_CHILD)?
        .unwrap();
    assert_eq!(
        created.borrow().hooks.get("onclick").unwrap().first(),
        Some(&crate::ui_components::Arg::Int(click_id as i32))
    );
    assert_eq!(
        created.borrow().hooks["onclick"],
        [
            crate::ui_components::Arg::Int(click_id as i32),
            crate::ui_components::Arg::Null,
        ]
    );
    assert!(created.borrow().f.opbase.is_none());
    let origin = created
        .borrow()
        .creation_origin
        .clone()
        .expect("runtime child creator identity");
    assert_eq!(origin.script, Some(script_id as i32));
    let built_scripts = PackArchive::open(&output.join("client.scripts.js5"))?;
    let initialize = native910::script::decode_script(
        &built_scripts.group_files(script_id)?.unwrap()[&ROOT_FILE],
        &native910::opcode::OpcodeBook::embedded()?,
    )?;
    assert_eq!(
        origin.instruction,
        initialize
            .code
            .iter()
            .position(|instruction| instruction.command == "cc_create")
            .unwrap()
    );
    // The first traversal establishes top-level bounds. The following redraw
    // promotes those bounds into the renderer's requested regions.
    ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
    let painted = ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
    assert!(
        !painted.paint.quads.is_empty(),
        "authored text must reach the painter"
    );
    let at = [
        i32::from(WIDTH) / CENTRE_DIVISOR,
        i32::from(HEIGHT) / CENTRE_DIVISOR,
    ];
    ui.engine.platform.pending_mouse = at;
    clock.tick(&mut game, &mut ui)?;
    ui.input.click = Some(at);
    ui.input.left_held = true;
    ui.input.event = Some(crate::ui_defaults::MouseEvent {
        pos: at,
        action: LEFT_BUTTON_ACTION,
        count: CLICK_COUNT,
    });
    clock.tick(&mut game, &mut ui)?;
    ui.input.left_held = false;
    clock.tick(&mut game, &mut ui)?;
    assert_eq!(text(&mut ui)?, format!("{CLICKED_TEXT}null"));
    assert!(ui
        .diagnostics
        .executions
        .iter()
        .all(|execution| execution["ok"] != false));

    // Constructed nested children exercise the production traversal separately
    // from donor constructor defaults, which are not yet enabled for import.
    {
        use crate::ui_components::RuntimeChildId;
        use crate::ui_components::{Active, Arg, Component};
        use rs910_ui::ui_layout::{AxisRecipe, LayoutPadding, PositionConstraint, SizeConstraint};
        use std::rc::Rc;
        const CONTAINER_KEY: u16 = 200; // not a content id: synthetic runtime identity.
        const LEAF_KEY: u16 = 4097; // not a content id: synthetic runtime identity.
        const NESTED_TEXT: &str = "Nested runtime leaf";
        let root_component = ui.store.get(address(ROOT_FILE), NO_PARENT)?.unwrap();
        let static_label = ui.store.get(address(LABEL_FILE), NO_PARENT)?.unwrap();
        let interface = ui.store.interfaces[&(group as i32)].clone();
        ui.store.clear_runtime_children(address(ROOT_FILE))?;
        static_label.borrow_mut().f.hide = true;
        static_label.borrow_mut().f.text = Some(NESTED_TEXT.encode_utf16().collect());
        let mut active = Active::default();
        let recording: serde_json::Value = serde_json::from_str(include_str!(
            "../../crates/rs910-ui/fixtures/anchored-layout.json"
        ))?;
        let case = |kind: &str, name: &str| {
            recording[kind]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["name"] == name)
                .unwrap()
        };
        let positioning = case("position", "replay_padded_center");
        const AXIS_COUNT: usize = 2;
        let integers = |row: &serde_json::Value, field: &str| -> [i32; AXIS_COUNT] {
            std::array::from_fn(|axis| i32::try_from(row[field][axis].as_i64().unwrap()).unwrap())
        };
        let mut container = Component::default();
        [container.f.wsize, container.f.hsize] = integers(positioning, "parent");
        container.layout_padding = Some(LayoutPadding::from_edges(std::array::from_fn(|index| {
            u8::try_from(positioning["parent_padding"][index].as_u64().unwrap()).unwrap()
        })));
        ui.store.attach_runtime_child(
            &mut active,
            &interface,
            &root_component,
            container,
            RuntimeChildId::new(CONTAINER_KEY)?,
        )?;
        let container = active.component.clone().unwrap();
        let mut nested_label = Component::decode(
            address(ROOT_FILE),
            &interface::encode_component(&label, address(ROOT_FILE))?,
        )?;
        nested_label.f.text = Some(NESTED_TEXT.encode_utf16().collect());
        let mode_table: serde_json::Value =
            serde_json::from_str(include_str!("../../../../revisions/950/cs2/layout.json"))?;
        let recipes = |kind: &str| -> Vec<AxisRecipe> {
            mode_table[kind]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    let float = |field| {
                        f32::from_bits(u32::try_from(row[field].as_u64().unwrap()).unwrap())
                    };
                    AxisRecipe {
                        pixel_scale: i32::try_from(row["pixel_scale"].as_i64().unwrap()).unwrap(),
                        fraction_scale: float("fraction_scale_bits"),
                        fraction_bias: float("fraction_bias_bits"),
                        fraction_subtract: row["fraction_subtract"].as_bool().unwrap(),
                        element_fraction: float("element_fraction_bits"),
                    }
                })
                .collect()
        };
        let sizing_mode = case("size_modes", "replay");
        let positioning_mode = case("position_modes", "replay_center");
        let mut size = SizeConstraint::default();
        size.set_modes(
            &recipes("size"),
            integers(sizing_mode, "input"),
            integers(sizing_mode, "modes"),
        )?;
        nested_label.size_constraint = Some(size);
        nested_label.position_constraint = Some(PositionConstraint::from_modes(
            &recipes("position"),
            integers(positioning_mode, "input"),
            integers(positioning_mode, "modes"),
        )?);
        nested_label
            .hooks
            .insert("onclick", vec![Arg::Int(click_id as i32), Arg::Null]);
        ui.store.attach_runtime_child(
            &mut active,
            &interface,
            &container,
            nested_label,
            RuntimeChildId::new(LEAF_KEY)?,
        )?;
        let nested_label = active.component.clone().unwrap();
        assert!(Rc::ptr_eq(
            &root_component
                .borrow()
                .runtime_child(RuntimeChildId::new(LEAF_KEY)?)
                .unwrap(),
            &nested_label
        ));
        ui.state
            .layout
            .interface(&mut ui.store, group as i32, CANVAS, false)?;
        assert!(Rc::ptr_eq(
            &ui.state
                .layout
                .parent(&mut ui.store, &interface, &nested_label)?
                .unwrap(),
            &container
        ));
        {
            let label = nested_label.borrow();
            assert_eq!(
                [label.f.width, label.f.height],
                integers(positioning, "size")
            );
            assert_eq!([label.f.x, label.f.y], integers(positioning, "position"));
        }
        clock.tick(&mut game, &mut ui)?;
        ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        assert!(!ui
            .paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?
            .paint
            .quads
            .is_empty());
        let executions_before_click = ui.diagnostics.executions.len();
        ui.engine.platform.pending_mouse = at;
        ui.input.click = Some(at);
        ui.input.left_held = true;
        ui.input.event = Some(crate::ui_defaults::MouseEvent {
            pos: at,
            action: LEFT_BUTTON_ACTION,
            count: CLICK_COUNT,
        });
        clock.tick(&mut game, &mut ui)?;
        ui.input.left_held = false;
        clock.tick(&mut game, &mut ui)?;
        assert!(ui.diagnostics.executions.len() > executions_before_click);
        assert!(ui.diagnostics.executions.last().unwrap()["ok"] != false);
        assert_eq!(text(&mut ui)?, format!("{CLICKED_TEXT}null"));
        assert!(crate::ui_hooks::attached(&mut ui.store, &nested_label)?);
        // Entry visibility reaches the same drawing and pointer consumers. The
        // constructor flag stays clear, so this proves the parent-owned state.
        ui.store.set_runtime_child_hidden(&nested_label, true)?;
        assert!(!nested_label.borrow().f.hide);
        static_label.borrow_mut().f.text = Some(NESTED_TEXT.encode_utf16().collect());
        clock.tick(&mut game, &mut ui)?;
        ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        assert!(ui
            .paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?
            .paint
            .quads
            .is_empty());
        let executions_before_hidden_click = ui.diagnostics.executions.len();
        ui.input.click = Some(at);
        ui.input.left_held = true;
        ui.input.event = Some(crate::ui_defaults::MouseEvent {
            pos: at,
            action: LEFT_BUTTON_ACTION,
            count: CLICK_COUNT,
        });
        clock.tick(&mut game, &mut ui)?;
        ui.input.left_held = false;
        clock.tick(&mut game, &mut ui)?;
        assert_eq!(
            ui.diagnostics.executions.len(),
            executions_before_hidden_click
        );
        assert_eq!(text(&mut ui)?, NESTED_TEXT);
        ui.store.set_runtime_child_hidden(&nested_label, false)?;
        clock.tick(&mut game, &mut ui)?;
        ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        assert!(!ui
            .paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?
            .paint
            .quads
            .is_empty());
        let descendants = container.borrow().children.clone().unwrap();
        ui.store.attach_runtime_child(
            &mut active,
            &interface,
            &root_component,
            Component::default(),
            RuntimeChildId::new(CONTAINER_KEY)?,
        )?;
        assert!(descendants.borrow().is_empty());
        let replacement_container = active.component.clone().unwrap();
        // A retained old reference updates its same-id replacement's entry.
        ui.store.set_runtime_child_hidden(&container, true)?;
        assert_eq!(
            replacement_container.borrow().runtime_entry_hidden(),
            Some(true)
        );
        assert!(!replacement_container.borrow().f.hide);
        assert!(ui
            .store
            .remove_runtime_child(&root_component, RuntimeChildId::new(CONTAINER_KEY)?));
        assert_eq!(container.borrow().runtime_entry_hidden(), Some(false));
        assert!(root_component
            .borrow()
            .runtime_child(RuntimeChildId::new(LEAF_KEY)?)
            .is_none());
        assert!(!crate::ui_hooks::attached(&mut ui.store, &nested_label)?);
        clock.tick(&mut game, &mut ui)?;
        ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        assert!(ui
            .paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?
            .paint
            .quads
            .is_empty());
    }

    drop(ui);
    drop(game);
    let imported = ui_imported_replay::prepare(ui_imported_replay::Frame {
        pack_root: &output,
        directory: &directory,
        group,
        root_file: ROOT_FILE,
        root: frame,
        label_file: LABEL_FILE,
        label,
        setup_script_id: script_id,
    })?;
    let pack = crate::cache::Pack::open_with_overlay(&imported.pack_root, None);
    let configs = rs910_config::login_configs::LoginConfigs::read(&pack);
    let mut game = crate::client_game::ClientGame::login_with(
        &pack,
        &configs,
        LOCAL_PLAYER_SLOT,
        crate::protocol910::live::Feed::default(),
        REPLAY_SEED,
        true,
    )?;
    let mut ui = crate::ui_runtime::Runtime::new_with(pack.clone(), configs)?;
    ui.resize(CANVAS)?;
    ui.diagnostics.capture = true;
    ui.target.quiet = true;
    let mut clock = crate::test_support::SimClock::default();
    if imported.retained_long_stack {
        ui.pool.contexts.push(crate::ui_hooks::ExecutionContext {
            longs: vec![RETAINED_LONG_VALUE; CS2_STACK_CAPACITY],
            ..Default::default()
        });
    }
    clock.with(&mut game, |vars| ui.packet(vars, &event))?;
    clock.tick(&mut game, &mut ui)?;
    let operation_label = |ui: &mut crate::ui_runtime::Runtime| -> anyhow::Result<Option<String>> {
        let component = ui.store.get(address(ROOT_FILE), NO_PARENT)?.unwrap();
        let value = component
            .borrow()
            .ops
            .as_ref()
            .and_then(|operations| operations.first())
            .and_then(Option::as_ref)
            .map(|label| String::from_utf16_lossy(label));
        Ok(value)
    };
    let imported_child = ui
        .store
        .get(address(ROOT_FILE), FIRST_DYNAMIC_CHILD)?
        .expect("native setup creates runtime child before imported hook");
    let imported_children = ui
        .store
        .get(address(ROOT_FILE), NO_PARENT)?
        .unwrap()
        .borrow()
        .children
        .clone()
        .unwrap();
    assert_eq!(text(&mut ui)?, imported.active_text);
    ui_imported_replay::replay_palette(
        &imported.palette,
        &mut game,
        &mut ui,
        &mut clock,
        address(LABEL_FILE),
        &directory,
    )?;
    ui_imported_replay::replay_colour_helper(
        &imported.palette,
        &mut game,
        &mut ui,
        &mut clock,
        address(ROOT_FILE),
        &directory,
    )?;
    // The imported selector reaches a child created by the ordinary setup
    // script. Reuse the frame's text style for the isolated drawing observation.
    {
        const SELECTED_COLOUR: i32 = 0x0034_a878;
        const MISSED_COLOUR: i32 = 0x0078_34a8;
        const MISSING_CHILD: i32 = FIRST_DYNAMIC_CHILD + 1;
        let static_label = ui.store.get(address(LABEL_FILE), NO_PARENT)?.unwrap();
        let child_fields = imported_child.borrow().f.clone();
        let static_hidden = static_label.borrow().f.hide;
        {
            let mut child = imported_child.borrow_mut();
            child.f = static_label.borrow().f.clone();
            child.f.id = child_fields.id;
            child.f.parentlayer = child_fields.parentlayer;
            child.f.layer = child_fields.layer;
        }
        static_label.borrow_mut().f.hide = true;
        ui_imported_replay::replay_flat_child(
            imported.flat_child_script,
            FIRST_DYNAMIC_CHILD,
            SELECTED_COLOUR,
            &mut game,
            &mut ui,
            &mut clock,
        )?;
        assert_eq!(imported_child.borrow().f.colour, SELECTED_COLOUR);
        ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        assert!(!ui
            .paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?
            .paint
            .quads
            .is_empty());
        ui_imported_replay::replay_flat_child(
            imported.flat_child_script,
            MISSING_CHILD,
            MISSED_COLOUR,
            &mut game,
            &mut ui,
            &mut clock,
        )?;
        assert_eq!(imported_child.borrow().f.colour, SELECTED_COLOUR);
        assert!(ui
            .diagnostics
            .executions
            .iter()
            .filter(|row| row["script"] == imported.flat_child_script)
            .all(|row| row["ok"] != false));
        std::fs::write(
            directory.join("flat-child-observations.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"script":imported.flat_child_script,"found_child":FIRST_DYNAMIC_CHILD,"missed_child":MISSING_CHILD,"actual_colour":imported_child.borrow().f.colour,"painted":true}),
            )?,
        )?;
        ui_imported_replay::replay_retained_hook(
            &imported,
            &imported_child,
            &mut game,
            &mut ui,
            &mut clock,
            &directory,
        )?;
        imported_child.borrow_mut().f = child_fields;
        static_label.borrow_mut().f.hide = static_hidden;
    }
    let active_operation = operation_label(&mut ui)?;
    if let Some(expected) = &imported.operation_labels {
        assert_eq!(active_operation.as_deref(), Some(expected[0].as_str()));
    }
    ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
    assert!(!ui
        .paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?
        .paint
        .quads
        .is_empty());
    ui.engine.platform.pending_mouse = at;
    clock.tick(&mut game, &mut ui)?;
    ui.input.click = Some(at);
    ui.input.left_held = true;
    ui.input.event = Some(crate::ui_defaults::MouseEvent {
        pos: at,
        action: LEFT_BUTTON_ACTION,
        count: CLICK_COUNT,
    });
    clock.tick(&mut game, &mut ui)?;
    ui.input.left_held = false;
    clock.tick(&mut game, &mut ui)?;
    assert_eq!(
        text(&mut ui)?,
        imported
            .callback
            .as_ref()
            .map_or(imported.inactive_text.as_str(), |callback| callback
                .text
                .as_str())
    );
    let inactive_operation = operation_label(&mut ui)?;
    if let Some(expected) = &imported.operation_labels {
        assert_eq!(inactive_operation.as_deref(), Some(expected[1].as_str()));
    }
    if let Some(callback) = &imported.callback {
        let frame = ui.store.get(address(ROOT_FILE), NO_PARENT)?.unwrap();
        let mut expected_hook = vec![crate::ui_components::Arg::Int(callback.script)];
        expected_hook.extend(callback.arguments.clone());
        assert_eq!(frame.borrow().hooks["onop"], expected_hook);
        assert!(ui
            .store
            .get(address(ROOT_FILE), FIRST_DYNAMIC_CHILD)?
            .is_none());
        assert!(imported_children.borrow().is_empty());
        assert_eq!(imported_child.borrow().f.id, FIRST_DYNAMIC_CHILD);
        assert!(ui.store.get(address(LABEL_FILE), NO_PARENT)?.is_some());
        let completed = |id: i32| {
            ui.diagnostics
                .executions
                .iter()
                .rposition(|execution| execution["id"] == id)
                .expect("pointer invocation recorded")
        };
        assert!(completed(callback.before_script) < completed(callback.script));
        assert_eq!(
            ui.diagnostics.executions[completed(callback.before_script)]["steps"],
            callback.entry_steps
        );
        assert_eq!(
            ui.diagnostics.executions[completed(callback.script)]["steps"],
            callback.steps
        );
        assert_eq!(text(&mut ui)?, callback.text);
        assert!(frame.borrow().f.hide);
        ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        assert!(ui
            .paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?
            .paint
            .quads
            .is_empty());
    }
    assert!(ui
        .diagnostics
        .executions
        .iter()
        .all(|execution| execution["ok"] != false
            || ui_imported_replay::expected_font_binding_failure(&imported.palette, execution)));
    if imported.retained_long_stack {
        assert_eq!(
            ui.pool.contexts[0].longs,
            vec![RETAINED_LONG_VALUE; CS2_STACK_CAPACITY]
        );
    }
    std::fs::write(
        directory.join("modern-ui-observations.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "active_text": imported.active_text, "inactive_text": imported.inactive_text,
            "active_operation": active_operation, "inactive_operation": inactive_operation,
            "callback": imported.callback.as_ref().map(|callback| serde_json::json!({ "script": callback.script, "text": callback.text, "entry_steps": callback.entry_steps, "steps": callback.steps, "runtime_children_empty": imported_children.borrow().is_empty(), "static_label_present": true })),
            "retained_long_slots": ui.pool.contexts.first().map(|context| context.longs.len()),
            "executions": ui.diagnostics.executions,
        }))?,
    )?;
    Ok(())
}
