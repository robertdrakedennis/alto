//! Retained interface ownership and
//! synchronous lifecycle. The eight-bucket table preserves Node identity and
//! the table iterator's cached next node across hooks which mutate the table.
use crate::{
    server_prot::{ActiveBinding, ScriptArg, UiEvent},
    ui_changes::Changes,
    ui_components::{Arg, Ref, Store},
    ui_hooks::{self as hooks, Executor, Request},
    ui_properties::State,
    ui_resources::Keys,
};
use anyhow::{Context, Result};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

pub type SubRef = Rc<RefCell<Sub>>;
#[derive(Clone)]
enum Link {
    Node(Weak<RefCell<Sub>>),
    Bucket(usize),
    End,
}
pub struct Sub {
    pub parent: i32,
    pub id: i32,
    pub kind: i32,
    pub binding: Option<ActiveBinding>,
    linked: bool,
    next: Link,
}
impl Sub {
    pub fn new(id: i32, kind: i32, binding: Option<ActiveBinding>) -> SubRef {
        Rc::new(RefCell::new(Self {
            parent: 0,
            id,
            kind,
            binding,
            linked: false,
            next: Link::End,
        }))
    }
}
#[derive(Default)]
pub struct Table {
    buckets: [Vec<SubRef>; 8],
}
impl Table {
    pub fn get(&self, parent: i32) -> Option<SubRef> {
        self.buckets[(parent as u32 & 7) as usize]
            .iter()
            .find(|n| n.borrow().parent == parent)
            .cloned()
    }
    pub fn ordered(&self) -> impl Iterator<Item = &SubRef> {
        self.buckets.iter().flatten()
    }
    pub fn unlink(&mut self, node: &SubRef) {
        if !node.borrow().linked {
            return;
        }
        let b = (node.borrow().parent as u32 & 7) as usize;
        let pos = self.buckets[b]
            .iter()
            .position(|n| Rc::ptr_eq(n, node))
            .expect("linked sub belongs to table");
        if pos > 0 {
            self.buckets[b][pos - 1].borrow_mut().next = node.borrow().next.clone();
        }
        self.buckets[b].remove(pos);
        let mut n = node.borrow_mut();
        n.linked = false;
        n.next = Link::End;
    }
    pub fn put(&mut self, node: SubRef, parent: i32) {
        self.unlink(&node);
        let b = (parent as u32 & 7) as usize;
        if let Some(last) = self.buckets[b].last() {
            last.borrow_mut().next = Link::Node(Rc::downgrade(&node));
        }
        {
            let mut n = node.borrow_mut();
            n.parent = parent;
            n.linked = true;
            n.next = Link::Bucket(b);
        }
        self.buckets[b].push(node);
    }
}
// A strong cached node survives unlinking, whose clearing of next is
// observable to the close loop's is-linked/restart branch.
enum Next {
    Node(SubRef),
    Bucket(usize),
    End,
}
struct Cursor {
    next: Next,
    index: usize,
}
impl Cursor {
    fn new(t: &Table) -> Self {
        Self {
            next: t.buckets[0]
                .first()
                .cloned()
                .map_or(Next::Bucket(0), Next::Node),
            index: 1,
        }
    }
    fn next(&mut self, t: &Table) -> Option<SubRef> {
        loop {
            match &self.next {
                Next::Node(n) => {
                    let n = n.clone();
                    self.next = match &n.borrow().next {
                        Link::Node(w) => w.upgrade().map_or(Next::End, Next::Node),
                        Link::Bucket(b) => Next::Bucket(*b),
                        Link::End => Next::End,
                    };
                    return Some(n);
                }
                Next::Bucket(b) => {
                    debug_assert_eq!(*b, self.index - 1);
                }
                Next::End => return None,
            }
            if self.index >= 8 {
                self.next = Next::End;
                return None;
            }
            self.next = t.buckets[self.index]
                .first()
                .cloned()
                .map_or(Next::Bucket(self.index), Next::Node);
            self.index += 1;
        }
    }
}

