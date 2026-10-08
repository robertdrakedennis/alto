//! Runtime children have a local drawing order and an owner-wide identity.
//! Construction supplies a fully initialized component; wire bank decoding and
//! revision-specific constructor defaults belong to the script adapter.
use crate::ui_components::{
    Active, Array, Component, InterfaceRef, Ref, RuntimeChildId, RuntimeLink, Store,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

fn owner(parent: &Ref) -> anyhow::Result<Ref> {
    const STATIC_COMPONENT_ID: i32 = -1;
    let parent_state = parent.borrow();
    match &parent_state.runtime_link {
        Some(link) => link
            .owner
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("expired runtime owner")),
        None if parent_state.f.id == STATIC_COMPONENT_ID => Ok(parent.clone()),
        None => anyhow::bail!("runtime container has no owner"),
    }
}

fn array(component: &Ref) -> Array {
    component
        .borrow_mut()
        .children
        .get_or_insert_with(|| Rc::new(RefCell::new(Vec::new())))
        .clone()
}

impl Store {
    /// Change the immediate parent's current entry with the retained target's
    /// identity. A detached target can address a same-id replacement. An
    /// expired parent or an absent entry is a successful no-op.
    pub fn set_runtime_child_hidden(&mut self, target: &Ref, hidden: bool) -> anyhow::Result<()> {
        let (parent, id) = {
            let target = target.borrow();
            anyhow::ensure!(target.has_runtime_parent(), "target is not a runtime child");
            let id = RuntimeChildId::new(u16::try_from(target.f.id)?)?;
            (target.runtime_parent(), id)
        };
        let Some(parent) = parent else { return Ok(()) };
        let current = parent.borrow().children.as_ref().and_then(|children| {
            children
                .borrow()
                .iter()
                .flatten()
                .find(|current| current.borrow().f.id == i32::from(id.encoded()))
                .cloned()
        });
        let Some(current) = current else {
            return Ok(());
        };
        let changed = if hidden {
            parent.borrow_mut().runtime_hidden.insert(id)
        } else {
            parent.borrow_mut().runtime_hidden.remove(&id)
        };
        if changed {
            self.updated.push(current);
        }
        Ok(())
    }

    /// Remove a local runtime identity from both traversal orders, clearing a
    /// removed container's descendants before releasing the owning references.
    /// Retained arrays stay allocated and selection is untouched. A missing
    /// local identity returns false, even if another parent owns that identity.
    pub fn remove_runtime_child(&mut self, parent: &Ref, id: RuntimeChildId) -> bool {
        if !parent.borrow().is_container() {
            return false;
        }
        let children = parent.borrow().children.clone();
        let Some(children) = children else {
            return false;
        };
        let position = children.borrow().iter().position(|current| {
            current
                .as_ref()
                .is_some_and(|current| current.borrow().f.id == i32::from(id.encoded()))
        });
        let Some(position) = position else {
            return false;
        };
        let removed = children.borrow()[position].clone().unwrap();
        if removed.borrow().is_container() {
            super::ui_components::clear_runtime_subtree(&removed);
        }
        children.borrow_mut().remove(position);
        if let Some(sorted) = parent.borrow().sorted.clone() {
            if !Rc::ptr_eq(&children, &sorted) {
                let position = sorted.borrow().iter().position(|current| {
                    current
                        .as_ref()
                        .is_some_and(|current| current.borrow().f.id == i32::from(id.encoded()))
                });
                if let Some(position) = position {
                    sorted.borrow_mut().remove(position);
                }
            }
        }
        super::ui_components::remove_lookup_children(parent, &[Some(removed)]);
        parent.borrow_mut().runtime_hidden.remove(&id);
        self.updated.push(parent.clone());
        true
    }

