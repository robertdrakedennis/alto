//! One live owner for component operations, target selection and the
//! interface drag lifecycle.
use crate::{
    ui_components::{Component, Ref, Store, Text},
    ui_draw::Drag,
    ui_hooks::{Executor, Request, INTERACTIVE_LIMIT},
    ui_properties::State,
};
use anyhow::{Context, Result};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
#[cfg(test)]
#[path = "ui_action_bar_drag_tests.rs"]
mod action_bar_drag_tests;
#[cfg(test)]
#[path = "ui_interaction_tests.rs"]
mod tests;

#[derive(Clone)]
pub enum Action {
    Op {
        op: i32,
        parent: i32,
        child: i32,
        base: Option<Text>,
    },
    /// Map-element menu actions are
    /// resolved through the trigger-script owner after menu input returns.
    MapElementTrigger {
        trigger: i32,
        element: i32,
        category: i32,
        /// Mouse coordinates are present for the original client's 15-17 hover triggers;
        /// menu operations use the element-only local layout.
        mouse: Option<[i32; 2]>,
    },
    Select {
        parent: i32,
        child: i32,
    },
    Target {
        parent: i32,
        child: i32,
    },
    ClearTarget,
    Pause {
        parent: i32,
        child: i32,
    },
}

#[derive(Clone, Default)]
pub struct Target {
    pub active: bool,
    pub parent: i32,
    pub child: i32,
    pub object: i32,
    pub mask: i32,
    pub param: i32,
    pub cursor: i32,
    #[allow(dead_code, reason = "recorded default cursor; no reader yet")]
    pub default_cursor: i32,
    pub verb: String,
    pub name: String,
}