/// Lifecycle service trace. ResetAnimations records the synchronous reset;
/// RemoveMenuOptions is drained by the retained menu owner.
pub enum Service {
    ResetAnimations {
        #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
        interface_id: i32,
        #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
        keys: Keys,
        #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
        components: Vec<Ref>,
    },
    RemoveMenuOptions(i32),
}
pub struct Life {
    ///  and. The UI renderer and
    /// minimap owner consume these retained states at the live cutover.
    pub game_screen_enabled: bool,
    pub cached_minimap_level: i32,
    pub top: i32,
    pub subs: Table,
    pub verify: i32,
    pub verify_changed: bool,
    pub pressed_continue: Option<Ref>,
    pub redraw: [bool; 114],
    pub services: Vec<Service>,
    pub map_flag: Option<[i32; 2]>,
    /// redrawCycle and the transmit redraw-cycle stamps (ui_loop.rs).
    pub cycles: crate::ui_loop::Cycles,
}
impl Default for Life {
    fn default() -> Self {
        Self {
            game_screen_enabled: true,
            cached_minimap_level: -1,
            top: -1,
            subs: Table::default(),
            verify: 0,
            verify_changed: false,
            pressed_continue: None,
            redraw: [false; 114],
            services: Vec::new(),
            map_flag: None,
            cycles: Default::default(),
        }
    }
}
fn sync(state: &mut State) {
    state.layout.subs = state
        .life
        .subs
        .ordered()
        .map(|n| {
            let n = n.borrow();
            (n.parent, n.id)
        })
        .collect();
}
fn reset(store: &mut Store, state: &mut State, id: i32, keys: Keys) -> Result<()> {
    if id != -1 && store.open(id, keys)? {
        let components = store.interfaces[&id].borrow().components.borrow().clone();
        for component in components.iter().flatten() {
            if let Some(node) = component.borrow_mut().model_animator.as_mut() {
                node.node.restart(0);
            }
        }
        state.life.services.push(Service::ResetAnimations {
            interface_id: id,
            keys,
            components: components.into_iter().flatten().collect(),
        });
    }
    Ok(())
}
fn clear_continue(store: &mut Store, state: &mut State) {
    if let Some(c) = state.life.pressed_continue.take() {
        store.updated.push(c);
    }
}
fn redraw(store: &mut Store, state: &mut State, c: &Ref, hooks: bool) -> Result<()> {
    let id = ((c.borrow().f.parentlayer as u32) >> 16) as i32;
    let interface = store
        .interfaces
        .get(&id)
        .cloned()
        .context("redraw interface missing")?;
    state.layout.redraw(store, &interface, c, hooks)
}
fn top_hook(store: &mut Store, state: &mut State, e: &mut impl Executor) -> Result<()> {
    if state.life.top != -1 {
        hooks::immediate(store, state, state.life.top, 1, e)?;
    }
    Ok(())
}
/// openSubInterface. `skip_hooks` is true for script-owned
/// kind-3 interfaces; their layout still runs, with resize hooks disabled.
pub fn open_sub(
    store: &mut Store,
    state: &mut State,
    parent: i32,
    node: SubRef,
    keys: Keys,
    skip_hooks: bool,
    e: &mut impl Executor,
) -> Result<SubRef> {
    if let Some(old) = state.life.subs.get(parent) {
        let unload = old.borrow().id != node.borrow().id;
        close_sub(store, state, &old, unload, skip_hooks, e)?;
    }
    state.life.subs.put(node.clone(), parent);
    sync(state);
    let id = node.borrow().id;
    reset(store, state, id, keys)?;
    let c = store.get(parent, -1)?;
    if let Some(c) = &c {
        store.updated.push(c.clone());
    }
    clear_continue(store, state);
    if let Some(c) = &c {
        redraw(store, state, c, !skip_hooks)?;
    }
    if !skip_hooks {
        hooks::on_load(store, state, id, keys, e)?;
        top_hook(store, state, e)?;
    }
    Ok(node)
}
/// closeSubInterface; purgeServerActive.
pub fn close_sub(
    store: &mut Store,
    state: &mut State,
    node: &SubRef,
    unload: bool,
    skip_hooks: bool,
    e: &mut impl Executor,
) -> Result<()> {
    let (id, parent) = {
        let n = node.borrow();
        (n.id, n.parent)
    };
    state.life.subs.unlink(node);
    sync(state);
    if unload {
        store.unload(id)?;
    }
    // getActive builds an ADD key. Child -1 can borrow into the group bits;
    // purge compares those raw key bits, not a reconstructed component ID.
    state
        .layout
        .active_masks
        .retain(|k, _| ((k >> 48) & 65535) != id as i64);
    state
        .layout
        .active_params
        .retain(|k, _| ((k >> 48) & 65535) != id as i64);
    if let Some(c) = store.get(parent, -1)? {
        store.updated.push(c);
    }
    state.life.services.push(Service::RemoveMenuOptions(id));
    if !skip_hooks {
        top_hook(store, state, e)?;
    }
    let mut cursor = Cursor::new(&state.life.subs);
    while let Some(mut child) = cursor.next(&state.life.subs) {
        if !child.borrow().linked {
            cursor = Cursor::new(&state.life.subs);
            let Some(n) = cursor.next(&state.life.subs) else {
                break;
            };
            child = n;
        }
        let close = {
            let n = child.borrow();
            n.kind == 3 && ((n.parent as u32) >> 16) as i32 == id
        };
        if close {
            close_sub(store, state, &child, true, skip_hooks, e)?;
        }
    }
    Ok(())
}

/// The interface half of showing the login or lobby screen: unload the
/// current top level, close every open sub-interface with hooks, replace the interface arrays
/// (the interface store reset) and open `top` (a `GraphicsDefaults` login or
/// lobby interface, or `-1` for none) with layout, full redraw and its
/// unkeyed `onload`.
pub fn replace_top(
    store: &mut Store,
    state: &mut State,
    top: i32,
    e: &mut impl Executor,
) -> Result<()> {
    if state.life.top != -1 {
        store.unload(state.life.top)?;
    }
    let mut cursor = Cursor::new(&state.life.subs);
    while let Some(mut node) = cursor.next(&state.life.subs) {
        if !node.borrow().linked {
            cursor = Cursor::new(&state.life.subs);
            let Some(n) = cursor.next(&state.life.subs) else {
                break;
            };
            node = n;
        }
        close_sub(store, state, &node, true, false, e)?;
    }
    state.life.top = -1;
    state.life.subs = Table::default();
    sync(state);
    clear_continue(store, state);
    store.reset_all();
    state.life.top = top;
    if top != -1 {
        store.open(top, None)?;
        state
            .layout
            .interface(store, top, state.layout.canvas, false)?;
        state.life.redraw.fill(true);
        hooks::on_load(store, state, top, None, e)?;
    }
    Ok(())
}

