//! The minimenu: the option list the client rebuilds every logic cycle and
//! the entry the scripts read back through `get_active_minimenu_entry`.
//!
//! It owns the cancel option, the list/active-entry half of the update,
//! option adding and queueing, submenu insertion, the component options
//! (operation names, target verbs), option and interface-option removal,
//! menu reset and clear, the getters, the action classifiers and the
//! ordering/priority predicates.
//!
//! Popup input and painting live in ui_menu_input/ui_menu_render; component
//! operations and target/drag state in ui_interaction. The live scene supplies
//! walk options. Scene quest icon text is retained from the live config owner.
//!
//! The original client keeps entries in linked lists; here `entries` holds the
//! all-entries order (index 0 = the first entry), each submenu holds its list
//! in secondary-next order (index 0 = the head's next, so a push-back inserts
//! at 0 and the front / previous entry walk from the end).
use crate::ui_components::{Ref, Text};
use std::collections::BTreeMap;
#[path = "ui_menu_input.rs"]
mod input;
pub use input::{Popup, PopupLook};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub op: String,
    /// `target`; `None` is the original client's null (a component with no `opbase`).
    pub target: Option<String>,
    pub cursor: i32,
    pub action: i32,
    pub obj_id: i32,
    pub entity_id: i64,
    pub tile_x: i32,
    pub tile_z: i32,
    pub enabled: bool,
    pub has_arrow: bool,
    pub sub_id: i64,
    pub force_submenu: bool,
    pub detail: Option<String>,
}

