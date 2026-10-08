//! Retained interface traversal, clipping, dragging,
//! redraw ownership and ordered graphics dispatch. Scene and resource consumers
//! execute synchronously at these boundaries, on the caller's render target.
use crate::{
    ui_components::{field_snapshot, Array, Ref, Store},
    ui_properties::State,
};
use anyhow::{Context, Result};
// `Fade` moved to rs910-game (Phase 3.2): the cutscene logic writes it.
pub use rs910_game::screen_fade::*;
use std::{collections::HashMap, rc::Rc};
field_snapshot!(
    /// The scalars `components`/`special` read after the component's
    /// clip, scroll or leaf dispatch point.
    DrawFields {
        r#type: i32,
        width: i32,
        height: i32,
        parentlayer: i32,
        scrollx: i32,
        scrolly: i32,
        colour: i32,
        fill: bool,
        linedirection: bool,
        linewid: i32,
    }
);
pub const DRAG_PASS: i32 = -1412584499;
pub type Rect = [i32; 4]; // x,y,width,height
pub type Clip = [i32; 4]; // min x,y,max x,y (exclusive)

/// Where an interface or component array is drawn: the inherited clip, the
/// offset of its origin and the top-level bounds slot (`-1` while the
/// top-level component is still to be assigned).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub clip: Clip,
    pub offset: [i32; 2],
    pub top: i32,
}

/// A component with a client-drawn code, at its screen position: its clip
/// within the parent clip, and its top-level bounds slot.
struct SpecialSite {
    pos: [i32; 2],
    clipped: Clip,
    parent: Clip,
    top: i32,
}

/// Calls which require the shared toolkit or another client service. Text,
/// sprite and model paint retain the component and inherited clip; their
/// resource consumers must implement the corresponding original leaf branches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Kind {
    Bounds = 0,
    Flush = 1,
    Reset2d = 2,
    ClientComponent = 3,
    Scene = 4,
    EntityLayers = 5,
    GraphicReady = 6,
    Minimap = 7,
    ColourShade = 8,
    ColourGrid = 9,
    Compass = 10,
    WorldMap = 11,
    WorldMapOverview = 12,
    Debug = 13,
    Preview = 14,
    PreviewDraw = 15,
    Streaming = 16,
    StreamReady = 17,
    StreamDraw = 18,
    Environment = 19,
    EnvironmentViewport = 20,
    FramebufferEnabled = 21,
    FramebufferSize = 22,
    EntityOverlays = 23,
    Fill = 24,
    Outline = 25,
    Line = 26,
    Text = 27,
    Sprite = 28,
    Model = 29,
}
#[derive(Clone, Debug)]
pub struct Call {
    pub kind: Kind,
    pub component: Option<Ref>,
    pub args: Vec<i32>,
}
pub enum Reply {
    Unit,
    Bool(bool),
    #[cfg_attr(not(test), allow(dead_code, reason = "constructed by tests only"))]
    Size(Option<[i32; 2]>),
}
pub trait Backend {
    /// No default/no-op implementation: an absent consumer must be reported.
    /// This can recurse into attached UI while retaining painter order.
    fn call(
        &mut self,
        frame: &mut Frame,
        store: &mut Store,
        state: &mut State,
        call: Call,
    ) -> Result<Reply>;
}
#[derive(Clone, Default)]
pub struct Drag {
    pub component: Option<Ref>,
    pub layer: Option<Ref>,
    pub default_layer: Option<Ref>,
    pub active: bool,
    pub parent_ready: bool,
    pub mouse: [i32; 2],
    pub press: [i32; 2],
    pub bounds: Rect,
}
#[derive(Clone)]
pub struct Deferred {
    pub array: Array,
    pub offset: [i32; 2],
}
pub struct Frame {
    pub cycle: i32,
    pub last_cycle: i32,
    pub requested: [bool; 114],
    pub bounds: [Rect; 114],
    pub count: usize,
    pub draw_mode: i32,
    pub scene_state: i32,
    pub client_state: i32,
    pub drag: Drag,
    pub deferred: Option<Deferred>,
    pub fade: Fade,
}
impl Default for Frame {
    fn default() -> Self {
        Self {
            cycle: 0,
            last_cycle: -2,
            requested: [false; 114],
            bounds: [[0; 4]; 114],
            count: 0,
            draw_mode: 0,
            scene_state: 0,
            client_state: 0,
            drag: Drag::default(),
            deferred: None,
            fade: Fade::default(),
        }
    }
}
fn same(a: &Option<Ref>, b: &Option<Ref>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}
fn is(c: &Ref, a: &Option<Ref>) -> bool {
    a.as_ref().is_some_and(|a| Rc::ptr_eq(a, c))
}
fn mark(state: &mut State, index: i32) -> Result<()> {
    *state
        .life
        .redraw
        .get_mut(index as usize)
        .context("redraw index")? = true;
    Ok(())
}
/// One walk's layer lookup. Indices retain the component array's visit order.
#[derive(Clone, Copy)]
struct ComponentWalk<'a> {
    array: &'a Array,
    children: &'a Children,
}