    /// Attach a constructed component, replacing the same parent's identity.
    /// A collision elsewhere in the owner fails without changing selection.
    /// A leaf target resolves through its immediate parent, then requires a
    /// container. Constructor policy is deliberately separate from attachment.
    pub fn attach_runtime_child(
        &mut self,
        active: &mut Active,
        interface: &InterfaceRef,
        target: &Ref,
        mut child: Component,
        id: RuntimeChildId,
    ) -> anyhow::Result<()> {
        // Invalid constructed inputs must not clear a replaced container.
        anyhow::ensure!(
            child
                .children
                .as_ref()
                .is_none_or(|children| children.borrow().is_empty()),
            "constructed child already has descendants"
        );
        anyhow::ensure!(
            child
                .sorted
                .as_ref()
                .is_none_or(|children| children.borrow().is_empty()),
            "constructed child already has drawing references"
        );
        anyhow::ensure!(
            child.runtime_lookup.as_ref().is_none_or(BTreeMap::is_empty),
            "constructed child already has an owner index"
        );
        anyhow::ensure!(
            child.runtime_hidden.is_empty(),
            "constructed child already has entry visibility"
        );
        let parent = if target.borrow().is_container() {
            target.clone()
        } else if target.borrow().has_runtime_parent() {
            target
                .borrow()
                .runtime_parent()
                .ok_or_else(|| anyhow::anyhow!("expired runtime parent"))?
        } else {
            const NO_PARENT: i32 = -1;
            let layer = target.borrow().f.layer;
            anyhow::ensure!(layer != NO_PARENT, "component has no parent");
            interface
                .borrow()
                .get(layer)?
                .ok_or_else(|| anyhow::anyhow!("missing runtime parent"))?
        };
        anyhow::ensure!(
            parent.borrow().is_container(),
            "runtime parent is not a container"
        );
        let root = owner(&parent)?;
        let existing = root.borrow().runtime_child(id);
        // Allocation of the local list precedes a cross-parent collision.
        let children = array(&parent);
        let position = children.borrow().iter().position(|current| {
            current
                .as_ref()
                .is_some_and(|current| current.borrow().f.id == i32::from(id.encoded()))
        });
        anyhow::ensure!(
            existing.is_none() || position.is_some(),
            "runtime child identity belongs to another parent"
        );
        if let Some(position) = position {
            let old = children.borrow()[position].clone();
            if let Some(old) = old.filter(|old| old.borrow().is_container()) {
                super::ui_components::clear_runtime_subtree(&old);
            }
        }
        // Replacement takes the constructed child's initial visibility, rather
        // than retaining the old entry's state.
        if child.f.hide {
            parent.borrow_mut().runtime_hidden.insert(id);
        } else {
            parent.borrow_mut().runtime_hidden.remove(&id);
        }
        let packed = parent.borrow().f.parentlayer;
        child.f.parentlayer = packed;
        child.f.layer = packed;
        child.f.id = i32::from(id.encoded());
        child.runtime_link = Some(RuntimeLink {
            parent: Rc::downgrade(&parent),
            owner: Rc::downgrade(&root),
        });
        let child = Rc::new(RefCell::new(child));
        // Ordered insertion is the default traversal. Explicit drawing order
        // stays separate only after a reorder populated its retained storage.
        let sorted = parent
            .borrow()
            .sorted
            .clone()
            .filter(|order| !Rc::ptr_eq(order, &children) && !order.borrow().is_empty());
        if let Some(position) = position {
            children.borrow_mut()[position] = Some(child.clone());
            if let Some(sorted) = &sorted {
                let drawing_position = sorted.borrow().iter().position(|current| {
                    current
                        .as_ref()
                        .is_some_and(|current| current.borrow().f.id == i32::from(id.encoded()))
                });
                if let Some(position) = drawing_position {
                    sorted.borrow_mut()[position] = Some(child.clone());
                }
            }
        } else {
            let position = children
                .borrow()
                .iter()
                .position(|current| {
                    current
                        .as_ref()
                        .is_some_and(|current| current.borrow().f.id > i32::from(id.encoded()))
                })
                .unwrap_or_else(|| children.borrow().len());
            children.borrow_mut().insert(position, Some(child.clone()));
            if let Some(sorted) = sorted {
                sorted.borrow_mut().push(Some(child.clone()));
            }
        }
        // Direct owner insertions use their ordered list until nested insertion
        // needs a shared index. Retain that allocation when the tree is cleared.
        let seed = if !Rc::ptr_eq(&root, &parent) && root.borrow().runtime_lookup.is_none() {
            Some(
                root.borrow()
                    .children
                    .as_ref()
                    .map(|children| {
                        children
                            .borrow()
                            .iter()
                            .flatten()
                            .filter_map(|child| {
                                RuntimeChildId::new(u16::try_from(child.borrow().f.id).ok()?)
                                    .ok()
                                    .map(|id| (id, child.clone()))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        } else {
            None
        };
        let mut root_state = root.borrow_mut();
        if let Some(seed) = seed {
            root_state.runtime_lookup = Some(seed);
        }
        if let Some(index) = &mut root_state.runtime_lookup {
            index.insert(id, child.clone());
        }
        drop(root_state);
        active.interface = Some(interface.clone());
        active.component = Some(child);
        self.updated.push(parent);
        Ok(())
    }
}