/// `SubInterfaceActive*` validity is checked by the client before the
/// interface walk. The packet carries world coordinates for loc/object
/// bindings and entity indices for players/NPCs; the retained scene snapshot
/// is the same owner used by scene menus and CAM2 trackables.
pub fn invalid_active_sub_parents(
    state: &State,
    scene: &crate::ui_cam2::SceneInput<'_>,
) -> Vec<i32> {
    state
        .life
        .subs
        .ordered()
        .filter_map(|node| {
            let sub = node.borrow();
            let binding = sub.binding.as_ref()?;
            (!binding_valid(binding, scene)).then_some(sub.parent)
        })
        .collect()
}

fn binding_valid(binding: &ActiveBinding, scene: &crate::ui_cam2::SceneInput<'_>) -> bool {
    match binding {
        ActiveBinding::Player { index } => {
            let Some(players) = scene.players else {
                return true;
            };
            usize::try_from(*index)
                .ok()
                .and_then(|index| players.players.get(index))
                .is_some_and(Option::is_some)
        }
        ActiveBinding::Npc { index } => {
            let Some(npcs) = scene.npcs else { return true };
            let Ok(index) = usize::try_from(*index) else {
                return false;
            };
            npcs.slots.contains(&index) && npcs.entities.contains_key(&index)
        }
        ActiveBinding::Loc {
            coord, shape, id, ..
        } => {
            let Some(locations) = scene.locations else {
                return true;
            };
            let Some(layer) = loc_layer(*shape) else {
                return false;
            };
            let x = coord.x.wrapping_sub(scene.base[0] >> 9);
            let z = coord.z.wrapping_sub(scene.base[1] >> 9);
            locations
                .get(&(coord.level, layer, x, z))
                .is_some_and(|snapshot| snapshot.id == *id)
        }
        ActiveBinding::Obj { coord, id } => {
            let Some(objects) = scene.objects else {
                return true;
            };
            let key = ((i64::from(coord.level & 3)) << 28)
                | ((i64::from(coord.z & 0x3fff)) << 14)
                | i64::from(coord.x & 0x3fff);
            objects
                .stacks
                .get(&key)
                .is_some_and(|stack| stack.iter().any(|object| object.id == *id))
        }
    }
}

/// `locShapeToLayer` (getLocLayer).
pub(crate) fn loc_layer(shape: i32) -> Option<i32> {
    [
        0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3,
    ]
    .get(usize::try_from(shape).ok()?)
    .copied()
}

#[cfg(test)]
mod active_binding_tests {
    use super::*;