struct Children {
    layers: HashMap<i32, Vec<usize>>,
}

impl Children {
    fn new(array: &Array) -> Self {
        let mut layers: HashMap<i32, Vec<usize>> = HashMap::new();
        for (index, component) in array.borrow().iter().enumerate() {
            if let Some(component) = component {
                layers
                    .entry(component.borrow().f.layer)
                    .or_default()
                    .push(index);
            }
        }
        Self { layers }
    }

    fn indices(&self, array: &Array, layer: i32, drag: &Option<Ref>) -> Vec<usize> {
        if layer == DRAG_PASS {
            return array
                .borrow()
                .iter()
                .enumerate()
                .filter_map(|(index, component)| {
                    let component = component.as_ref()?;
                    (component.borrow().f.layer == layer || is(component, drag)).then_some(index)
                })
                .collect();
        }
        self.layers.get(&layer).cloned().unwrap_or_default()
    }
}

impl Frame {
    /// componentUpdated. Queued hook invalidations are
    /// consumed before advancing the draw-cycle owner.
    pub fn component_updated(&mut self, state: &mut State, c: &Ref) -> Result<()> {
        let c = c.borrow();
        if c.f.lastDrawCycle == self.last_cycle {
            mark(state, c.f.topLevelIndex)?;
        }
        Ok(())
    }
    pub fn consume_updates(&mut self, store: &mut Store, state: &mut State) -> Result<()> {
        for c in std::mem::take(&mut store.updated) {
            self.component_updated(state, &c)?;
        }
        Ok(())
    }
    /// drawGame/drawTitleOrLobby. Animation and other
    /// per-frame services run before this boundary in their owning loop.
    pub fn promote(&mut self, store: &mut Store, state: &mut State, cycle: i32) -> Result<()> {
        self.consume_updates(store, state)?;
        for i in 0..self.count {
            self.requested[i] = state.life.redraw[i];
            state.life.redraw[i] = false;
        }
        self.cycle = cycle;
        self.last_cycle = cycle;
        if state.life.top != -1 {
            self.count = 0;
        }
        Ok(())
    }
    pub fn request_at(&mut self, state: &mut State, r: Rect) {
        for (i, b) in self.bounds[..self.count].iter().enumerate() {
            if b[0].wrapping_add(b[2]) > r[0]
                && b[0] < r[0].wrapping_add(r[2])
                && b[1].wrapping_add(b[3]) > r[1]
                && b[1] < r[1].wrapping_add(r[3])
            {
                state.life.redraw[i] = true;
            }
        }
    }
    pub fn hidden(&self, state: &State, c: &Ref) -> bool {
        let component = c.borrow();
        let f = &component.f;
        if state.layout.debug_bounds && (state.layout.active_mask(c) != 0 || f.r#type == 0) {
            return false;
        }
        component.runtime_entry_hidden().unwrap_or(f.hide)
            || (f.clientcode == 1405 && !state.debug_visible[0] && !state.debug_visible[1])
    }
    fn invoke(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
        kind: Kind,
        c: Option<&Ref>,
        args: &[i32],
    ) -> Result<Reply> {
        backend.call(
            self,
            store,
            state,
            Call {
                kind,
                component: c.cloned(),
                args: args.to_vec(),
            },
        )
    }
    fn emit(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
        kind: Kind,
        c: Option<&Ref>,
        args: &[i32],
    ) -> Result<()> {
        anyhow::ensure!(
            matches!(
                self.invoke(store, state, backend, kind, c, args)?,
                Reply::Unit
            ),
            "unexpected graphics reply"
        );
        Ok(())
    }
    fn query(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
        kind: Kind,
        c: Option<&Ref>,
    ) -> Result<bool> {
        match self.invoke(store, state, backend, kind, c, &[])? {
            Reply::Bool(v) => Ok(v),
            _ => anyhow::bail!("expected graphics boolean"),
        }
    }
    pub fn root(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
    ) -> Result<()> {
        self.deferred = None;
        // No top interface open (openedTopInterface == -1): the frame
        // draws the fullscreen world with no UI pass, not an error.
        if state.life.top == -1 {
            return Ok(());
        }
        let [w, h] = state.layout.canvas;
        self.interface(
            store,
            state,
            backend,
            state.life.top,
            View {
                clip: [0, 0, w, h],
                offset: [0, 0],
                top: -1,
            },
        )?;
        if let Some(d) = self.deferred.clone() {
            let top = if same(&self.drag.layer, &self.drag.default_layer) {
                -1
            } else {
                self.drag
                    .layer
                    .as_ref()
                    .context("missing drag layer")?
                    .borrow()
                    .f
                    .topLevelIndex
            };
            self.components(
                store,
                state,
                backend,
                &d.array,
                DRAG_PASS,
                View {
                    clip: [0, 0, w, h],
                    offset: d.offset,
                    top,
                },
            )?;
            self.deferred = None;
        }
        Ok(())
    }
    pub fn interface(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
        id: i32,
        view: View,
    ) -> Result<()> {
        let top = view.top;
        if id != -1 && store.open(id, None)? {
            let i = store
                .interfaces
                .get(&id)
                .context("interface missing after open")?
                .borrow();
            // An existing sorted array is used as is; none is allocated
            // merely to draw.
            let a = i.sorted.as_ref().unwrap_or(&i.components).clone();
            drop(i);
            self.components(store, state, backend, &a, -1, view)
        } else {
            if top == -1 {
                state.life.redraw.fill(true);
            } else {
                mark(state, top)?;
            }
            Ok(())
        }
    }
    pub fn components(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
        array: &Array,
        layer: i32,
        view: View,
    ) -> Result<()> {
        let children = Children::new(array);
        self.components_indexed(
            store,
            state,
            backend,
            ComponentWalk {
                array,
                children: &children,
            },
            layer,
            view,
        )
    }