/// A submenu: its title, its entry list (secondary-next order) and its size.
#[derive(Clone, Debug, PartialEq)]
pub struct SubMenu {
    pub title: Option<String>,
    /// Entry slots in `secondaryNext` order.
    pub entries: Vec<usize>,
    pub size: i32,
}
impl SubMenu {
    /// `getLastAction`-27: `head.secondaryPrev` is the last in next order.
    fn last_action(&self, menu: &MiniMenu) -> i32 {
        self.entries.last().map_or(-1, |&e| menu.slot(e).action)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MiniMenu {
    pub popup: Popup,
    pub pending: Option<Entry>,
    pub pending_open: bool,
    /// Entry storage; slots are freed on removal.
    slots: Vec<Option<Entry>>,
    /// `allEntries` in list order.
    pub entries: Vec<usize>,
    subs: Vec<Option<SubMenu>>,
    /// The submenus, in next order.
    pub submenus: Vec<usize>,
    /// `submenusById` (`HashTable` keyed by `subId`, insertion order).
    by_id: Vec<(i64, usize)>,
    cancel: Entry,
    pub open: bool,
    pub grouped: bool,
    pub option_count: i32,
    pub submenu_count: i32,
    pub active: Option<usize>,
    pub last_option: Option<usize>,
    pub secondary: Option<usize>,
    pub row_height: i32,
    pub min_length: i32,
    /// `defaultCursor`: reset to -1 before the
    /// interface loop and set by hovered components.
    pub default_cursor: i32,
    /// Retained quest icon tags for scene entries, keyed by menu slot. The
    /// Runtime fills these from the real NPC/loc/object config owners each
    /// cycle before CS2 reads `getEntryQuestText`.
    quest_texts: BTreeMap<usize, String>,
    /// Debug names for hidden ops.
    pub hidden_ops: bool,
}
impl Default for MiniMenu {
    fn default() -> Self {
        let mut m = Self {
            popup: Popup::default(),
            pending: None,
            pending_open: false,
            slots: Vec::new(),
            entries: Vec::new(),
            subs: Vec::new(),
            submenus: Vec::new(),
            by_id: Vec::new(),
            cancel: Entry::default_cancel(-1),
            open: false,
            grouped: false,
            option_count: 0,
            submenu_count: 0,
            active: None,
            last_option: None,
            secondary: None,
            row_height: 16,
            min_length: -1,
            default_cursor: -1,
            quest_texts: BTreeMap::new(),
            hidden_ops: false,
        };
        // createCancelOption, then defaultCursor = -1.
        m.create_cancel_option();
        m
    }
}
impl Entry {
    /// The cancel option every menu starts with.
    fn default_cancel(default_cursor: i32) -> Self {
        Self {
            op: rs910_core::texts::Msg::Cancel.get().into(),
            target: Some(String::new()),
            cursor: default_cursor,
            action: 1006,
            obj_id: -1,
            entity_id: 0,
            tile_x: 0,
            tile_z: 0,
            enabled: true,
            has_arrow: false,
            sub_id: 0,
            force_submenu: true,
            detail: None,
        }
    }
}

pub fn is_priority_action(action: i32) -> bool {
    matches!(action, 59 | 2 | 8 | 17 | 15 | 16 | 58)
}
pub fn orders_before(a: i32, b: i32) -> bool {
    if b >= 1000 && a < 1000 {
        true
    } else if b >= 1000 || a >= 1000 {
        b >= 1000 && a >= 1000
    } else if is_priority_action(a) {
        true
    } else {
        !is_priority_action(b)
    }
}
pub fn is_interface_action(a: i32) -> bool {
    matches!(a, 57 | 58 | 1007 | 25 | 30)
}
pub fn is_obj_action(a: i32) -> bool {
    matches!(a, 18 | 19 | 20 | 21 | 22 | 1004 | 17)
}
pub fn is_npc_action(a: i32) -> bool {
    matches!(a, 9 | 10 | 11 | 12 | 13 | 1003 | 8)
}
pub fn is_player_action(a: i32) -> bool {
    matches!(a, 44..=53 | 15)
}
pub fn is_loc_action(a: i32) -> bool {
    matches!(a, 3 | 4 | 5 | 6 | 1001 | 1002 | 2)
}

pub fn mask_has_op(mask: i32, op: i32) -> bool {
    (mask >> (op + 1)) & 1 != 0
}
pub fn mask_target(mask: i32) -> i32 {
    (mask >> 11) & 0x7F
}
/// `isTargetable` (`mask >> 22 & 1`).
pub fn mask_targetable(mask: i32) -> bool {
    mask >> 22 & 1 != 0
}
pub fn mask_pausebutton(mask: i32) -> bool {
    mask & 1 != 0
}

fn text(t: &Option<Text>) -> Option<String> {
    t.as_ref().map(|t| String::from_utf16_lossy(t))
}

/// What `pushMinimenuEntry` pushes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryView {
    pub entity_type: i32,
    pub op: String,
    pub op_base: String,
    /// Quest icon suffix returned by `getEntryQuestText`.
    pub quest_text: Option<String>,
}

impl MiniMenu {
    fn slot(&self, id: usize) -> &Entry {
        self.slots[id].as_ref().expect("live minimenu entry")
    }
    pub fn entry(&self, id: usize) -> &Entry {
        self.slot(id)
    }
    fn sub(&self, id: usize) -> &SubMenu {
        self.subs[id].as_ref().expect("live submenu")
    }
    /// `createCancelOption` (the cursor is `defaultCursor` at that moment).
    pub fn create_cancel_option(&mut self) {
        self.cancel = Entry::default_cancel(self.default_cursor);
    }
    fn alloc(&mut self, e: Entry) -> usize {
        if let Some(i) = self.slots.iter().position(Option::is_none) {
            self.slots[i] = Some(e);
            i
        } else {
            self.slots.push(Some(e));
            self.slots.len() - 1
        }
    }
    fn alloc_sub(&mut self, title: Option<String>) -> usize {
        let s = SubMenu {
            title,
            entries: Vec::new(),
            size: 0,
        };
        if let Some(i) = self.subs.iter().position(Option::is_none) {
            self.subs[i] = Some(s);
            i
        } else {
            self.subs.push(Some(s));
            self.subs.len() - 1
        }
    }
    /// Adds an option unless the popup is open or the menu is full. A cursor
    /// of -1 takes the menu's default cursor.
    pub fn add_option(&mut self, mut entry: Entry) {
        if self.open || self.option_count >= 505 {
            return;
        }
        if entry.cursor == -1 {
            entry.cursor = self.default_cursor;
        }
        self.enqueue(entry);
    }
    pub fn set_last_detail(&mut self, detail: Option<String>) {
        if let Some(&id) = self.entries.last() {
            self.slots[id].as_mut().unwrap().detail = detail;
        }
    }
    pub fn set_last_quest_text(&mut self, text: Option<String>) {
        let Some(&id) = self.entries.last() else {
            return;
        };
        if let Some(text) = text {
            self.quest_texts.insert(id, text);
        } else {
            self.quest_texts.remove(&id);
        }
    }
    /// Menu slots whose `getEntryQuests` resolves through the
    /// `objId != -1` branch: interface/target entries carrying an object id
    /// that are not themselves object actions.
    pub fn object_quest_entries(&self) -> Vec<(usize, i32)> {
        self.entries
            .iter()
            .map(|&id| (id, self.entry(id)))
            .filter(|(_, e)| !is_obj_action(e.action) && e.obj_id != -1)
            .map(|(id, e)| (id, e.obj_id))
            .collect()
    }
    /// Install one slot's `getQuestIconTags` text.
    pub fn set_quest_text(&mut self, id: usize, text: String) {
        self.quest_texts.insert(id, text);
    }
    fn enqueue(&mut self, e: Entry) {
        let id = self.alloc(e);
        self.entries.push(id);
        self.option_count += 1;
        let (force, target, sub_id) = {
            let e = self.slot(id);
            (e.force_submenu, e.target.clone(), e.sub_id)
        };
        let sub = if force || target.as_deref() == Some("") {
            self.submenu_count += 1;
            self.alloc_sub(target)
        } else {
            // submenusById.get(subId) then nextWithKey until the title matches.
            match self
                .by_id
                .iter()
                .find(|(k, s)| *k == sub_id && self.sub(*s).title == target)
            {
                Some(&(_, s)) => s,
                None => {
                    // cachedSubmenus reuse is an allocation detail; a fresh
                    // submenu is observably identical.
                    let s = self.alloc_sub(target);
                    self.by_id.push((sub_id, s));
                    self.submenu_count += 1;
                    s
                }
            }
        };
        if self.insert_sorted(sub, id) {
            self.insert_submenu(sub);
        }
    }
    /// `insertSorted`-47: walk from `peekFront`
    /// (`head.secondaryPrev`, the end in next order) towards the head.
    fn insert_sorted(&mut self, sub: usize, entry: usize) -> bool {
        self.detach_entry_from_subs(entry);
        let action = self.slot(entry).action;
        let list = self.sub(sub).entries.clone();
        let mut first = true;
        for (i, &other) in list.iter().enumerate().rev() {
            if orders_before(action, self.slot(other).action) {
                // pushNodeBack(entry, other): right after `other` in next order.
                let s = self.subs[sub].as_mut().unwrap();
                s.entries.insert(i + 1, entry);
                s.size += 1;
                return !first;
            }
            first = false;
        }
        // pushBack: right after the head (index 0).
        let s = self.subs[sub].as_mut().unwrap();
        s.entries.insert(0, entry);
        s.size += 1;
        first
    }
    fn detach_entry_from_subs(&mut self, entry: usize) {
        for s in self.subs.iter_mut().flatten() {
            s.entries.retain(|&e| e != entry);
        }
    }
    /// `insertSubmenu` over the `submenus` list.
    fn insert_submenu(&mut self, sub: usize) {
        self.submenus.retain(|&s| s != sub);
        let action = self.sub(sub).last_action(self);
        let list = self.submenus.clone();
        for (i, &other) in list.iter().enumerate().rev() {
            if orders_before(action, self.sub(other).last_action(self)) {
                // insertBefore(sub, other): just before `other` in next order.
                self.submenus.insert(i, sub);
                return;
            }
        }
        self.submenus.insert(0, sub);
    }
    pub fn remove_option(&mut self, entry: usize) {
        if self.open {
            return;
        }
        self.entries.retain(|&e| e != entry);
        self.option_count -= 1;
        let (force, target, sub_id) = {
            let e = self.slot(entry);
            (e.force_submenu, e.target.clone(), e.sub_id)
        };
        let found = if !force {
            self.by_id
                .iter()
                .find(|(k, s)| *k == sub_id && self.sub(*s).title == target)
                .map(|&(_, s)| s)
        } else {
            self.submenus
                .iter()
                .rev()
                .copied()
                .find(|&s| self.sub(s).title == target && self.sub(s).entries.contains(&entry))
        };
        if let Some(sub) = found {
            if self.sub_remove(sub, entry) {
                self.insert_submenu(sub);
            }
        }
        // The original client unlinks the entry object only: `activeMiniMenuEntry`,
        // `secondaryMiniMenuEntry` and `lastOption` keep referencing it until
        // the next `update`/`resetMenu` (the hover cursor reads it this frame).
        if ![self.active, self.secondary, self.last_option].contains(&Some(entry)) {
            self.slots[entry] = None;
            self.quest_texts.remove(&entry);
        }
    }
    fn sub_remove(&mut self, sub: usize, entry: usize) -> bool {
        let before = self.sub(sub).last_action(self);
        let s = self.subs[sub].as_mut().unwrap();
        s.entries.retain(|&e| e != entry);
        s.size -= 1;
        if s.size != 0 {
            return before != self.sub(sub).last_action(self);
        }
        self.submenus.retain(|&x| x != sub);
        self.by_id.retain(|&(_, x)| x != sub);
        self.submenu_count -= 1;
        self.subs[sub] = None;
        false
    }
    pub fn remove_interface_options(&mut self, parentlayer: i32) {
        for e in self.entries.clone() {
            let (action, z) = {
                let e = self.slot(e);
                (e.action, e.tile_z)
            };
            if is_interface_action(action) && z >> 16 == parentlayer {
                self.remove_option(e);
            }
        }
    }
    fn clear_lists(&mut self) {
        // resetMenu/clearMenu: submenus with more than one entry
        // are cached (allocation detail); everything is unlinked.
        self.submenu_count = 0;
        self.option_count = 0;
        self.entries.clear();
        self.by_id.clear();
        self.submenus.clear();
        self.slots.iter_mut().for_each(|s| *s = None);
        self.quest_texts.clear();
        self.subs.iter_mut().for_each(|s| *s = None);
        self.active = None;
        self.last_option = None;
        self.secondary = None;
    }
    pub fn reset(&mut self) {
        self.clear_lists();
        let cancel = self.cancel.clone();
        self.enqueue(cancel);
    }
    /// `update`: reorder `allEntries` and pick the active/secondary
    /// entries. `modifier_held` is `isMenuModifierHeld()`; `canvas_height` is
    /// `canvasHei`; `custom` is `customFormatting`. The click/open
    /// handling that follows is consumed by the live Runtime input
    /// owner after this ordering pass.
    pub fn update(&mut self, modifier_held: bool, canvas_height: i32, custom: bool) {
        if !self.open {
            self.grouped = (self.min_length != -1 && self.option_count >= self.min_length)
                || (if custom { 26_i32 } else { 22 })
                    .wrapping_add(self.option_count.wrapping_mul(self.row_height))
                    > canvas_height;
        }
        let mut kept = Vec::new();
        let mut normal = Vec::new();
        let mut priority = Vec::new();
        for &e in &self.entries {
            let action = self.slot(e).action;
            if action < 1000 {
                if is_priority_action(action) {
                    priority.push(e)
                } else {
                    normal.push(e)
                }
            } else {
                kept.push(e);
            }
        }
        // Merging appends the moved lists at the tail.
        kept.extend(normal);
        kept.extend(priority);
        self.entries = kept;
        let n = self.entries.len();
        if self.option_count > 1 && n >= 2 {
            self.active = Some(if modifier_held && self.option_count > 2 {
                self.entries[n - 2]
            } else {
                self.entries[n - 1]
            });
            self.last_option = Some(self.entries[n - 1]);
            self.secondary = if self.option_count > 2 {
                Some(self.entries[n - 2])
            } else {
                None
            };
        } else {
            self.active = None;
            self.last_option = None;
            self.secondary = None;
        }
    }
    /// `addComponentOptions` for a component with the resolved
    /// server key properties mask (the active mask). Target mode is
    /// the target owner's.
    pub fn add_component_options(&mut self, c: &Ref, mask: i32) {
        let c = c.borrow();
        let has_onop = c.hooks.contains_key("onop");
        let opbase = text(&c.f.opbase);
        let key = (i64::from(c.f.id) << 32) | i64::from(c.f.parentlayer);
        // getOp.
        let op = |i: usize| -> Option<String> {
            if !mask_has_op(mask, i as i32) && !has_onop {
                return None;
            }
            let s = c
                .ops
                .as_ref()
                .and_then(|ops| ops.get(i))
                .and_then(|o| o.as_ref())
                .map(|t| String::from_utf16_lossy(t));
            match s {
                Some(s) if !s.trim().is_empty() => Some(s),
                _ => self.hidden_ops.then(|| format!("Hidden-{i}")),
            }
        };
        // getIfTypeOpName.
        let opname = |i: usize| -> i32 {
            if !mask_has_op(mask, i as i32) && !has_onop {
                return -1;
            }
            c.opname
                .as_ref()
                .and_then(|n| n.get(i))
                .copied()
                .unwrap_or(-1)
        };
        let mut pending: Vec<(String, Option<String>, i32, i32, i64)> = Vec::new();
        for i in (5..=9).rev() {
            if let Some(o) = op(i) {
                pending.push((o, opbase.clone(), opname(i), 1007, (i + 1) as i64));
            }
        }
        // targetVerb.
        if mask_target(mask) != 0 {
            let verb = text(&c.f.targetverb);
            let verb = match verb {
                Some(v) if !v.trim().is_empty() => Some(v),
                _ => self.hidden_ops.then(|| "Hidden-use".to_string()),
            };
            if let Some(v) = verb {
                pending.push((v, opbase.clone(), c.f.targetopcursor, 25, 0));
            }
        }
        for i in (0..=4).rev() {
            if let Some(o) = op(i) {
                pending.push((o, opbase.clone(), opname(i), 57, (i + 1) as i64));
            }
        }
        if mask_pausebutton(mask) {
            // CONTINUE (EN "Continue").
            let t = text(&c.f.pausetext)
                .unwrap_or_else(|| rs910_core::texts::Msg::Continue.get().into());
            pending.push((t, Some(String::new()), -1, 30, 0));
        }
        let (id, parentlayer, invobject) = (c.f.id, c.f.parentlayer, c.f.invobject);
        drop(c);
        for (o, target, cursor, action, entity) in pending {
            self.add_option(Entry {
                op: o,
                target,
                cursor,
                action,
                obj_id: invobject,
                entity_id: entity,
                tile_x: id,
                tile_z: parentlayer,
                enabled: true,
                has_arrow: false,
                sub_id: key,
                force_submenu: false,
                detail: None,
            });
        }
    }
    pub fn entity_type(&self, e: Option<usize>) -> i32 {
        if self.open {
            return 6;
        }
        let Some(e) = e else { return 0 };
        let a = self.slot(e).action;
        if is_interface_action(a) {
            1
        } else if is_obj_action(a) {
            2
        } else if is_loc_action(a) {
            3
        } else if is_npc_action(a) {
            4
        } else if is_player_action(a) {
            7
        } else if a == 16 {
            8
        } else {
            5
        }
    }
    /// `pushMinimenuEntry` over `getEntryOp`
    /// `getEntryOpBase` and `getEntryQuestText`
    ///  (`getEntryQuests`).
    pub fn view(&self, e: Option<usize>) -> EntryView {
        let entity_type = self.entity_type(e);
        if self.open || e.is_none() {
            return EntryView {
                entity_type,
                op: String::new(),
                op_base: String::new(),
                quest_text: Some(String::new()),
            };
        }
        let entry = self.slot(e.unwrap());
        let target_empty = entry.target.as_ref().is_none_or(String::is_empty);
        let op_base = if target_empty && entry.detail.as_ref().is_some_and(|d| !d.is_empty()) {
            entry.detail.clone().unwrap_or_default()
        } else {
            // A null target pushes null in the original client; scripts see it as the empty string.
            entry.target.clone().unwrap_or_default()
        };
        let needs_quests = is_obj_action(entry.action)
            || entry.obj_id != -1
            || is_npc_action(entry.action)
            || is_loc_action(entry.action);
        EntryView {
            entity_type,
            op: entry.op.clone(),
            op_base,
            quest_text: if needs_quests {
                Some(
                    self.quest_texts
                        .get(&e.unwrap())
                        .cloned()
                        .unwrap_or_default(),
                )
            } else {
                Some(String::new())
            },
        }
    }
    /// Object id used by `getEntryQuests` for ground/inventory
    /// object actions. Ground entries store the id in `entity_id`, while
    /// interface entries retain it in `obj_id`.
    pub fn object_id(&self, e: Option<usize>) -> Option<i32> {
        let e = e?;
        if self.open {
            return None;
        }
        let entry = self.slot(e);
        if !is_obj_action(entry.action) {
            return None;
        }
        Some(if entry.obj_id != -1 {
            entry.obj_id
        } else {
            entry.entity_id as i32
        })
    }
}

#[cfg(test)]
mod tests {
    const LOOK: PopupLook = PopupLook {
        ascent: 12,
        descent: 4,
        custom: false,
    };
    use super::*;

