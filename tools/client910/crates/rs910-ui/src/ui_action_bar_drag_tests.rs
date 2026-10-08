use super::*;
use crate::{
    ui_components::Interface,
    ui_draw::Frame,
    ui_loop::{Input, Transmits},
};
use rs910_symbols::{component, interface, ComponentId};

const CANVAS: [i32; 2] = [200, 120];
const MAIN_ORIGIN: [i32; 2] = [10, 20];
const EXTRA_ORIGIN: [i32; 2] = [100, 50];
const SLOT_SIZE: [i32; 2] = [30, 30];
const PRESS: [i32; 2] = [15, 25];
const EXTRA_DROP: [i32; 2] = [110, 60];
const SCENE_DROP: [i32; 2] = [180, 100];
const STATIC_CHILD: i32 = -1;
const NO_GRAPHIC: i32 = -1;
const ROOT_LAYER: i32 = -1;
const LAYER_KIND: i32 = 0;
const OVERLAY_KIND: i32 = 1;
const GRAPHIC_KIND: i32 = 5;
const NO_DRAG_DELAY: i32 = 0;
const NO_DRAG_DISTANCE: i32 = 0;
const ZERO_ORIGIN: [i32; 2] = [0, 0];
const X_AXIS: usize = 0;
const Y_AXIS: usize = 1;
const EVENT_MASK_INDEX: usize = 0;
const FIRST_OPERATION_EVENT: i32 = 2;
const DROP_TARGET_EVENT: i32 = 1 << 21;
const FIRST_ENTRY: usize = 0;
const ENTRY_COUNT: usize = 1;

struct NoHooks;
impl Executor for NoHooks {
    fn run(&mut self, _: &mut Store, _: &mut State, _: Request, _: usize) -> Result<()> {
        Ok(())
    }
}

fn component(id: ComponentId, origin: [i32; 2], size: [i32; 2], kind: i32) -> Ref {
    let value = Rc::new(RefCell::new(Component::default()));
    {
        let mut value = value.borrow_mut();
        value.f.parentlayer = id.packed();
        value.f.id = STATIC_CHILD;
        value.f.layer = ROOT_LAYER;
        value.f.x = origin[X_AXIS];
        value.f.y = origin[Y_AXIS];
        value.f.width = size[X_AXIS];
        value.f.height = size[Y_AXIS];
        value.f.r#type = kind;
        value.f.hashook = true;
        value.f.graphic = NO_GRAPHIC;
        value.f.clickmask = false;
        value.f.dragdeadtime = NO_DRAG_DELAY;
        value.f.dragdeadzone = NO_DRAG_DISTANCE;
    }
    value
}

fn mount(store: &mut Store, group: i32, components: &[Ref]) {
    let last = components
        .iter()
        .map(|c| (c.borrow().f.parentlayer & u16::MAX as i32) as usize)
        .max()
        .unwrap();
    let mut entries = vec![None; last + ENTRY_COUNT];
    // Store uses the first entry as a group marker. This fixture has sparse
    // named slots rather than the entire cache interface; keep the marker hidden.
    let mut marker = components[FIRST_ENTRY].borrow().clone();
    marker.f.hide = true;
    entries[FIRST_ENTRY] = Some(Rc::new(RefCell::new(marker)));
    for component in components {
        let slot = (component.borrow().f.parentlayer & u16::MAX as i32) as usize;
        entries[slot] = Some(component.clone());
    }
    store.interfaces.insert(group, Interface::new(entries));
}

fn cycle(
    store: &mut Store,
    state: &mut State,
    frame: &mut Frame,
    mouse: [i32; 2],
    held: bool,
    click: bool,
) {
    begin_cycle(state, mouse);
    crate::ui_loop::update_top_level(
        store,
        state,
        frame,
        &Input {
            mouse,
            left_held: held,
            click: click.then_some(mouse),
            ..Input::default()
        },
        &Transmits::default(),
    )
    .unwrap();
    finish_drag(store, state, &mut NoHooks, held).unwrap();
}