    fn components_indexed(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
        walk: ComponentWalk<'_>,
        layer: i32,
        view: View,
    ) -> Result<()> {
        let View { clip, offset, top } = view;
        self.emit(store, state, backend, Kind::Bounds, None, &clip)?;
        let ComponentWalk { array, children } = walk;
        for index in children.indices(array, layer, &self.drag.component) {
            // Other layers' components are skipped without taking a
            // reference (programme Phase 6).
            let c = {
                let array = array.borrow();
                let Some(c) = array.get(index).context("component array index")? else {
                    continue;
                };
                if c.borrow().f.layer != layer
                    && !(layer == DRAG_PASS && is(c, &self.drag.component))
                {
                    continue;
                }
                c.clone()
            };
            let top = if top == -1 {
                let f = &c.borrow().f;
                *self
                    .bounds
                    .get_mut(self.count)
                    .context("top-level bounds capacity")? = [
                    f.x.wrapping_add(offset[0]),
                    f.y.wrapping_add(offset[1]),
                    f.width,
                    f.height,
                ];
                self.count += 1;
                (self.count - 1) as i32
            } else {
                top
            };
            {
                let f = &mut c.borrow_mut().f;
                f.topLevelIndex = top;
                f.lastDrawCycle = self.cycle;
            }
            if self.hidden(state, &c) {
                continue;
            }
            if c.borrow().f.clientcode != 0 {
                self.emit(store, state, backend, Kind::ClientComponent, Some(&c), &[])?;
            }
            let (mut x, mut y, mut trans) = {
                let f = &c.borrow().f;
                (
                    f.x.wrapping_add(offset[0]),
                    f.y.wrapping_add(offset[1]),
                    f.trans,
                )
            };
            if state.layout.debug_bounds
                && (state.layout.active_mask(&c) != 0 || c.borrow().f.r#type == 0)
                && trans > 127
            {
                trans = 127;
            }
            if is(&c, &self.drag.component) {
                let f = &c.borrow().f;
                let direct = state.layout.active_mask(&c) >> 23 & 1 != 0;
                if layer != DRAG_PASS
                    && (f.dragrenderbehaviour == 2
                        || f.dragrenderbehaviour == 0
                        || direct && f.dragrenderbehaviour != 3)
                {
                    self.deferred = Some(Deferred {
                        array: array.clone(),
                        offset,
                    });
                    continue;
                }
                if self.drag.active && self.drag.parent_ready {
                    let mut dx = self.drag.mouse[0].wrapping_sub(self.drag.press[0]);
                    let mut dy = self.drag.mouse[1].wrapping_sub(self.drag.press[1]);
                    let b = self.drag.bounds;
                    if dx < b[0] {
                        dx = b[0];
                    }
                    if f.width.wrapping_add(dx) > b[0].wrapping_add(b[2]) {
                        dx = b[0].wrapping_add(b[2]).wrapping_sub(f.width);
                    }
                    if dy < b[1] {
                        dy = b[1];
                    }
                    if f.height.wrapping_add(dy) > b[3].wrapping_add(b[1]) {
                        dy = b[3].wrapping_add(b[1]).wrapping_sub(f.height);
                    }
                    if direct {
                        self.request_at(state, [dx, dy, f.width, f.height]);
                    }
                    if f.dragrenderbehaviour != 3 {
                        x = dx;
                        y = dy;
                    }
                }
                if f.dragrenderbehaviour == 0 {
                    trans = 128;
                }
            }
            let f = DrawFields::of(&c.borrow().f);
            let clipped = if f.r#type == 2 {
                clip
            } else {
                [
                    x.max(clip[0]),
                    y.max(clip[1]),
                    x.wrapping_add(f.width)
                        .wrapping_add((f.r#type == 9) as i32)
                        .min(clip[2]),
                    y.wrapping_add(f.height)
                        .wrapping_add((f.r#type == 9) as i32)
                        .min(clip[3]),
                ]
            };
            if clipped[0] >= clipped[2] || clipped[1] >= clipped[3] {
                continue;
            }
            let site = SpecialSite {
                pos: [x, y],
                clipped,
                parent: clip,
                top,
            };
            if self.special(store, state, backend, &c, site)? {
                continue;
            }
            if c.borrow().f.r#type == 0 {
                if c.borrow().f.clientcode == 1407 {
                    self.emit(store, state, backend, Kind::Flush, None, &[])?;
                    self.emit(store, state, backend, Kind::Environment, None, &[])?;
                    self.emit(
                        store,
                        state,
                        backend,
                        Kind::EnvironmentViewport,
                        None,
                        &[x, y, state.layout.canvas[0], state.layout.canvas[1]],
                    )?;
                }
                let (parent, sx, sy) = {
                    let f = &c.borrow().f;
                    (
                        f.parentlayer,
                        x.wrapping_sub(f.scrollx),
                        y.wrapping_sub(f.scrolly),
                    )
                };
                let runtime = c.borrow().has_runtime_parent();
                if !runtime {
                    self.components_indexed(
                        store,
                        state,
                        backend,
                        walk,
                        parent,
                        View {
                            clip: clipped,
                            offset: [sx, sy],
                            top,
                        },
                    )?;
                }
                let sorted = c.borrow().child_drawing_order();
                if let Some(sorted) = sorted {
                    let f = DrawFields::of(&c.borrow().f);
                    self.components(
                        store,
                        state,
                        backend,
                        &sorted,
                        f.parentlayer,
                        View {
                            clip: clipped,
                            offset: [x.wrapping_sub(f.scrollx), y.wrapping_sub(f.scrolly)],
                            top,
                        },
                    )?;
                }
                let sub = state
                    .life
                    .subs
                    .get(c.borrow().f.parentlayer)
                    .filter(|_| !runtime)
                    .map(|s| s.borrow().id);
                if let Some(id) = sub {
                    let f = DrawFields::of(&c.borrow().f);
                    self.interface(
                        store,
                        state,
                        backend,
                        id,
                        View {
                            clip: clipped,
                            offset: [x.wrapping_sub(f.scrollx), y.wrapping_sub(f.scrolly)],
                            top,
                        },
                    )?;
                }
                if c.borrow().f.clientcode == 1407 {
                    if self.query(store, state, backend, Kind::FramebufferEnabled, None)? {
                        let f = DrawFields::of(&c.borrow().f);
                        self.emit(
                            store,
                            state,
                            backend,
                            Kind::FramebufferSize,
                            None,
                            &[f.width, f.height],
                        )?;
                        if self.client_state == 18 {
                            let f = DrawFields::of(&c.borrow().f);
                            self.emit(
                                store,
                                state,
                                backend,
                                Kind::EntityOverlays,
                                None,
                                &[x, y, f.width, f.height],
                            )?;
                        }
                    }
                    if self.scene_state == 0 {
                        if let Some(colour) = self.fade.colour(self.cycle) {
                            self.emit(
                                store,
                                state,
                                backend,
                                Kind::Fill,
                                None,
                                &[
                                    clipped[0],
                                    clipped[1],
                                    clipped[2].wrapping_sub(clipped[0]),
                                    clipped[3].wrapping_sub(clipped[1]),
                                    colour,
                                    1,
                                ],
                            )?;
                        }
                    }
                }
                self.emit(store, state, backend, Kind::Bounds, None, &clip)?;
            }
            if !*self
                .requested
                .get(top as usize)
                .context("draw redraw index")?
                && self.draw_mode <= 1
            {
                continue;
            }
            let f = DrawFields::of(&c.borrow().f);
            match f.r#type {
                3 => {
                    let (colour, blend) = if trans == 0 {
                        (f.colour | 0xff000000u32 as i32, 0)
                    } else {
                        (
                            (255 - (trans & 255)).wrapping_shl(24) | (f.colour & 0xffffff),
                            1,
                        )
                    };
                    self.emit(
                        store,
                        state,
                        backend,
                        if f.fill { Kind::Fill } else { Kind::Outline },
                        None,
                        &[x, y, f.width, f.height, colour, blend],
                    )?;
                }
                4..=6 => {
                    let mut args = vec![x, y, trans];
                    args.extend(clip);
                    args.extend(clipped);
                    self.emit(
                        store,
                        state,
                        backend,
                        match f.r#type {
                            4 => Kind::Text,
                            5 => Kind::Sprite,
                            _ => Kind::Model,
                        },
                        Some(&c),
                        &args,
                    )?;
                }
                9 => {
                    let y2 = y.wrapping_add(f.height);
                    self.emit(
                        store,
                        state,
                        backend,
                        Kind::Line,
                        None,
                        &[
                            x,
                            if f.linedirection { y2 } else { y },
                            x.wrapping_add(f.width),
                            if f.linedirection { y } else { y2 },
                            f.colour | 0xff000000u32 as i32,
                            f.linewid,
                            0,
                        ],
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn special(
        &mut self,
        store: &mut Store,
        state: &mut State,
        backend: &mut impl Backend,
        c: &Ref,
        site: SpecialSite,
    ) -> Result<bool> {
        let SpecialSite {
            pos,
            clipped,
            parent,
            top,
        } = site;
        let [x, y] = pos;
        let code = c.borrow().f.clientcode;
        match code {
            1337 | 1403 => {
                if state.life.game_screen_enabled {
                    self.emit(store, state, backend, Kind::Flush, None, &[])?;
                    let f = DrawFields::of(&c.borrow().f);
                    self.emit(
                        store,
                        state,
                        backend,
                        Kind::Scene,
                        None,
                        &[x, y, f.width, f.height, (code == 1403) as i32],
                    )?;
                    self.emit(
                        store,
                        state,
                        backend,
                        Kind::EntityLayers,
                        None,
                        &[top, clipped[0], clipped[1], clipped[2], clipped[3], x, y],
                    )?;
                    self.emit(store, state, backend, Kind::Reset2d, None, &[])?;
                    self.emit(store, state, backend, Kind::Bounds, None, &parent)?;
                    mark(state, top)?;
                }
            }
            1338 if self.scene_state == 3 => {
                if c.borrow().f.r#type != 5
                    || self.query(store, state, backend, Kind::GraphicReady, Some(c))?
                {
                    self.emit(store, state, backend, Kind::Minimap, Some(c), &pos)?;
                    self.emit(store, state, backend, Kind::Bounds, None, &parent)?;
                }
            }
            1408 => self.emit(store, state, backend, Kind::ColourShade, Some(c), &pos)?,
            1409 => {
                let colour = c.borrow().f.colour % 64;
                self.emit(
                    store,
                    state,
                    backend,
                    Kind::ColourGrid,
                    Some(c),
                    &[x, y, colour],
                )?;
            }
            1339 => {
                if self.query(store, state, backend, Kind::GraphicReady, Some(c))? {
                    self.emit(store, state, backend, Kind::Flush, None, &[])?;
                    self.emit(store, state, backend, Kind::Compass, Some(c), &pos)?;
                    self.emit(store, state, backend, Kind::Bounds, None, &parent)?;
                }
            }
            1400 | 1401 => {
                let f = DrawFields::of(&c.borrow().f);
                self.emit(
                    store,
                    state,
                    backend,
                    if code == 1400 {
                        Kind::WorldMap
                    } else {
                        Kind::WorldMapOverview
                    },
                    // The backend reads the component's size/type (and a
                    // type-5 graphic mask) for drawWorldMap.
                    Some(c),
                    &[x, y, f.width, f.height],
                )?;
                mark(state, top)?;
                self.emit(store, state, backend, Kind::Bounds, None, &parent)?;
            }
            1405 => {
                if state.debug_visible[0] || state.debug_visible[1] {
                    self.emit(store, state, backend, Kind::Debug, Some(c), &pos)?;
                    mark(state, top)?;
                }
            }
            1410 => {
                let size = match self.invoke(store, state, backend, Kind::Preview, None, &[])? {
                    Reply::Size(v) => v,
                    _ => anyhow::bail!("preview size reply"),
                };
                if let Some([w, h]) = size {
                    let pw = parent[2].wrapping_sub(parent[0]);
                    let ph = parent[3].wrapping_sub(parent[1]);
                    let ratio = pw as f32 / ph as f32;
                    let aspect = w as f32 / h as f32;
                    let [w, h] = if ratio < aspect {
                        [pw, (pw as f32 / aspect) as i32]
                    } else {
                        [(ph as f32 * aspect) as i32, ph]
                    };
                    self.emit(
                        store,
                        state,
                        backend,
                        Kind::PreviewDraw,
                        None,
                        &[
                            pw.wrapping_sub(w).wrapping_div(2).wrapping_add(parent[0]),
                            ph.wrapping_sub(h).wrapping_div(2).wrapping_add(parent[1]),
                            w,
                            h,
                        ],
                    )?;
                }
            }
            1411 => {
                if self.query(store, state, backend, Kind::Streaming, None)?
                    && self.query(store, state, backend, Kind::StreamReady, None)?
                {
                    self.emit(store, state, backend, Kind::StreamDraw, None, &parent)?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