    #[test]
    fn reset_leaves_only_cancel_and_no_active_entry() {
        let mut m = MiniMenu::default();
        m.reset();
        assert_eq!(m.option_count, 1);
        assert_eq!(m.submenu_count, 1);
        m.update(false, 768, false);
        assert_eq!(m.active, None);
        assert_eq!(
            m.view(m.active),
            EntryView {
                entity_type: 0,
                op: String::new(),
                op_base: String::new(),
                quest_text: Some(String::new())
            }
        );
    }

    #[test]
    fn removed_active_interface_entry_stays_readable_until_reset() {
        // Closing a dialog (removeInterfaceOptions) must not invalidate the
        // active entry the hover cursor reads in the same frame.
        let mut m = MiniMenu::default();
        m.reset();
        m.add_option(Entry {
            op: "Yes".into(),
            target: None,
            cursor: 46,
            action: 57,
            obj_id: -1,
            entity_id: 1,
            tile_x: -1,
            tile_z: rs910_symbols::component::graphics_change_confirm::YES_BUTTON.packed(),
            enabled: true,
            has_arrow: false,
            sub_id: 1,
            force_submenu: false,
            detail: None,
        });
        m.update(false, 768, false);
        let active = m.active.unwrap();
        m.remove_interface_options(rs910_symbols::interface::GRAPHICS_CHANGE_CONFIRM.id());
        assert_eq!(m.hover_cursor([0, 0], false, false), 46);
        assert_eq!(m.entry(active).action, 57);
        m.reset();
        assert!(m.active.is_none());
    }

