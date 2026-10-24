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
            layer(store, state, frame, array, packed, ticks)?;
            let sorted = c.borrow().child_drawing_order();
            if let Some(children) = sorted {
                layer(store, state, frame, &children, packed, ticks)?;
            }
            if let Some(sub) = state.life.subs.get(packed) {
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