#[test]
fn recorded_bar_permissions_allow_cross_window_drag_and_scene_discard_packets() {
    // Server login event payloads recorded once over TCP. Expected client
    // packets were encoded independently by the server socket test helper.
    // Geometry is a small mounted graph; native CS2 execution remains to be proved
    // by the required combined gameplay replay.
    let recording: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/action-bar-drag-events.json")).unwrap();
    let main = component(
        component::game_window::ACTION_BAR_SLOT,
        MAIN_ORIGIN,
        SLOT_SIZE,
        LAYER_KIND,
    );
    let extra = component(
        component::game_window::ACTION_BAR_SECONDARY_SLOT,
        EXTRA_ORIGIN,
        SLOT_SIZE,
        LAYER_KIND,
    );
    let discard = component(
        component::game_window::ACTION_BAR_DISCARD_SCENE,
        ZERO_ORIGIN,
        CANVAS,
        LAYER_KIND,
    );
    let source = component(
        component::action_bar::SLICE_SLOT,
        ZERO_ORIGIN,
        SLOT_SIZE,
        GRAPHIC_KIND,
    );
    let target = component(
        component::action_bar_secondary::SLOT_1,
        ZERO_ORIGIN,
        SLOT_SIZE,
        GRAPHIC_KIND,
    );
    source.borrow_mut().default_active[EVENT_MASK_INDEX] =
        FIRST_OPERATION_EVENT | DROP_TARGET_EVENT;
    let mut store = Store::default();
    mount(
        &mut store,
        interface::GAME_WINDOW.id(),
        &[discard.clone(), main, extra],
    );
    mount(
        &mut store,
        interface::ACTION_BAR.id(),
        std::slice::from_ref(&source),
    );
    mount(
        &mut store,
        interface::ACTION_BAR_SECONDARY.id(),
        std::slice::from_ref(&target),
    );
    let mut state = State::default();
    state.life.top = interface::GAME_WINDOW.id();
    state.layout.canvas = CANVAS;
    state.layout.subs = vec![
        (
            component::game_window::ACTION_BAR_SLOT.packed(),
            interface::ACTION_BAR.id(),
        ),
        (
            component::game_window::ACTION_BAR_SECONDARY_SLOT.packed(),
            interface::ACTION_BAR_SECONDARY.id(),
        ),
    ];
    for &(parent, group) in &state.layout.subs {
        state.life.subs.put(
            crate::ui_lifecycle::Sub::new(group, OVERLAY_KIND, None),
            parent,
        );
    }
    assert!(
        !draggable(&state, &source),
        "cached drop-only permission cannot start a source drag"
    );
    for (payload, expected) in recording["events"].as_array().unwrap().iter().zip([
        component::action_bar::SLICE_SLOT,
        component::action_bar_secondary::SLOT_1,
        component::game_window::ACTION_BAR_DISCARD_SCENE,
    ]) {
        let payload: Vec<u8> = serde_json::from_value(payload.clone()).unwrap();
        let event =
            crate::server_prot::parse_ui_event(crate::proto::server::IF_SETEVENTS, &payload)
                .unwrap()
                .unwrap();
        assert!(
            matches!(event, crate::server_prot::UiEvent::SetEvents { packed, from: STATIC_CHILD, to: STATIC_CHILD, .. } if packed == expected.packed() as u32)
        );
        crate::ui_lifecycle::packet(&mut store, &mut state, &event, &mut NoHooks).unwrap();
    }
    assert!(draggable(&state, &source));
    assert!(same(
        &resolve_layer(&mut store, &state, &source).unwrap(),
        &state.interaction.drag.default_layer
    ));
    let mut frame = Frame::default();
    for (destination, expected, destination_component) in [
        (EXTRA_DROP, "crossBarPacket", target),
        (SCENE_DROP, "discardPacket", discard),
    ] {
        cycle(&mut store, &mut state, &mut frame, PRESS, true, true);
        assert!(same(
            &state.interaction.drag.component,
            &Some(source.clone())
        ));
        cycle(&mut store, &mut state, &mut frame, destination, true, false);
        assert!(state.interaction.drag.active);
        assert!(same(
            &state.interaction.drop_target,
            &Some(destination_component)
        ));
        cycle(
            &mut store,
            &mut state,
            &mut frame,
            destination,
            false,
            false,
        );
        let packet: Vec<u8> = serde_json::from_value(recording[expected].clone()).unwrap();
        assert_eq!(state.interaction.outgoing, packet);
        state.interaction.outgoing.clear();
    }
}
