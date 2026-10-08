//! Model animation ownership for type-6 components.
//! Playback and asset loading are shared with the verified actor path.
use crate::{
    animation_assets::{AnimationAssets, Playback, Pose},
    animation_playback::AnimationRandom,
    cache::Pack,
};

pub struct Animations {
    pub assets: AnimationAssets,
    pack: Pack,
    pub random: AnimationRandom,
}
impl Animations {
    pub fn new(pack: Pack, seed: u64) -> anyhow::Result<Self> {
        Ok(Self {
            assets: AnimationAssets::load(&pack)?,
            pack,
            random: AnimationRandom::new(seed),
        })
    }
    pub fn start(&mut self, playback: &mut Playback, sequence: i32) -> anyhow::Result<()> {
        playback.start(&self.assets, sequence, 0, 0, false, &mut self.random)
    }
    pub fn advance(&mut self, playback: &mut Playback, ticks: i32) -> bool {
        playback.advance(&self.assets, ticks, &mut self.random)
    }
    /// Restarts the current sequence.
    pub fn reset(&mut self, playback: &mut Playback) {
        playback.node.restart(0);
    }
    pub fn prepare(&mut self, playback: &mut Playback, angle: i32) -> anyhow::Result<Vec<Pose>> {
        self.assets.interface_poses(&self.pack, playback, angle)
    }
}

/// animateInterface/animateLayer: a render traversal independent of
/// input hooks, clipping and mouse hit testing. `ticks` is accumulated logic
/// cycles since the preceding draw (sceneDelta).
pub fn interface(
    store: &mut crate::ui_components::Store,
    state: &crate::ui_properties::State,
    frame: &crate::ui_draw::Frame,
    id: i32,
    ticks: i32,
) -> anyhow::Result<()> {
    if id == -1 || !store.open(id, None)? {
        return Ok(());
    }
    let components = store.interfaces[&id].borrow().components.clone();
    layer(store, state, frame, &components, -1, ticks)
}
fn layer(
    store: &mut crate::ui_components::Store,
    state: &crate::ui_properties::State,
    frame: &crate::ui_draw::Frame,
    array: &crate::ui_components::Array,
    parent: i32,
    ticks: i32,
) -> anyhow::Result<()> {
    // The original client's `animateLayer` walks the whole array per layer. By index, not
    // over a copy of the array: nothing in this walk changes the array
    // (only the components' animators), and the copy (plus a reference
    // count per component) per layer dominated the walk (programme
    // Phase 6).
    let count = array.borrow().len();
    for index in 0..count {
        // animateLayer only reads these scalar fields. Cloning Fields copied
        // every component's strings and arrays for every parent in the tree.
        let (c, (layer_id, kind, packed, sequence)) = {
            let array = array.borrow();
            let Some(c) = array.get(index).and_then(Option::as_ref) else {
                continue;
            };
            let f = &c.borrow().f;
            if f.layer != parent {
                continue;
            }
            (c.clone(), (f.layer, f.r#type, f.parentlayer, f.modelanim))
        };
        if layer_id != parent || frame.hidden(state, &c) {
            continue;
        }
        if kind == 0 {
            let runtime = c.borrow().has_runtime_parent();
            if !runtime {
                layer(store, state, frame, array, packed, ticks)?;
            }
            let sorted = c.borrow().child_drawing_order();
            if let Some(children) = sorted {
                layer(store, state, frame, &children, packed, ticks)?;
            }
            if let Some(sub) = state.life.subs.get(packed).filter(|_| !runtime) {
                interface(store, state, frame, sub.borrow().id, ticks)?;
            }
        }
        if kind == 6 && sequence != -1 {
            let service = state
                .model_animations
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("interface animation resources missing"))?;
            let mut c = c.borrow_mut();
            if c.model_animator.is_none() {
                let mut p = Playback::default();
                service.borrow_mut().start(&mut p, sequence)?;
                c.model_animator = Some(p);
            }
            let p = c.model_animator.as_mut().unwrap();
            let mut animations = service.borrow_mut();
            if animations.advance(p, ticks) && p.node.finished {
                animations.reset(p);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn interface_animation_visibility_children_completion_and_reset() -> anyhow::Result<()> {
    use crate::{
        ui_components::{Component, Interface, Store},
        ui_draw::Frame,
        ui_properties::State,
    };
    use std::{cell::RefCell, rc::Rc};
    let pack = Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
    let mut service = Animations::new(pack, 0)?;
    let mut seq = crate::protocol910::sequence_types::Sequence::empty(1);
    seq.frames = Some(vec![2, 3]);
    seq.frame_ids = Some(vec![0, 1]);
    service.assets.sequences.insert(1, seq);
    let mut state = State {
        model_animations: Some(Rc::new(RefCell::new(service))),
        ..Default::default()
    };
    let mut store = Store::default();
    let frame = Frame::default();
    let component = |packed, layer, kind| {
        let mut c = Component::default();
        c.f.parentlayer = packed;
        c.f.layer = layer;
        c.f.r#type = kind;
        c.f.modelanim = if kind == 6 { 1 } else { -1 };
        Rc::new(RefCell::new(c))
    };
    let parent = component(65536, -1, 0);
    let normal = component(65537, 65536, 6);
    let sorted = component(65536, 65536, 6);
    sorted.borrow_mut().f.id = 0;
    let children = Rc::new(RefCell::new(vec![Some(sorted.clone())]));
    parent.borrow_mut().children = Some(children.clone());
    parent.borrow_mut().sorted = Some(children);
    let closed = component(131072, -1, 6);
    store.interfaces.insert(
        1,
        Interface::new(vec![Some(parent.clone()), Some(normal.clone())]),
    );
    store
        .interfaces
        .insert(2, Interface::new(vec![Some(closed.clone())]));
    interface(&mut store, &state, &frame, 1, 3)?;
    for c in [&normal, &sorted] {
        let c = c.borrow();
        let n = &c.model_animator.as_ref().unwrap().node;
        assert_eq!((n.frame, n.time), (1, 1));
        assert!(!c.f.hashook, "animation must not depend on hooks");
    }
    assert!(closed.borrow().model_animator.is_none());
    parent.borrow_mut().f.hide = true;
    interface(&mut store, &state, &frame, 1, 20)?;
    assert_eq!(
        normal.borrow().model_animator.as_ref().unwrap().node.time,
        1
    );
    parent.borrow_mut().f.hide = false;
    interface(&mut store, &state, &frame, 1, 4)?;
    for c in [&normal, &sorted] {
        let c = c.borrow();
        let n = &c.model_animator.as_ref().unwrap().node;
        assert_eq!((n.frame, n.time, n.finished), (0, 0, false));
    }
    interface(&mut store, &state, &frame, 1, 1)?;
    struct NoHooks;
    impl crate::ui_hooks::Executor for NoHooks {
        fn run(
            &mut self,
            _: &mut Store,
            _: &mut State,
            _: crate::ui_hooks::Request,
            _: usize,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }
    state.layout.canvas = [800, 600];
    crate::ui_lifecycle::packet(
        &mut store,
        &mut state,
        &crate::server_prot::UiEvent::OpenTop {
            interface_id: 1,
            keys: [0; 4],
        },
        &mut NoHooks,
    )?;
    assert_eq!(
        normal.borrow().model_animator.as_ref().unwrap().node.time,
        0,
        "open resets direct components synchronously"
    );
    assert_eq!(
        sorted.borrow().model_animator.as_ref().unwrap().node.time,
        1,
        "ifAnimReset does not recursively reset dynamic children"
    );
    Ok(())
}