    #[test]
    fn scene_quest_tags_are_returned_by_active_entry_query() {
        let mut m = MiniMenu::default();
        m.add_option(Entry {
            op: "Talk".into(),
            target: Some("Goblin".into()),
            cursor: -1,
            action: 9,
            obj_id: -1,
            entity_id: 7,
            tile_x: 10,
            tile_z: 10,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        m.add_option(Entry {
            op: "Examine".into(),
            target: Some("Goblin".into()),
            cursor: -1,
            action: 1003,
            obj_id: -1,
            entity_id: 7,
            tile_x: 10,
            tile_z: 10,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        m.update(false, 768, false);
        m.set_last_quest_text(Some(" <sprite=12>".into()));
        assert_eq!(m.view(m.active).quest_text.as_deref(), Some(" <sprite=12>"));
    }

    /// openMenu over getEntryWidth: the row
    /// width includes the entry's quest icon tags and, for an arrow entry,
    /// `submenuArrowSprite.getWidth() + 4`; drawEntry paints the same text.
    #[test]
    fn popup_width_counts_quest_icons_and_submenu_arrow() {
        let mut m = MiniMenu::default();
        m.reset();
        m.add_option(Entry {
            op: "Follow".into(),
            target: Some("Alice".into()),
            cursor: -1,
            action: 57,
            obj_id: -1,
            entity_id: 1,
            tile_x: 0,
            tile_z: 0,
            enabled: true,
            has_arrow: true,
            sub_id: 1,
            force_submenu: false,
            detail: None,
        });
        m.add_option(Entry {
            op: "Talk-to".into(),
            target: Some("Bob".into()),
            cursor: -1,
            action: 9,
            obj_id: -1,
            entity_id: 2,
            tile_x: 0,
            tile_z: 0,
            enabled: true,
            has_arrow: false,
            sub_id: 2,
            force_submenu: false,
            detail: None,
        });
        m.set_last_quest_text(Some(" <sprite=12>".into()));
        m.update(false, 768, false);
        assert!(!m.grouped);
        assert!(m.has_arrow_entry());
        let rows = m.popup_rows(false);
        let talk = rows.iter().find(|r| r.text.starts_with("Talk-to")).unwrap();
        assert_eq!(talk.text, "Talk-to Bob <sprite=12>");
        assert!(!talk.arrow);
        let follow = rows.iter().find(|r| r.text.starts_with("Follow")).unwrap();
        assert!(follow.arrow);
        let mut width = |s: &str| s.len() as i32 * 6;
        assert_eq!(follow.width(7, &mut width), 12 * 6 + 7 + 4);
        m.open_at([400, 300], [800, 600], LOOK, false, 7, width);
        // max("Choose Option" 78, talk 23*6, follow 83) + 8 + 10.
        assert_eq!(m.popup.bounds[2], 23 * 6 + 18);
        // Without the quest tags the arrow entry is the widest row.
        m.close_popup();
        m.quest_texts.clear();
        m.open_at([400, 300], [800, 600], LOOK, false, 7, |s| {
            s.len() as i32 * 6
        });
        assert_eq!(m.popup.bounds[2], 12 * 6 + 7 + 4 + 18);
    }

    #[test]
    fn update_orders_priority_last_and_picks_it() {
        let mut m = MiniMenu::default();
        m.reset();
        // A normal interface op, then a priority one (58 = target verb on a
        // component in target mode), then a >=1000 examine.
        m.add_option(Entry {
            op: "Talk".into(),
            target: Some("Hans".into()),
            cursor: -1,
            action: 57,
            obj_id: -1,
            entity_id: 1,
            tile_x: 5,
            tile_z: 6,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        m.add_option(Entry {
            op: "Use".into(),
            target: Some("Hans".into()),
            cursor: -1,
            action: 58,
            obj_id: -1,
            entity_id: 1,
            tile_x: 5,
            tile_z: 6,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        m.add_option(Entry {
            op: "Examine".into(),
            target: Some("Hans".into()),
            cursor: -1,
            action: 1004,
            obj_id: -1,
            entity_id: 1,
            tile_x: 5,
            tile_z: 6,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        assert_eq!(m.option_count, 4);
        m.update(false, 768, false);
        let actions: Vec<i32> = m.entries.iter().map(|&e| m.entry(e).action).collect();
        // [>=1000 in order (cancel 1006, examine 1004)] ++ [normal 57] ++ [priority 58]
        assert_eq!(actions, vec![1006, 1004, 57, 58]);
        assert_eq!(m.entry(m.active.unwrap()).op, "Use");
        assert_eq!(m.entry(m.secondary.unwrap()).op, "Talk");
        assert_eq!(m.entity_type(m.active), 1);
        // The menu modifier picks the second entry from the end.
        m.update(true, 768, false);
        assert_eq!(m.entry(m.active.unwrap()).op, "Talk");
        // Two entries only: no secondary.
        m.reset();
        m.add_option(Entry {
            op: "Walk here".into(),
            target: Some(String::new()),
            cursor: -1,
            action: 23,
            obj_id: -1,
            entity_id: 1,
            tile_x: 0,
            tile_z: 0,
            enabled: true,
            has_arrow: false,
            sub_id: 0,
            force_submenu: true,
            detail: None,
        });
        m.update(false, 768, false);
        assert_eq!(m.entry(m.active.unwrap()).op, "Walk here");
        assert_eq!(m.secondary, None);
        assert_eq!(m.entity_type(m.active), 5);
    }

    #[test]
    fn submenus_group_by_sub_id_and_title() {
        let mut m = MiniMenu::default();
        m.reset();
        m.add_option(Entry {
            op: "A".into(),
            target: Some("Hans".into()),
            cursor: -1,
            action: 57,
            obj_id: -1,
            entity_id: 1,
            tile_x: 0,
            tile_z: 0,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        m.add_option(Entry {
            op: "B".into(),
            target: Some("Hans".into()),
            cursor: -1,
            action: 57,
            obj_id: -1,
            entity_id: 1,
            tile_x: 0,
            tile_z: 0,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        m.add_option(Entry {
            op: "C".into(),
            target: Some("Bob".into()),
            cursor: -1,
            action: 57,
            obj_id: -1,
            entity_id: 1,
            tile_x: 0,
            tile_z: 0,
            enabled: true,
            has_arrow: false,
            sub_id: 7,
            force_submenu: false,
            detail: None,
        });
        // cancel (forced) + Hans + Bob
        assert_eq!(m.submenu_count, 3);
        m.remove_interface_options(0);
        assert_eq!((m.option_count, m.submenu_count), (1, 1));
    }
}

#[cfg(test)]
#[path = "ui_cursor_menu_tests.rs"]
mod cursor_tests;