    #[test]
    fn active_player_sub_validity_uses_live_player_slots() {
        let map = crate::protocol910::Context {
            local: 0,
            base_x: 3200,
            base_z: 3200,
            width: 104,
            height: 104,
            bridges: vec![],
        };
        let mut players = crate::protocol910::Players::default();
        players.players[3] = Some(crate::entities910::Player::default());
        let scene = crate::ui_cam2::SceneInput::new(&map, &players, None, 0);
        let mut state = State::default();
        state.life.subs.put(
            Sub::new(1, 0, Some(ActiveBinding::Player { index: 3 })),
            0x0001_0000,
        );
        state.life.subs.put(
            Sub::new(2, 0, Some(ActiveBinding::Player { index: 7 })),
            0x0002_0000,
        );

        let invalid = invalid_active_sub_parents(&state, &scene);
        assert!(!invalid.contains(&0x0001_0000));
        assert!(invalid.contains(&0x0002_0000));
    }
}
/// A packet changes only its own cached payload fields. Other fields survive
/// coalescing/key collisions exactly as the delayed-state-change setters do.
pub struct Update {
    pub kind: i32,
    pub target: i64,
    pub ints: [Option<i32>; 3],
    pub text: Option<Vec<u16>>,
}
impl Update {
    pub fn enqueue(self, changes: &mut Changes) {
        let c = changes.push_server(self.kind, self.target);
        for (slot, v) in c.ints.iter_mut().zip(self.ints) {
            if let Some(v) = v {
                *slot = v;
            }
        }
        if let Some(s) = self.text {
            c.string = Some(s);
        }
    }
}
/// Client packet dispatch increments the verify counter before side effects,
/// even for redundant opens/closes and requests whose resource is not ready.
/// The map flag changes no interface state and leaves it alone.
pub fn packet(
    store: &mut Store,
    state: &mut State,
    event: &UiEvent,
    e: &mut impl Executor,
) -> Result<Vec<Update>> {
    if matches!(event, UiEvent::VarcAck) {
        return Ok(Vec::new());
    }
    if !matches!(event, UiEvent::SetMapFlag { .. }) {
        state.life.verify = state.life.verify.wrapping_add(1);
        state.life.verify_changed = true;
    }
    let mut update = Vec::new();
    match event {
        UiEvent::WorldList { .. } => anyhow::bail!("world list requires runtime owner"),
        UiEvent::Camera { .. }
        | UiEvent::CameraForceAngle { .. }
        | UiEvent::CameraShake { .. }
        | UiEvent::CameraMoveTo { .. }
        | UiEvent::CameraLookAt { .. }
        | UiEvent::CameraReset
        | UiEvent::CameraSmoothReset
        | UiEvent::CameraRemoveRoof { .. } => {
            anyhow::bail!("camera packet requires the runtime camera owner")
        }
        UiEvent::PointLightColour { .. } | UiEvent::PointLightIntensity { .. } => {
            anyhow::bail!("point-light packet requires the scene light owner")
        }
        UiEvent::Audio { .. } => {
            anyhow::bail!("audio packet requires the retained audio owner")
        }
        UiEvent::PlayerSnapshot { .. }
        | UiEvent::ClearPlayerSnapshot { .. }
        | UiEvent::LobbyAppearance { .. } => {
            anyhow::bail!("player snapshot requires the retained model owner")
        }
        UiEvent::StockmarketSlot { .. } => {
            anyhow::bail!("stockmarket packet requires the retained VM owner")
        }
        UiEvent::Cutscene { .. } => {
            anyhow::bail!("cutscene packet requires the game scene owner")
        }
        UiEvent::OverrideEnvironment(_) => {
            anyhow::bail!("environment override requires the renderer owner")
        }
        UiEvent::SiteSettings { .. } => {}
        UiEvent::Uid192 { .. } => {}
        UiEvent::LastLoginInfo { .. } => {}
        UiEvent::UrlOpen { .. }
        | UiEvent::SocialNetworkLogout { .. }
        | UiEvent::Telemetry { .. } => {}
        UiEvent::Inventory { .. } => {
            anyhow::bail!("inventory packet requires the runtime inventory owner")
        }
        UiEvent::Stat { .. } => anyhow::bail!("stat packet requires the runtime stat owner"),
        UiEvent::VarcAck => {}
        UiEvent::VarClanEnable | UiEvent::VarClanDisable | UiEvent::VarClan { .. } => {
            anyhow::bail!("clan variable packet requires the runtime social owner")
        }
        UiEvent::SetVarcBit { id, value } => {
            return Ok(vec![Update {
                kind: 24,
                target: i64::from(*id),
                ints: [Some(*value), None, None],
                text: None,
            }]);
        }
        UiEvent::SetVarc { id, value } => {
            return Ok(vec![Update {
                kind: 1,
                target: i64::from(*id),
                ints: [Some(*value), None, None],
                text: None,
            }]);
        }
        UiEvent::SetVarcString { id, value } => {
            return Ok(vec![Update {
                kind: 2,
                target: i64::from(*id),
                ints: [None, None, None],
                text: Some(value.clone()),
            }]);
        }
        UiEvent::SetTargetParam {
            packed,
            from,
            to,
            param,
        } => {
            for child in *from..=*to {
                let key = ((*packed as i32 as i64) << 32).wrapping_add(i64::from(child));
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    state.layout.active_masks.entry(key)
                {
                    let mask = if child == -1 {
                        store
                            .get(*packed as i32, -1)?
                            .context("IF_SETTARGETPARAM component")?
                            .borrow()
                            .default_active[0]
                    } else {
                        0
                    };
                    slot.insert(mask);
                }
                state.layout.active_params.insert(key, *param);
            }
        }
        UiEvent::SetEvents {
            packed,
            from,
            to,
            mask,
        } => {
            for child in *from..=*to {
                let key = ((*packed as i32 as i64) << 32).wrapping_add(i64::from(child));
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    state.layout.active_params.entry(key)
                {
                    let param = if child == -1 {
                        store
                            .get(*packed as i32, -1)?
                            .context("IF_SETEVENTS component")?
                            .borrow()
                            .default_active[1]
                    } else {
                        -1
                    };
                    slot.insert(param);
                }
                state.layout.active_masks.insert(key, *mask);
            }
        }
        UiEvent::OpenTop { interface_id, keys } => {
            state.life.top = *interface_id as i32;
            reset(store, state, state.life.top, Some(*keys))?;
            state
                .layout
                .interface(store, state.life.top, state.layout.canvas, false)?;
            hooks::on_load(store, state, state.life.top, Some(*keys), e)?;
            state.life.redraw.fill(true);
        }
        UiEvent::OpenSub {
            parent_packed,
            sub_id,
            kind,
            keys,
        } => {
            open_sub(
                store,
                state,
                *parent_packed as i32,
                Sub::new(*sub_id as i32, *kind as i32, None),
                Some(*keys),
                false,
                e,
            )?;
        }
        UiEvent::OpenSubActive {
            parent_packed,
            sub_id,
            kind,
            keys,
            binding,
            ..
        } => {
            open_sub(
                store,
                state,
                *parent_packed as i32,
                Sub::new(*sub_id as i32, *kind as i32, Some(binding.clone())),
                Some(*keys),
                false,
                e,
            )?;
        }
        UiEvent::CloseSub { parent_packed } => {
            if let Some(n) = state.life.subs.get(*parent_packed as i32) {
                close_sub(store, state, &n, true, false, e)?;
            }
            clear_continue(store, state);
        }
        UiEvent::MoveSub {
            source_packed,
            target_packed,
        } => {
            let source = state.life.subs.get(*source_packed as i32);
            if let Some(dest) = state.life.subs.get(*target_packed as i32) {
                let unload = source
                    .as_ref()
                    .is_none_or(|s| s.borrow().id != dest.borrow().id);
                close_sub(store, state, &dest, unload, false, e)?;
            }
            if let Some(source) = source {
                state.life.subs.put(source, *target_packed as i32);
                sync(state);
            }
            if let Some(c) = store.get(*source_packed as i32, -1)? {
                store.updated.push(c);
            }
            if let Some(c) = store.get(*target_packed as i32, -1)? {
                store.updated.push(c.clone());
                redraw(store, state, &c, true)?;
            }
            top_hook(store, state, e)?;
        }
        UiEvent::RunScript(s) => {
            let mut args = vec![Arg::Int(s.script_id)];
            args.extend(s.args.iter().map(|a| match a {
                ScriptArg::Int(v) => Arg::Int(*v),
                ScriptArg::Str(v) => Arg::String(v.encode_utf16().collect()),
            }));
            e.run(
                store,
                state,
                Request {
                    args: Some(args),
                    ..Default::default()
                },
                hooks::INTERACTIVE_LIMIT,
            )?;
        }
        UiEvent::SetText { packed, text } => update.push(Update {
            kind: 3,
            target: *packed as i32 as i64,
            ints: [None; 3],
            text: Some(text.encode_utf16().collect()),
        }),
        UiEvent::SetHide { packed, flag, .. } => update.push(Update {
            kind: 7,
            target: *packed as i32 as i64,
            ints: [Some(*flag as i32), None, None],
            text: None,
        }),
        UiEvent::SetPosition { packed, x, y } => update.push(Update {
            kind: 11,
            target: *packed as i32 as i64,
            ints: [Some(*x as i32), Some(*y as i32), None],
            text: None,
        }),
        UiEvent::SetScrollPos { packed, scroll_y } => update.push(Update {
            kind: 12,
            target: *packed as i32 as i64,
            ints: [Some(*scroll_y as i32), None, None],
            text: None,
        }),
        UiEvent::SetInterfaceAnim { packed, animation } => update.push(Update {
            kind: 5,
            target: *packed as i32 as i64,
            ints: [Some(*animation), None, None],
            text: None,
        }),
        UiEvent::SetInterfaceModel {
            packed,
            model_kind,
            model,
            model_name_hash,
            local_player,
        } => {
            let model = if *local_player {
                state.local_player_uid
            } else {
                *model
            };
            update.push(Update {
                kind: 4,
                target: *packed as i32 as i64,
                ints: [Some(*model_kind), Some(model), Some(*model_name_hash)],
                text: None,
            })
        }
        // The link object count is set, then the listed object type's
        // `xan2d/yan2d/zoom2d` and `xof2d/yof2d/zan2d` (looking up -1 gives the
        // default type, not null).
        UiEvent::SetInterfaceObject {
            packed,
            object,
            count,
        } => {
            let target = *packed as i32 as i64;
            update.push(Update {
                kind: 9,
                target,
                ints: [Some(*object), Some(*count), None],
                text: None,
            });
            let default;
            let objects = state.objs.as_ref().context("object types not installed")?;
            let obj = match u32::try_from(*object).ok().and_then(|id| objects.get(id)) {
                Some(obj) => obj,
                None => {
                    default = crate::config::decode_obj(*object as u32, &[0])?;
                    &default
                }
            };
            let [angle_x, angle_y, angle_z] = obj.inventory.angles;
            let [offset_x, offset_y] = obj.inventory.offset;
            update.push(Update {
                kind: 8,
                target,
                ints: [Some(angle_x), Some(angle_y), Some(obj.inventory.zoom)],
                text: None,
            });
            update.push(Update {
                kind: 10,
                target,
                ints: [Some(offset_x), Some(offset_y), Some(angle_z)],
                text: None,
            });
        }
        UiEvent::SetInterfaceHttpImage { packed, image } => {
            let component = store
                .get(*packed as i32, -1)?
                .context("IF_SET_HTTP_IMAGE component")?;
            let mut component_mut = component.borrow_mut();
            if component_mut.f.httpImageId != *image {
                component_mut.f.httpImageId = *image;
                drop(component_mut);
                store.updated.push(component);
            }
        }
        UiEvent::SetMapFlag { x, z } => update.push(Update {
            kind: 14,
            target: 0,
            ints: [Some(*x), Some(*z), None],
            text: None,
        }),
        UiEvent::SetInterfaceColour { packed, colour } => update.push(Update {
            kind: 6,
            target: *packed as i32 as i64,
            ints: [Some(i32::from(*colour)), None, None],
            text: None,
        }),
        UiEvent::SetInterfaceAngle { packed, x, y, zoom } => update.push(Update {
            kind: 8,
            target: *packed as i32 as i64,
            ints: [
                Some(i32::from(*x)),
                Some(i32::from(*y)),
                Some(i32::from(*zoom)),
            ],
            text: None,
        }),
        UiEvent::SetInterfaceGraphic { packed, graphic } => update.push(Update {
            kind: 13,
            target: *packed as i32 as i64,
            ints: [Some(*graphic), None, None],
            text: None,
        }),
        UiEvent::SetInterfaceTextAntiMacro { packed, enabled } => update.push(Update {
            kind: 23,
            target: *packed as i32 as i64,
            ints: [Some(i32::from(*enabled)), None, None],
            text: None,
        }),
        UiEvent::SetInterfaceTextFont { packed, font } => update.push(Update {
            kind: 15,
            target: *packed as i32 as i64,
            ints: [Some(*font), None, None],
            text: None,
        }),
        UiEvent::SetInterfaceClickMask { packed, enabled } => update.push(Update {
            kind: 22,
            target: *packed as i32 as i64,
            ints: [Some(i32::from(*enabled)), None, None],
            text: None,
        }),
        UiEvent::SetInterfaceRecolour {
            packed,
            index,
            source,
            destination,
        } => update.push(Update {
            kind: 17,
            target: (i64::from(*index) << 32) | i64::from(*packed),
            ints: [
                Some(i32::from(*source)),
                Some(i32::from(*destination)),
                None,
            ],
            text: None,
        }),
        UiEvent::SetInterfaceRetexture {
            packed,
            index,
            source,
            destination,
        } => update.push(Update {
            kind: 20,
            target: (i64::from(*index) << 32) | i64::from(*packed),
            ints: [
                Some(i32::from(*source)),
                Some(i32::from(*destination)),
                None,
            ],
            text: None,
        }),
        // Server menu state (14/18/76) produces no component `Update`: the
        // retained engine owns it (`Engine::apply_menu_state`). The verify
        // bump above already ran, matching the interface dispatch.
        UiEvent::SetMoveAction { .. }
        | UiEvent::ShowFaceHere { .. }
        | UiEvent::SetPlayerOp { .. } => {}
        UiEvent::PlayerAttackPriority { .. }
        | UiEvent::NpcAttackPriority { .. }
        | UiEvent::RunEnergy { .. }
        | UiEvent::RunWeight { .. }
        | UiEvent::RebootTimer { .. }
        | UiEvent::SetTarget { .. }
        | UiEvent::SetDrawOrder { .. }
        | UiEvent::ChatFilters { .. }
        | UiEvent::ChatPrivateFilter { .. }
        | UiEvent::CreateEmailReply { .. }
        | UiEvent::AccountCreationResult { .. }
        | UiEvent::CreateNameReply { .. }
        | UiEvent::CreateSuggestNameError { .. }
        | UiEvent::CreateSuggestName { .. }
        | UiEvent::UpdateDob { .. }
        | UiEvent::LoyaltyUpdate { .. }
        | UiEvent::JCoinsUpdate { .. }
        | UiEvent::TriggerDialogAbort
        | UiEvent::GameMessage { .. }
        | UiEvent::FriendList { .. }
        | UiEvent::FriendListLoaded
        | UiEvent::IgnoreList { .. }
        | UiEvent::FriendChatFull { .. }
        | UiEvent::FriendChatSingle { .. }
        | UiEvent::FriendChannelMessage { .. }
        | UiEvent::ClanChannelFull { .. }
        | UiEvent::ClanRosterDelta { .. }
        | UiEvent::ClanSettingsFull { .. }
        | UiEvent::ClanSettingsUpdate { .. }
        | UiEvent::ClanChannelMessage { .. }
        | UiEvent::ClanChannelSystemMessage { .. }
        | UiEvent::PlayerGroupFull { .. }
        | UiEvent::GroupRosterDelta { .. }
        | UiEvent::PlayerGroupVars { .. }
        | UiEvent::PlayerGroupMessage { .. }
        | UiEvent::QuickChat { .. }
        | UiEvent::PublicMessage { .. }
        | UiEvent::PrivateMessageEcho { .. }
        | UiEvent::PrivateMessage { .. }
        | UiEvent::Logout { .. }
        | UiEvent::ChangeLobby { .. }
        | UiEvent::LogoutTransfer { .. } => {}
        // Runtime::packet owns this packet before the lifecycle dispatch; the
        // arm keeps the shared event matcher exhaustive for direct callers.
        UiEvent::MinimapToggle { .. } | UiEvent::HintArrow { .. } | UiEvent::HintTrail { .. } => {}
        // Only a loaded interface reads the g3 list;
        // out-of-range or absent components are skipped.
        UiEvent::DebugServerTriggers {
            interface,
            start,
            end,
            values,
        } => {
            if let Some(loaded) = store.interfaces.get(&i32::from(*interface)) {
                let loaded = loaded.borrow();
                let components = loaded.components.borrow();
                for (n, index) in (*start..*end).enumerate() {
                    let value = *values
                        .get(n)
                        .context("DEBUG_SERVER_TRIGGERS: truncated g3 list")?;
                    if let Some(Some(component)) = components.get(usize::from(index)) {
                        component.borrow_mut().f.serverTriggers = value;
                    }
                }
            }
        }
        UiEvent::ReflectionProbe(_)
        | UiEvent::Js5Reload
        | UiEvent::ExecuteClientCheat { .. }
        | UiEvent::DoCheat { .. }
        | UiEvent::UnhandledPacket { .. }
        | UiEvent::MalformedPacket { .. } => {
            anyhow::bail!("client shell packet requires the app session owner")
        }
    }
    Ok(update)
}
struct NoHooks;
impl Executor for NoHooks {
    fn run(&mut self, _: &mut Store, _: &mut State, _: Request, _: usize) -> Result<()> {
        anyhow::bail!("script-owned lifecycle unexpectedly ran a hook")
    }
}
pub fn command(
    store: &mut Store,
    state: &mut State,
    name: &str,
    ints: &mut Vec<i32>,
) -> Option<Result<Option<native910::vm::Value>>> {
    let count = match name {
        "if_opensubclient" | "if_hassubmodal" | "if_hassuboverlay" => 2,
        "if_closesubclient" | "if_hassub" | "if_set_gamescreen_enabled" => 1,
        "if_close" | "if_gettop" | "if_debug_getopenifcount" | "if_get_gamescreen" => 0,
        _ => return None,
    };
    Some((|| {
        anyhow::ensure!(ints.len() >= count, "interface lifecycle stack underflow");
        let a = ints.split_off(ints.len() - count);
        let value = match name {
            "if_close" => {
                state
                    .interaction
                    .outgoing
                    .push(crate::proto::client::CLOSE_MODAL);
                let mut cursor = Cursor::new(&state.life.subs);
                while let Some(mut n) = cursor.next(&state.life.subs) {
                    if !n.borrow().linked {
                        cursor = Cursor::new(&state.life.subs);
                        let Some(next) = cursor.next(&state.life.subs) else {
                            break;
                        };
                        n = next;
                    }
                    let modal = n.borrow().kind == 0;
                    if modal {
                        close_sub(store, state, &n, true, true, &mut NoHooks)?;
                    }
                }
                clear_continue(store, state);
                None
            }
            "if_get_gamescreen" => Some(
                state
                    .layout
                    .viewport
                    .as_ref()
                    .map_or(-1, |c| c.borrow().f.parentlayer),
            ),
            "if_set_gamescreen_enabled" => {
                state.life.game_screen_enabled = a[0] == 1;
                if state.life.game_screen_enabled {
                    state.life.cached_minimap_level = -1;
                }
                None
            }
            "if_opensubclient" => {
                open_sub(
                    store,
                    state,
                    a[0],
                    Sub::new(a[1], 3, None),
                    None,
                    true,
                    &mut NoHooks,
                )?;
                None
            }
            "if_closesubclient" => {
                if let Some(n) = state.life.subs.get(a[0]) {
                    if n.borrow().kind == 3 {
                        close_sub(store, state, &n, true, true, &mut NoHooks)?;
                    }
                }
                None
            }
            "if_hassub" => Some(state.life.subs.get(a[0]).is_some() as i32),
            "if_hassubmodal" | "if_hassuboverlay" => Some(
                state
                    .life
                    .subs
                    .get(a[0])
                    .is_some_and(|n| n.borrow().id == a[1]) as i32,
            ),
            "if_gettop" => Some(state.life.top),
            "if_debug_getopenifcount" => Some(
                (state.life.subs.ordered().count() as i32)
                    .wrapping_add((state.life.top != -1) as i32),
            ),
            _ => unreachable!(),
        };
        Ok(value.map(native910::vm::Value::Int))
    })())
}

