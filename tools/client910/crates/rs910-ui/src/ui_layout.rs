//! Synchronous size/position and recursive UI layout.
//! Integer overflow, division order, prior dimensions and resize-hook order
//! follow the original client. This is also the source of the scene viewport component.
use crate::ui_components::{Array, InterfaceRef, Ref, Store};
pub use crate::ui_layout_constraints::{
    AspectConstraint, AxisRecipe, LayoutPadding, PositionAxis, PositionConstraint, SizeAxis,
    SizeConstraint, ViewportInsets,
};
use rs910_core::fault::Fault;
use std::{
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};
#[derive(Default)]
pub struct Layout {
    pub canvas: [i32; 2],
    pub debug_bounds: bool,
    pub active_masks: BTreeMap<i64, i32>,
    pub active_params: BTreeMap<i64, i32>,
    /// Parent slot / sub-interface, in the owner's HashTable iteration
    /// order. Parent lookup chooses the first matching interface ID.
    pub subs: Vec<(i32, i32)>,
    pub viewport: Option<Ref>,
    /// hookRequests: layout and explicit callonresize share FIFO order.
    pub hooks: VecDeque<crate::ui_hooks::Request>,
    /// hookRequestsTimer / hookRequestsMouseStop (ui_loop.rs).
    pub hooks_timer: VecDeque<crate::ui_hooks::Request>,
    pub hooks_mouse_stop: VecDeque<crate::ui_hooks::Request>,
    /// Actor-attached UI layout calls retained for the entity/UI owner.
    pub actor_layers: Vec<([i32; 2], bool)>,
}
fn div(a: i32, b: i32) -> anyhow::Result<i32> {
    anyhow::ensure!(b != 0, Fault::DivisionByZero.message("component size"));
    Ok(a.wrapping_div(b))
}
impl Layout {
    pub fn active_mask(&self, c: &Ref) -> i32 {
        let c = c.borrow();
        let key = (c.f.parentlayer as i64)
            .wrapping_shl(32)
            .wrapping_add(c.f.id as i64);
        self.active_masks
            .get(&key)
            .copied()
            .unwrap_or(c.default_active[0])
    }
    pub fn size(&mut self, c: &Ref, parent: [i32; 2], hooks: bool) -> anyhow::Result<()> {
        let mask = self.active_mask(c);
        let mut c0 = c.borrow_mut();
        let constraint = c0.size_constraint;
        let viewport = c0.viewport_layout;
        let f = &mut c0.f;
        let old = [f.width, f.height];
        if let Some(constraint) = constraint {
            let mut dimensions = old;
            let result = constraint.apply(
                &mut dimensions,
                parent,
                [f.aspectwidth, f.aspectheight],
                viewport,
            );
            [f.width, f.height] = dimensions;
            result?;
        } else {
            match f.widthSizeMode {
                0 => f.width = f.wsize,
                1 => f.width = parent[0].wrapping_sub(f.wsize),
                2 => f.width = f.wsize.wrapping_mul(parent[0]) >> 14,
                _ => {}
            }
            match f.heightSizeMode {
                0 => f.height = f.hsize,
                1 => f.height = parent[1].wrapping_sub(f.hsize),
                2 => f.height = f.hsize.wrapping_mul(parent[1]) >> 14,
                _ => {}
            }
            if f.widthSizeMode == 4 {
                f.width = div(f.aspectwidth.wrapping_mul(f.height), f.aspectheight)?;
            }
            if f.heightSizeMode == 4 {
                f.height = div(f.aspectheight.wrapping_mul(f.width), f.aspectwidth)?;
            }
        }
        if constraint.is_none() && self.debug_bounds && (mask != 0 || f.r#type == 0) {
            if f.height < 5 && f.width < 5 {
                f.height = 5;
                f.width = 5;
            } else {
                if f.height <= 0 {
                    f.height = 5;
                }
                if f.width <= 0 {
                    f.width = 5;
                }
            }
        }
        if f.clientcode == 1337 {
            self.viewport = Some(c.clone());
        }
        if hooks && old != [f.width, f.height] {
            if let Some(h) = c0.hooks.get("onresize") {
                self.hooks
                    .push_back(crate::ui_hooks::Request::component(c, h.clone()));
            }
        }
        Ok(())
    }
    pub fn position(&self, c: &Ref, parent: [i32; 2]) {
        let mask = self.active_mask(c);
        let mut c = c.borrow_mut();
        let constraint = c.position_constraint;
        let viewport = c.viewport_layout;
        let f = &mut c.f;
        fn axis(mode: i8, raw: i32, size: i32, parent: i32) -> i32 {
            let free = parent.wrapping_sub(size);
            let scaled = raw.wrapping_mul(parent) >> 14;
            match mode {
                0 => raw,
                1 => (free / 2).wrapping_add(raw),
                2 => free.wrapping_sub(raw),
                3 => scaled,
                4 => scaled.wrapping_add(free / 2),
                _ => free.wrapping_sub(scaled),
            }
        }
        if let Some(constraint) = constraint {
            [f.x, f.y] = constraint.resolve(parent, [f.width, f.height], viewport);
        } else {
            f.x = axis(f.xmode, f.xpos, f.width, parent[0]);
            f.y = axis(f.ymode, f.ypos, f.height, parent[1]);
        }
        if constraint.is_none() && self.debug_bounds && (mask != 0 || f.r#type == 0) {
            if f.x < 0 {
                f.x = 0;
            } else if f.width.wrapping_add(f.x) > parent[0] {
                f.x = parent[0].wrapping_sub(f.width);
            }
            if f.y < 0 {
                f.y = 0;
            } else if f.height.wrapping_add(f.y) > parent[1] {
                f.y = parent[1].wrapping_sub(f.height);
            }
        }
    }
    pub fn parent(
        &self,
        store: &mut Store,
        interface: &InterfaceRef,
        c: &Ref,
    ) -> anyhow::Result<Option<Ref>> {
        if c.borrow().has_runtime_parent() {
            return Ok(c.borrow().runtime_parent());
        }
        let (layer, packed) = {
            let c = c.borrow();
            (c.f.layer, c.f.parentlayer)
        };
        let parent = if layer != -1 {
            interface.borrow().get(layer)?
        } else if !interface.borrow().transient {
            match self
                .subs
                .iter()
                .find(|(_, id)| *id == ((packed as u32) >> 16) as i32)
            {
                Some((p, _)) => store.get(*p, -1)?,
                None => None,
            }
        } else {
            None
        };
        Ok(parent)
    }
    pub fn align(
        &mut self,
        store: &mut Store,
        interface: &InterfaceRef,
        c: &Ref,
    ) -> anyhow::Result<()> {
        let dimensions = self.parent(store, interface, c)?.map_or(self.canvas, |c| {
            let c = c.borrow();
            let dimensions = [c.f.width, c.f.height];
            c.layout_padding
                .map_or(dimensions, |padding| padding.available_space(dimensions))
        });
        self.size(c, dimensions, false)?;
        self.position(c, dimensions);
        Ok(())
    }
    pub fn interface(
        &mut self,
        store: &mut Store,
        id: i32,
        parent: [i32; 2],
        hooks: bool,
    ) -> anyhow::Result<()> {
        if id != -1 && store.open(id, None)? {
            let array = store
                .interfaces
                .get(&id)
                .unwrap()
                .borrow()
                .components
                .clone();
            self.layer(store, &array, -1, parent, hooks)?;
        }
        Ok(())
    }
    pub fn redraw(
        &mut self,
        store: &mut Store,
        interface: &InterfaceRef,
        c: &Ref,
        hooks: bool,
    ) -> anyhow::Result<()> {
        self.tree(store, &interface.borrow().components.clone(), c, hooks)
    }
    pub fn tree(
        &mut self,
        store: &mut Store,
        array: &Array,
        c: &Ref,
        hooks: bool,
    ) -> anyhow::Result<()> {
        let (packed, dimensions, sorted) = {
            let c = c.borrow();
            let f = &c.f;
            (
                f.parentlayer,
                {
                    let dimensions = [
                        if f.scrollwidth == 0 {
                            f.width
                        } else {
                            f.scrollwidth
                        },
                        if f.scrollheight == 0 {
                            f.height
                        } else {
                            f.scrollheight
                        },
                    ];
                    c.layout_padding
                        .map_or(dimensions, |padding| padding.available_space(dimensions))
                },
                c.child_drawing_order(),
            )
        };
        let runtime = c.borrow().has_runtime_parent();
        if !runtime {
            self.layer(store, array, packed, dimensions, hooks)?;
        }
        if let Some(a) = sorted {
            self.layer(store, &a, packed, dimensions, hooks)?;
        }
        if let Some((_, id)) = self
            .subs
            .iter()
            .find(|(p, _)| !runtime && *p == packed)
            .copied()
        {
            self.interface(store, id, dimensions, hooks)?;
        }
        if self.viewport.as_ref().is_some_and(|v| Rc::ptr_eq(v, c)) {
            self.actor_layers.push((dimensions, hooks));
        }
        Ok(())
    }
    pub fn layer(
        &mut self,
        store: &mut Store,
        array: &Array,
        parent_id: i32,
        parent: [i32; 2],
        hooks: bool,
    ) -> anyhow::Result<()> {
        for c in array.borrow().clone().into_iter().flatten() {
            if c.borrow().f.layer != parent_id {
                continue;
            }
            self.size(&c, parent, hooks)?;
            self.position(&c, parent);
            {
                let mut c = c.borrow_mut();
                let f = &mut c.f;
                f.scrollx = f.scrollx.min(f.scrollwidth.wrapping_sub(f.width)).max(0);
                f.scrolly = f.scrolly.min(f.scrollheight.wrapping_sub(f.height)).max(0);
            }
            if c.borrow().f.r#type == 0 {
                self.tree(store, array, &c, hooks)?;
            }
        }
        Ok(())
    }
}