pub struct Interaction {
    pub drag: Drag,
    pub drag_cycle: i32,
    pub drag_ready: bool,
    pub drag_origin: [i32; 2],
    pub drop_target: Option<Ref>,
    pub target: Target,
    /// Menu actions dispatched after traversal and the drag hooks.
    pub actions: VecDeque<Action>,
    /// Commands issued by the currently executing VM instruction. Menu
    /// follow-up actions must wait until the whole operation hook returns.
    pub script_actions: VecDeque<Action>,
    pub outgoing: Vec<u8>,
}
impl Default for Interaction {
    fn default() -> Self {
        Self {
            drag: Drag {
                default_layer: Some(Rc::new(RefCell::new(Component::default()))),
                ..Default::default()
            },
            drag_cycle: 0,
            drag_ready: false,
            drag_origin: [0; 2],
            drop_target: None,
            target: Target::default(),
            actions: VecDeque::new(),
            script_actions: VecDeque::new(),
            outgoing: vec![],
        }
    }
}
pub fn same(a: &Option<Ref>, b: &Option<Ref>) -> bool {
    a.as_ref()
        .zip(b.as_ref())
        .is_some_and(|(a, b)| Rc::ptr_eq(a, b))
}
/// isOpen: container queries include descendant entries.
pub fn menu_open_for(store: &mut Store, state: &State, parent: i32, child: i32) -> Result<bool> {
    if !state.minimenu.open {
        return Ok(false);
    }
    let Some(component) = store.get(parent, -1)? else {
        return Ok(false);
    };
    for &id in &state.minimenu.entries {
        let entry = state.minimenu.entry(id);
        if !crate::ui_minimenu::is_interface_action(entry.action) {
            continue;
        }
        if child == -1 && component.borrow().f.r#type == 0 {
            let mut current = store.get(entry.tile_z, -1)?;
            while let Some(c) = current {
                if c.borrow().f.parentlayer == parent {
                    return Ok(true);
                }
                let interface = store
                    .interfaces
                    .get(&(c.borrow().f.parentlayer >> 16))
                    .context("menu interface")?
                    .clone();
                current = state.layout.parent(store, &interface, &c)?;
            }
        } else if entry.tile_x == child && entry.tile_z == parent {
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn draggable(state: &State, c: &Ref) -> bool {
    let mask = state.layout.active_mask(c);
    mask >> 18 & 7 != 0 || mask >> 23 & 1 != 0 || c.borrow().draggable.is_some()
}
pub fn resolve_layer(store: &mut Store, state: &State, c: &Ref) -> Result<Option<Ref>> {
    let mask = state.layout.active_mask(c);
    let default = &state.interaction.drag.default_layer;
    if mask >> 23 & 1 != 0 {
        return Ok(default.clone());
    }
    let depth = mask >> 18 & 7;
    if depth == 0 {
        return Ok(None);
    }
    let mut current = c.clone();
    for _ in 0..depth {
        let id = current.borrow().f.parentlayer >> 16;
        let interface = store.interfaces.get(&id).context("drag interface")?.clone();
        let Some(parent) = state.layout.parent(store, &interface, &current)? else {
            return Ok(default.clone());
        };
        current = parent;
    }
    Ok(Some(current))
}
pub fn pickup(store: &mut Store, state: &mut State, c: Option<Ref>, press: [i32; 2]) -> Result<()> {
    if state.interaction.drag.component.is_some() || state.minimenu.open {
        return Ok(());
    }
    let Some(c) = c.filter(|c| draggable(state, c)) else {
        return Ok(());
    };
    let layer = resolve_layer(store, state, &c)?.or_else(|| c.borrow().draggable.clone());
    let d = &mut state.interaction;
    d.drag.component = Some(c);
    d.drag.layer = layer;
    d.drag.press = press;
    d.drag.active = false;
    d.drag_cycle = 0;
    Ok(())
}
pub fn cancel_drag(state: &mut State) {
    state.interaction.drag.component = None;
    state.interaction.drag.layer = None;
}
pub fn begin_cycle(state: &mut State, mouse: [i32; 2]) {
    let d = &mut state.interaction;
    d.drop_target = None;
    d.drag_ready = false;
    d.drag.parent_ready = false;
    d.drag.mouse = mouse;
}
fn hook(
    store: &mut Store,
    state: &mut State,
    exec: &mut (impl Executor + ?Sized),
    c: &Ref,
    name: &str,
    mut r: Request,
) -> Result<()> {
    let args = c.borrow().hooks.get(name).cloned();
    if let Some(args) = args {
        r.args = Some(args);
        r.component = Some(c.clone());
        exec.run(store, state, r, INTERACTIVE_LIMIT)?;
    }
    Ok(())
}
/// Packet alternate byte orders, shared by the interface writers.
pub(crate) fn p2(out: &mut Vec<u8>, v: i32, alt: u8) {
    let hi = (v >> 8) as u8;
    let lo = v as u8;
    out.extend(match alt {
        1 => [lo, hi],
        2 => [hi, lo.wrapping_add(128)],
        3 => [lo.wrapping_add(128), hi],
        _ => [hi, lo],
    });
}
pub(crate) fn p4(out: &mut Vec<u8>, v: i32, alt: u8) {
    let [a, b, c, d] = v.to_be_bytes();
    out.extend(match alt {
        1 => [d, c, b, a],
        2 => [c, d, a, b],
        3 => [b, a, d, c],
        _ => [a, b, c, d],
    });
}
pub fn button_packet(op: i32, parent: i32, child: i32, object: i32) -> Vec<u8> {
    use crate::proto::client::*;
    let ids = [
        IF_BUTTON1,
        IF_BUTTON2,
        IF_BUTTON3,
        IF_BUTTON4,
        IF_BUTTON5,
        IF_BUTTON6,
        IF_BUTTON7,
        IF_BUTTON8,
        IF_BUTTON9,
        IF_BUTTON10,
    ];
    let Some(&id) = usize::try_from(op - 1).ok().and_then(|i| ids.get(i)) else {
        return vec![];
    };
    let mut out = vec![id];
    p2(&mut out, object, 3);
    p2(&mut out, child, 2);
    p4(&mut out, parent, 0);
    out
}
/// `triggerOp`'s linked-component branch (`IF_PLAYER`).
pub fn if_player_packet(op: i32, parent: i32, child: i32, link: &[u16], group_kind: u8) -> Vec<u8> {
    let mut name = link
        .iter()
        .map(|&unit| crate::ui_dialogue::cp1252_encode_unit(unit))
        .collect::<Vec<_>>();
    name.push(0);
    let body_len = name.len() + 8;
    let mut out = vec![crate::proto::client::IF_PLAYER, body_len as u8];
    out.extend(name);
    p2(&mut out, child, 1);
    out.push(0u8.wrapping_sub(op as u8));
    out.push(128u8.wrapping_sub(group_kind));
    p4(&mut out, parent, 2);
    out
}
pub fn drag_packet(source: &Ref, target: &Ref) -> Vec<u8> {
    let s = &source.borrow().f;
    let t = &target.borrow().f;
    let mut out = vec![crate::proto::client::IF_BUTTOND];
    p2(&mut out, t.id, 0);
    p4(&mut out, s.parentlayer, 3);
    p2(&mut out, t.invobject, 1);
    p2(&mut out, s.invobject, 3);
    p4(&mut out, t.parentlayer, 3);
    p2(&mut out, s.id, 1);
    out
}
pub fn dispatch(
    store: &mut Store,
    state: &mut State,
    exec: &mut (impl Executor + ?Sized),
    action: Action,
) -> Result<()> {
    match action {
        Action::Op {
            op,
            parent,
            child,
            base,
        } => {
            let Some(c) = store.get(parent, child)? else {
                return Ok(());
            };
            hook(
                store,
                state,
                exec,
                &c,
                "onop",
                Request {
                    opindex: op,
                    opbase: base,
                    ..Default::default()
                },
            )?;
            // Hooks run before the mask is re-read; they may change both the
            // server permission and the inventory object written to the packet.
            if crate::ui_debug_flags::flags().ui_trace_input {
                log::info!(
                    "[ui-input] operation after hook mask={} object={}",
                    state.layout.active_mask(&c),
                    c.borrow().f.invobject
                );
            }
            if crate::ui_minimenu::mask_has_op(state.layout.active_mask(&c), op - 1) {
                let component = c.borrow();
                let f = &component.f;
                if let Some(link) = f.link.as_deref() {
                    let kind = component
                        .group_kind
                        .ok_or_else(|| anyhow::anyhow!("linked component group kind missing"))?;
                    state
                        .interaction
                        .outgoing
                        .extend(if_player_packet(op, parent, child, link, kind));
                } else {
                    state.interaction.outgoing.extend(button_packet(
                        op,
                        parent,
                        child,
                        f.invobject,
                    ));
                }
            }
        }
        Action::MapElementTrigger { .. } => {
            // The Runtime resolves this action through the JS5 trigger
            // provider. Keeping it in the ordinary action queue preserves
            // The original client's ordering: useMenuOption queues no packet, then the
            // trigger runs before the next client tick flush.
        }
        Action::ClearTarget => {
            // clearTargetMode: no-op when inactive; null
            // component still clears active/object/cursor state.
            if !state.interaction.target.active {
                return Ok(());
            }
            let t = state.interaction.target.clone();
            if let Some(c) = store.get(t.parent, t.child)? {
                hook(store, state, exec, &c, "ontargetleave", Request::default())?;
                store.updated.push(c);
            }
            state.interaction.target.active = false;
            state.interaction.target.object = -1;
            state.minimenu.default_cursor = -1;
        }
        Action::Select { parent, child } => {
            // useMenuOption Select 25: null component no-op,
            // then clearTargetMode before getActive.
            let Some(c) = store.get(parent, child)? else {
                return Ok(());
            };
            dispatch(store, state, exec, Action::ClearTarget)?;
            let mask = state.layout.active_mask(&c);
            let param = state
                .layout
                .active_params
                .get(&((i64::from(parent) << 32).wrapping_add(i64::from(child))))
                .copied()
                .unwrap_or(c.borrow().default_active[1]);
            // setTargetActiveComponent runs ontargetenter
            // before setting active state.
            hook(store, state, exec, &c, "ontargetenter", Request::default())?;
            let f = &c.borrow().f;
            // The target verb verbatim: mask 0 -> null -> minimenu "Null"; blank
            // verb -> hidden_ops "Hidden-use" else null -> "Null". The component
            // cursor -1 rules (65535 -> -1) already hold in the wire decode; -1
            // survives here verbatim.
            let target_mask = crate::ui_minimenu::mask_target(mask);
            let raw = f
                .targetverb
                .as_ref()
                .map(|s| String::from_utf16_lossy(s))
                .filter(|s| !s.trim().is_empty());
            let verb = if target_mask == 0 {
                "Null".into()
            } else {
                match raw {
                    Some(v) => v,
                    None if state.minimenu.hidden_ops => "Hidden-use".into(),
                    None => "Null".into(),
                }
            };
            state.interaction.target = Target {
                active: true,
                parent: f.parentlayer,
                child: f.id,
                object: f.invobject,
                mask: target_mask,
                param,
                cursor: f.targetCursor,
                default_cursor: f.targetDefaultCursor,
                verb,
                // MiniMenu targetName = opbase + colTag(16777215);
                // colTag is "<col=" + hex + ">" so white is "<col=ffffff>";
                // original null opbase concatenates as "null".
                name: format!(
                    "{}<col=ffffff>",
                    f.opbase
                        .as_ref()
                        .map(|s| String::from_utf16_lossy(s))
                        .unwrap_or_else(|| "null".into())
                ),
            };
            state.minimenu.default_cursor = f.targetDefaultCursor;
            store.updated.push(c.clone());
        }
        Action::Target { parent, child } => {
            if !state.interaction.target.active {
                return Ok(());
            }
            let Some(c) = store.get(parent, child)? else {
                return Ok(());
            };
            let t = state.interaction.target.clone();
            if let Some(source) = store.get(t.parent, t.child)? {
                hook(
                    store,
                    state,
                    exec,
                    &c,
                    "onopt",
                    Request {
                        drop: Some(source),
                        ..Default::default()
                    },
                )?;
            }
            let t = &state.interaction.target;
            let f = &c.borrow().f;
            let mut out = vec![crate::proto::client::IF_BUTTONT];
            p4(&mut out, f.parentlayer, 1);
            p2(&mut out, f.invobject, 0);
            p2(&mut out, t.object, 2);
            p2(&mut out, f.id, 2);
            p2(&mut out, t.child, 2);
            p4(&mut out, t.parent, 1);
            state.interaction.outgoing.extend(out);
        }
        Action::Pause { parent, child } => {
            if state.life.pressed_continue.is_none() {
                let mut out = vec![crate::proto::client::RESUME_PAUSEBUTTON];
                p4(&mut out, parent, 3);
                p2(&mut out, child, 2);
                state.interaction.outgoing.extend(out);
                state.life.pressed_continue = store.get(parent, child)?;
                if let Some(c) = &state.life.pressed_continue {
                    store.updated.push(c.clone());
                }
            }
        }
    }
    Ok(())
}
/// loopIf3Drag: called after the three hook queues, before update.
/// Returns the press location when a non-drag release should replay pending menu input.
pub fn finish_drag(
    store: &mut Store,
    state: &mut State,
    exec: &mut (impl Executor + ?Sized),
    held: bool,
) -> Result<Option<[i32; 2]>> {
    let Some(c) = state.interaction.drag.component.clone() else {
        return Ok(None);
    };
    store.updated.push(c.clone());
    state.interaction.drag_cycle = state.interaction.drag_cycle.wrapping_add(1);
    let d = &state.interaction;
    if !d.drag_ready || !d.drag.parent_ready {
        if d.drag_cycle > 1 {
            cancel_drag(state);
        }
        return Ok(None);
    }
    let (width, height, deadtime, deadzone) = {
        let f = &c.borrow().f;
        (f.width, f.height, f.dragdeadtime, f.dragdeadzone)
    };
    let b = d.drag.bounds;
    let mut xy = [
        d.drag.mouse[0] - d.drag.press[0],
        d.drag.mouse[1] - d.drag.press[1],
    ];
    xy[0] = xy[0].max(b[0]);
    if xy[0] + width > b[0] + b[2] {
        xy[0] = b[0] + b[2] - width;
    }
    xy[1] = xy[1].max(b[1]);
    if xy[1] + height > b[1] + b[3] {
        xy[1] = b[1] + b[3] - height;
    }
    let local = if same(&d.drag.layer, &d.drag.default_layer) {
        xy
    } else {
        let layer = d.drag.layer.as_ref().context("drag layer")?.borrow();
        [
            layer.f.scrollx + xy[0] - b[0],
            layer.f.scrolly + xy[1] - b[1],
        ]
    };
    if held {
        if d.drag_cycle > deadtime
            && (xy[0] - d.drag_origin[0])
                .abs()
                .max((xy[1] - d.drag_origin[1]).abs())
                > deadzone
        {
            state.interaction.drag.active = true;
        }
        if state.interaction.drag.active {
            hook(
                store,
                state,
                exec,
                &c,
                "ondrag",
                Request {
                    mouse: local,
                    ..Default::default()
                },
            )?;
        }
        return Ok(None);
    }
    let press = [
        d.drag_origin[0] + d.drag.press[0],
        d.drag_origin[1] + d.drag.press[1],
    ];
    let active = d.drag.active;
    if active {
        dispatch(store, state, exec, Action::ClearTarget)?;
        let parent = resolve_layer(store, state, &c)?;
        let mut target = state.interaction.drop_target.clone();
        let mut ancestor = false;
        while let (Some(t), Some(p)) = (&target, &parent) {
            if t.borrow().f.layer == -1 {
                break;
            }
            if p.borrow().f.parentlayer == t.borrow().f.parentlayer {
                ancestor = true;
                break;
            }
            let layer = t.borrow().f.layer;
            target = store.get(layer, -1)?;
        }
        let drop = if target.is_none()
            || parent.is_none()
            || same(&parent, &state.interaction.drag.default_layer)
            || ancestor
        {
            state.interaction.drop_target.clone()
        } else {
            state.interaction.drag.default_layer.clone()
        };
        hook(
            store,
            state,
            exec,
            &c,
            "ondragcomplete",
            Request {
                mouse: local,
                drop,
                ..Default::default()
            },
        )?;
        // Resolve again after synchronous hooks, exactly as the original client does.
        if let Some(target) = state.interaction.drop_target.clone() {
            if resolve_layer(store, state, &c)?.is_some() {
                state.interaction.outgoing.extend(drag_packet(&c, &target));
            }
        }
    }
    cancel_drag(state);
    Ok((!active).then_some(press))
}