/// updateInterfaces. Call once for each component change
/// released by ui_vars::State::poll (which owns the varc kinds 1/2). Unknown
/// types are ignored by the original client, including type 255 produced by negative signed
/// target keys.
pub fn apply_change(
    store: &mut Store,
    state: &mut State,
    change: &crate::ui_changes::Change,
) -> Result<()> {
    let kind = change.kind();
    let [a, b, d] = change.ints;
    if kind == 14 {
        state.life.map_flag = Some([a, b]);
        return Ok(());
    }
    if !matches!(kind,3..=13|15|17|20..=23) {
        return Ok(());
    }
    let c = store.get(change.target() as i32, -1)?;
    if kind == 12 && c.as_ref().is_none_or(|c| c.borrow().f.r#type != 0) {
        return Ok(());
    }
    let c = c.context("delayed component missing")?;
    if kind == 5 {
        let mut component = c.borrow_mut();
        if component.f.modelanim != a {
            if a == -1 {
                component.model_animator = None;
            } else {
                let service = state
                    .model_animations
                    .as_ref()
                    .context("interface animation resources")?;
                let playback = component
                    .model_animator
                    .get_or_insert_with(Default::default);
                service.borrow_mut().start(playback, a)?;
            }
            component.f.modelanim = a;
            drop(component);
            store.updated.push(c);
        }
        return Ok(());
    }
    let mut updated = false;
    {
        let mut c = c.borrow_mut();
        macro_rules! set {
            ($field:ident,$value:expr) => {{
                let value = $value;
                if c.f.$field != value {
                    c.f.$field = value;
                    updated = true;
                }
            }};
        }
        match kind {
            3 => {
                let text = change.string.as_ref().context("delayed text is null")?;
                set!(text, Some(text.clone()));
            }
            4 => {
                if [c.f.modelkind, c.f.model, c.f.modelNameHash] != [a, b, d] {
                    c.f.modelkind = a;
                    c.f.model = b;
                    c.f.modelNameHash = d;
                    // `customisation = null`.
                    c.npc_customisation = None;
                    updated = true;
                }
            }
            23 => set!(textantimacro, a == 1),
            6 => set!(
                colour,
                ((a & 31) << 3) + (((a >> 10) & 31) << 19) + (((a >> 5) & 31) << 11)
            ),
            7 => set!(hide, a == 1),
            8 => {
                if [c.f.modelangle_x, c.f.modelangle_y, c.f.modelzoom] != [a, b, d] {
                    c.f.modelangle_x = a;
                    c.f.modelangle_y = b;
                    c.f.modelzoom = d;
                    if c.f.invobject != -1 {
                        let denom = if c.f.modelobjwidth > 0 {
                            c.f.modelobjwidth
                        } else {
                            c.f.wsize
                        };
                        if denom > 0 {
                            c.f.modelzoom = d.wrapping_mul(32) / denom;
                        }
                    }
                    updated = true;
                }
            }
            9 => {
                set!(invobject, a);
                set!(invcount, b);
            }
            10 => {
                set!(modelxof, a);
                set!(modelyof, b);
                set!(modelangle_z, d);
            }
            11 => {
                c.f.xmode = 0;
                c.f.ymode = 0;
                c.f.x = a;
                c.f.xpos = a;
                c.f.y = b;
                c.f.ypos = b;
                updated = true;
            }
            12 => {
                let y = a.min(c.f.scrollheight.wrapping_sub(c.f.height)).max(0);
                set!(scrolly, y);
            }
            13 => c.f.graphic = a,
            15 => c.set_target_font(a),
            21 => c.f.fontmono = a == 1,
            22 => c.f.clickmask = a == 1,
            17 | 20 => {
                let index = (change.target() >> 32) as i32;
                if (0..5).contains(&index) {
                    let slot = if kind == 17 {
                        &mut c.recolour
                    } else {
                        &mut c.retexture
                    };
                    let pair = slot.get_or_insert(([0; 5], [0; 5]));
                    pair.0[index as usize] = a as i16;
                    pair.1[index as usize] = b as i16;
                }
            }
            _ => unreachable!(),
        }
    }
    if updated {
        store.updated.push(c);
    }
    Ok(())
}

#[cfg(test)]
mod delayed_model_tests {
    use super::*;
    use crate::ui_components::{Component, Interface};

    /// A changed type-4 model clears the component's
    /// NPC customisation; an identical triple leaves it.
    #[test]
    fn delayed_model_change_clears_customisation() {
        let component = Rc::new(RefCell::new(Component::default()));
        let mut store = Store::default();
        store
            .interfaces
            .insert(7, Interface::new(vec![Some(component.clone())]));
        let mut state = State::default();
        let custom = crate::ui_models::NpcCustomisation {
            cache_key_salt: 1,
            models: vec![],
            custom_scale: vec![],
            custom_rotation: vec![],
            custom_offset: vec![],
            custom_recol_d: None,
            custom_retex_d: None,
        };
        let change = |ints| crate::ui_changes::Change {
            key: (4i64 << 56) | (7 << 16),
            ints,
            ..Default::default()
        };
        {
            let mut c = component.borrow_mut();
            c.f.parentlayer = 7 << 16;
            c.f.modelkind = 8;
            c.f.model = 94;
            c.f.modelNameHash = -1;
            c.npc_customisation = Some(custom);
        }
        apply_change(&mut store, &mut state, &change([8, 94, -1])).unwrap();
        assert!(component.borrow().npc_customisation.is_some());
        apply_change(&mut store, &mut state, &change([9, 94, 1426])).unwrap();
        let c = component.borrow();
        assert_eq!([c.f.modelkind, c.f.model, c.f.modelNameHash], [9, 94, 1426]);
        assert!(c.npc_customisation.is_none());
    }
}

#[cfg(test)]
mod debug_server_trigger_tests {
    use super::*;
    use crate::ui_components::{Component, Interface};

    #[test]
    fn debug_server_triggers_sets_loaded_components_and_bumps_verify() {
        // g2 interface, g2 start, g2 end, then one g3
        // per component in [start, end).
        let payload = [
            0, 7, 0, 1, 0, 4, 0x12, 0x34, 0x56, 0, 0, 9, 0xff, 0xff, 0xff,
        ];
        let event = crate::server_prot::parse_ui_event(
            crate::proto::server::DEBUG_SERVER_TRIGGERS,
            &payload,
        )
        .unwrap()
        .unwrap();
        let comps: Vec<_> = (0..3)
            .map(|_| Rc::new(RefCell::new(Component::default())))
            .collect();
        let mut store = Store::default();
        store.interfaces.insert(
            7,
            Interface::new(vec![Some(comps[0].clone()), Some(comps[1].clone()), None]),
        );
        let mut state = State::default();
        let before = state.life.verify;
        packet(&mut store, &mut state, &event, &mut NoHooks).unwrap();
        assert_eq!(state.life.verify, before.wrapping_add(1));
        assert_eq!(comps[0].borrow().f.serverTriggers, 0);
        assert_eq!(comps[1].borrow().f.serverTriggers, 0x12_3456);
        // Index 2 is null and index 3 is out of range: skipped.
        // An unloaded interface ignores the list entirely.
        let other = crate::server_prot::UiEvent::DebugServerTriggers {
            interface: 8,
            start: 0,
            end: 5,
            values: vec![],
        };
        packet(&mut store, &mut state, &other, &mut NoHooks).unwrap();
        // A loaded interface with a truncated list is malformed.
        let short = crate::server_prot::UiEvent::DebugServerTriggers {
            interface: 7,
            start: 0,
            end: 2,
            values: vec![1],
        };
        assert!(packet(&mut store, &mut state, &short, &mut NoHooks).is_err());
    }
}
