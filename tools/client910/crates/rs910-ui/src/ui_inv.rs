//! The client-side inventory mirror keyed by
//! `inv | (secondary ? MIN_VALUE : 0)`) and its script queries
//! and its script queries (`inv_getobj`, `inv_getnum`, `inv_total`,
//! `inv_freespace`). `UPDATE_INV_FULL`/`UPDATE_INV_PARTIAL` (server packets
//! 6/16) feed the cache and inventory transmit hooks.
use crate::protocol910::{script_types::script_type, varbits::Binding, variables::Value};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
pub type SlotVars = BTreeMap<i32, Value>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    /// `invSlotObjId`, initially `{ -1 }`.
    pub obj_ids: Vec<i32>,
    /// `invSlotObjCount`, initially `{ 0 }`.
    pub counts: Vec<i32>,
    pub vars: Option<Vec<Option<SlotVars>>>,
}
impl Default for Inventory {
    fn default() -> Self {
        Self {
            obj_ids: vec![-1],
            counts: vec![0],
            vars: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct InvCache {
    recent_use: BTreeMap<i64, Inventory>,
}
impl InvCache {
    fn key(inv: i32, secondary: bool) -> i64 {
        i64::from(inv | if secondary { i32::MIN } else { 0 })
    }
    pub fn inventory(&self, inv: i32, secondary: bool) -> Option<&Inventory> {
        self.recent_use.get(&Self::key(inv, secondary))
    }
    /// Every primary (`getInventory(inv, false)`) mirror's `invSlotObjId`,
    /// for the interface model owner of `buildModel`.
    pub fn primary_obj_ids(&self) -> impl Iterator<Item = (i32, &[i32])> {
        self.recent_use
            .iter()
            .filter(|(key, _)| **key >= 0)
            .map(|(key, inv)| (*key as i32, &inv.obj_ids[..]))
    }
    pub fn slot_type(&self, inv: i32, slot: i32, secondary: bool) -> i32 {
        match self.inventory(inv, secondary) {
            None => -1,
            Some(i) => usize::try_from(slot)
                .ok()
                .and_then(|s| i.obj_ids.get(s))
                .copied()
                .unwrap_or(-1),
        }
    }
    pub fn slot_count(&self, inv: i32, slot: i32, secondary: bool) -> i32 {
        match self.inventory(inv, secondary) {
            None => 0,
            Some(i) => usize::try_from(slot)
                .ok()
                .and_then(|s| i.counts.get(s))
                .copied()
                .unwrap_or(0),
        }
    }
    pub fn total(&self, inv: i32, obj: i32, secondary: bool) -> i32 {
        let Some(i) = self.inventory(inv, secondary) else {
            return 0;
        };
        if obj == -1 {
            return 0;
        }
        let mut total = 0i32;
        for (slot, &count) in i.counts.iter().enumerate() {
            if i.obj_ids[slot] == obj {
                total = total.wrapping_add(count);
            }
        }
        total
    }
    /// `getFreeSpace` with the `size` of `inv`.
    pub fn free_space(&self, inv: i32, secondary: bool, inv_size: i32) -> i32 {
        if secondary {
            return 0;
        }
        let Some(i) = self.inventory(inv, secondary) else {
            return inv_size;
        };
        let empty = i.obj_ids.iter().filter(|&&id| id == -1).count() as i32;
        empty + (inv_size - i.obj_ids.len() as i32)
    }
    /// `update`, including per-slot variable lifetime.
    #[cfg(test)] // production applies packets through `update_vars`
    pub fn update(&mut self, inv: i32, slot: usize, obj: i32, count: i32, secondary: bool) {
        self.update_vars(inv, slot, obj, count, None, secondary);
    }
    pub fn update_vars(
        &mut self,
        inv: i32,
        slot: usize,
        obj: i32,
        count: i32,
        vars: Option<SlotVars>,
        secondary: bool,
    ) {
        let entry = self
            .recent_use
            .entry(Self::key(inv, secondary))
            .or_default();
        if entry.obj_ids.len() <= slot {
            let mut ids = vec![-1; slot + 1];
            let mut counts = vec![0; slot + 1];
            ids[..entry.obj_ids.len()].copy_from_slice(&entry.obj_ids);
            counts[..entry.counts.len()].copy_from_slice(&entry.counts);
            entry.obj_ids = ids;
            entry.counts = counts;
            if let Some(vars) = &mut entry.vars {
                vars.resize(slot + 1, None);
            }
        }
        entry.obj_ids[slot] = obj;
        entry.counts[slot] = count;
        if vars.is_some() {
            entry
                .vars
                .get_or_insert_with(|| vec![None; entry.obj_ids.len()])[slot] = vars;
        } else if let Some(v) = &mut entry.vars {
            v[slot] = None;
        }
    }
    pub fn clear(&mut self, inv: i32, secondary: bool) {
        if let Some(i) = self.recent_use.get_mut(&Self::key(inv, secondary)) {
            i.obj_ids.iter_mut().for_each(|v| *v = -1);
            i.counts.iter_mut().for_each(|v| *v = 0);
            i.vars = None;
        }
    }
    pub fn remove(&mut self, inv: i32, secondary: bool) {
        self.recent_use.remove(&Self::key(inv, secondary));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub inv: i32,
    pub secondary: bool,
    pub clear: bool,
    pub remove: bool,
    pub slots: Vec<(usize, i32, i32, Option<SlotVars>)>,
}
/// read. Decode the complete packet
/// before committing it so malformed input never leaves a half-updated cache.
pub fn decode(opcode: u8, bytes: &[u8], defs: &BTreeMap<i32, Binding>) -> Result<Update> {
    use crate::{proto::server as p, ui_bytes::Cursor};
    let mut r = Cursor::new(bytes);
    if opcode == p::UPDATE_INV_STOP_TRANSMIT {
        let flags = r.g1()?.wrapping_sub(128);
        let inv = i32::from(r.g2()?);
        anyhow::ensure!(r.remaining() == 0, "inventory stop trailing bytes");
        return Ok(Update {
            inv,
            secondary: flags & 1 != 0,
            clear: false,
            remove: true,
            slots: vec![],
        });
    }
    anyhow::ensure!(
        matches!(opcode, p::UPDATE_INV_FULL | p::UPDATE_INV_PARTIAL),
        "inventory opcode"
    );
    let inv = i32::from(r.g2()?);
    let flags = r.g1()?;
    let full = opcode == p::UPDATE_INV_FULL;
    let size = if full { usize::from(r.g2()?) } else { 0 };
    let mut update = Update {
        inv,
        secondary: flags & 1 != 0,
        clear: full,
        remove: false,
        slots: vec![],
    };
    while if full {
        update.slots.len() < size
    } else {
        r.remaining() > 0
    } {
        let slot = if full {
            update.slots.len()
        } else {
            r.gsmart1or2()? as usize
        };
        let obj = i32::from(r.g2()?);
        let mut count = 0;
        let mut vars = None;
        if full || obj != 0 {
            count = i32::from(r.g1()?);
            if count == 255 {
                count = r.g4s()?;
            }
            if flags & 2 != 0 {
                let n = r.g1()?;
                if n > 0 {
                    let mut map = BTreeMap::new();
                    for _ in 0..n {
                        let id = i32::from(r.g2()?);
                        let d = defs
                            .get(&id)
                            .context("inventory slot variable definition")?;
                        let value = match d.data_type.and_then(script_type).map(|t| t.0) {
                            Some(0) => Value::Int(r.g4s()?),
                            Some(1) => Value::Long(r.g8()?),
                            Some(2) => {
                                anyhow::ensure!(r.g1()? == 0, "inventory string marker");
                                Value::String(r.gjstr()?)
                            }
                            Some(3) => {
                                Value::FineCoord([i32::from(r.g1()?), r.g4s()?, r.g4s()?, r.g4s()?])
                            }
                            _ => anyhow::bail!("inventory variable type for {id}"),
                        };
                        map.insert(id, value);
                    }
                    vars = Some(map);
                }
            }
        }
        update.slots.push((slot, obj - 1, count, vars));
    }
    anyhow::ensure!(r.remaining() == 0, "inventory trailing bytes");
    Ok(update)
}
impl InvCache {
    pub fn apply(&mut self, update: Update) -> i32 {
        let id = update.inv;
        let secondary = update.secondary;
        if update.remove {
            self.remove(id, secondary);
            return id;
        }
        if update.clear {
            self.clear(id, secondary);
        }
        for (slot, obj, count, vars) in update.slots {
            self.update_vars(id, slot, obj, count, vars, secondary);
        }
        id
    }
    pub fn slot_var(
        &self,
        inv: i32,
        slot: i32,
        varbit: i32,
        secondary: bool,
        defs: &crate::scenery_varbits::Inputs,
    ) -> Result<i32> {
        let t = defs
            .get(varbit, false)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let empty = BTreeMap::new();
        let values = self
            .inventory(inv, secondary)
            .and_then(|i| i.vars.as_ref())
            .and_then(|v| usize::try_from(slot).ok().and_then(|slot| v.get(slot)))
            .and_then(Option::as_ref)
            .unwrap_or(&empty);
        defs.get_sparse(&t, values)
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BACKPACK: i32 = rs910_symbols::inv::BACKPACK.id();
    const COINS: i32 = rs910_symbols::obj::COINS.id();

    #[test]
    fn empty_cache_answers_like_the_original() {
        let c = InvCache::default();
        assert_eq!(c.slot_type(BACKPACK, 0, false), -1);
        assert_eq!(c.slot_count(BACKPACK, 0, false), 0);
        assert_eq!(c.total(BACKPACK, COINS, false), 0);
        assert_eq!(c.free_space(BACKPACK, false, 28), 28);
    }

    #[test]
    fn update_grows_and_counts() {
        let mut c = InvCache::default();
        c.update(BACKPACK, 3, COINS, 10, false);
        c.update(BACKPACK, 1, COINS, 5, false);
        assert_eq!(
            c.inventory(BACKPACK, false).unwrap().obj_ids,
            vec![-1, COINS, -1, COINS]
        );
        assert_eq!(c.total(BACKPACK, COINS, false), 15);
        assert_eq!(c.slot_type(BACKPACK, 9, false), -1);
        assert_eq!(c.free_space(BACKPACK, false, 28), 2 + 24);
        c.clear(BACKPACK, false);
        assert_eq!(c.total(BACKPACK, COINS, false), 0);
        assert_eq!(c.free_space(BACKPACK, false, 28), 28);
    }
}
#[cfg(test)]
mod packet_tests {
    use super::*;
    const BACKPACK: i32 = rs910_symbols::inv::BACKPACK.id();
    #[test]
    fn replacing_and_clearing_slots_drop_their_variables() {
        let mut c = InvCache::default();
        c.update_vars(
            BACKPACK,
            0,
            1,
            1,
            Some(BTreeMap::from([(2, Value::Int(123))])),
            false,
        );
        c.update(BACKPACK, 27, 2, 1, false);
        assert_eq!(
            c.inventory(BACKPACK, false)
                .unwrap()
                .vars
                .as_ref()
                .unwrap()
                .len(),
            28
        );
        c.update(BACKPACK, 0, 3, 1, false);
        assert!(c.inventory(BACKPACK, false).unwrap().vars.as_ref().unwrap()[0].is_none());
        c.clear(BACKPACK, false);
        assert!(c.inventory(BACKPACK, false).unwrap().vars.is_none());
    }
}
#[cfg(test)]
mod slot_variable_tests {
    use super::*;
    #[test]
    fn cache_types_determine_slot_variable_widths() -> Result<()> {
        let defs: BTreeMap<_, _> = [(4, 0), (5, 35), (6, 36), (7, 50)]
            .into_iter()
            .map(|(id, kind)| {
                let mut b = Binding::empty(5, id);
                b.data_type = Some(kind);
                (id, b)
            })
            .collect();
        let mut bytes = vec![0, 93, 3, 0, 1, 0, 2, 1, 4];
        bytes.extend(4u16.to_be_bytes());
        bytes.extend((-123i32).to_be_bytes());
        bytes.extend(5u16.to_be_bytes());
        bytes.extend((-12345678901i64).to_be_bytes());
        bytes.extend(6u16.to_be_bytes());
        bytes.extend([0, 65, 128, 0]);
        bytes.extend(7u16.to_be_bytes());
        bytes.push(2);
        for v in [123i32, -456, 789] {
            bytes.extend(v.to_be_bytes());
        }
        let update = decode(crate::proto::server::UPDATE_INV_FULL, &bytes, &defs)?;
        let vars = update.slots[0].3.as_ref().unwrap();
        assert!(update.secondary);
        assert_eq!(vars[&4], Value::Int(-123));
        assert_eq!(vars[&5], Value::Long(-12345678901));
        assert_eq!(vars[&6], Value::String("A€".into()));
        assert_eq!(vars[&7], Value::FineCoord([2, 123, -456, 789]));
        for end in 9..bytes.len() {
            assert!(decode(crate::proto::server::UPDATE_INV_FULL, &bytes[..end], &defs).is_err());
        }
        assert!(decode(
            crate::proto::server::UPDATE_INV_FULL,
            &bytes,
            &BTreeMap::new()
        )
        .is_err());
        Ok(())
    }
}

#[cfg(test)]
mod frame_tests {
    //! `UPDATE_INV_FULL/PARTIAL/STOP_TRANSMIT` for every inventory the
    //! dev-server frames transmit: bank 95 (517, inventory type 1370), the
    //! `Shop.ts` fixture stock 4 (size 40, `SHOP_FIXTURE_STOCK`
    //! `[[1931,30],[1935,10]]`, the same bytes `ShopCommit.integration.test.ts`
    //! asserts server-side), and the trade/duel/loan/coins offers 90/134/541/
    //! 626 (sizes 28/9/1/1, `cache-survey.json`). Vectors mirror
    //! `verify-inventory.ts:6-9`; field order is
    //! FULL: `g2 inv, g1 flags, g2 size`, then `g2 obj+1, g1 count` with 255
    //! escaping to `g4s`; PARTIAL: `gsmart1or2` slot first
    //! and the STOP branch (`g1_alt1 flags, g2 inv`).
    use super::*;
    use crate::proto::server as p;
    use rs910_symbols::{inv, obj};
    const COINS: i32 = obj::COINS.id();

    /// 995 x100001: `g2 996`, then 255 escapes to `g4s 100001`.
    const BIG: &[u8] = &[0x03, 0xE4, 0xFF, 0x00, 0x01, 0x86, 0xA1];

    fn full(inv: i32, size: i32, occupied: &[&[u8]]) -> Vec<u8> {
        let mut bytes = vec![
            (inv >> 8) as u8,
            inv as u8,
            0,
            (size >> 8) as u8,
            size as u8,
        ];
        for slot in occupied {
            bytes.extend(*slot);
        }
        for _ in occupied.len()..size as usize {
            bytes.extend([0, 0, 0]);
        }
        bytes
    }

    fn routes_as(opcode: u8, bytes: &[u8]) -> bool {
        matches!(
            crate::server_prot::parse_ui_event(opcode, bytes),
            Ok(Some(crate::server_prot::UiEvent::Inventory { opcode: routed, .. })) if routed == opcode
        )
    }

    #[test]
    fn update_inv_frames_for_bank_shop_and_trade_inventories() -> Result<()> {
        let defs = BTreeMap::new();
        let stock: [&[u8]; 2] = [&[0x07, 0x8C, 30], &[0x07, 0x90, 10]];
        // (inv, inventory type size, occupied slot bytes, decoded (obj, count) rows)
        type Case<'a> = (i32, i32, &'a [&'a [u8]], &'a [(i32, i32)]);
        let cases: [Case; 7] = [
            (inv::BANK_CONTENTS.id(), 1370, &[BIG], &[(COINS, 100_001)]),
            (
                inv::GENERAL_STORE_STOCK.id(),
                40,
                &stock,
                &[(obj::EMPTY_POT.id(), 30), (obj::JUG.id(), 10)],
            ),
            (
                inv::GENERAL_STORE_STOCK.id(),
                40,
                &[BIG],
                &[(COINS, 100_001)],
            ),
            (inv::TRADE_OFFER.id(), 28, &[BIG], &[(COINS, 100_001)]),
            (inv::DUEL_STAKE_OFFER.id(), 9, &[BIG], &[(COINS, 100_001)]),
            (inv::TRADE_LEND_OFFER.id(), 1, &[BIG], &[(COINS, 100_001)]),
            (inv::TRADE_POUCH_OFFER.id(), 1, &[BIG], &[(COINS, 100_001)]),
        ];
        for (inv, size, occupied, rows) in cases {
            let (hi, lo) = ((inv >> 8) as u8, inv as u8);
            let last = size as usize - 1;
            // FULL: every slot decoded in order, then committed.
            let bytes = full(inv, size, occupied);
            let update = decode(p::UPDATE_INV_FULL, &bytes, &defs)?;
            assert_eq!(
                (update.inv, update.secondary, update.clear),
                (inv, false, true),
                "inv {inv}"
            );
            assert_eq!(update.slots.len(), size as usize, "inv {inv}");
            for (slot, &(obj, count)) in rows.iter().enumerate() {
                assert_eq!(update.slots[slot], (slot, obj, count, None), "inv {inv}");
            }
            if last >= rows.len() {
                assert_eq!(update.slots[last], (last, -1, 0, None), "inv {inv}");
            }
            let mut cache = InvCache::default();
            assert_eq!(cache.apply(update), inv);
            for (slot, &(obj, count)) in rows.iter().enumerate() {
                assert_eq!(cache.slot_type(inv, slot as i32, false), obj, "inv {inv}");
                assert_eq!(
                    cache.slot_count(inv, slot as i32, false),
                    count,
                    "inv {inv}"
                );
            }
            assert_eq!(
                cache.slot_type(inv, last as i32, false),
                if last < rows.len() { rows[last].0 } else { -1 }
            );
            assert_eq!(
                cache.free_space(inv, false, size),
                size - rows.len() as i32,
                "inv {inv}"
            );
            assert!(routes_as(p::UPDATE_INV_FULL, &bytes), "inv {inv}");
            // Malformed FULL never half-updates the cache.
            for end in [0, 5, 6, 11, 12]
                .into_iter()
                .filter(|&end| end < bytes.len())
            {
                assert!(
                    decode(p::UPDATE_INV_FULL, &bytes[..end], &defs).is_err(),
                    "inv {inv} prefix {end}"
                );
            }
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert!(
                decode(p::UPDATE_INV_FULL, &trailing, &defs).is_err(),
                "inv {inv}"
            );

            // PARTIAL: the secondary copy is isolated from the primary.
            cache.apply(decode(
                p::UPDATE_INV_PARTIAL,
                &[hi, lo, 1, 0, 0, 2, 7],
                &defs,
            )?);
            assert_eq!(cache.slot_count(inv, 0, true), 7, "inv {inv}");
            assert_eq!(cache.slot_count(inv, 0, false), rows[0].1, "inv {inv}");
            // 2-byte `gsmart1or2` slot 128, then 1-byte slot 0 emptied; the
            // decode is generic, so slot 128 decodes past a 1-slot size.
            let partial_128 = [hi, lo, 0, 0x80, 0x80, 0, 2, 3, 0, 0, 0];
            let update = decode(p::UPDATE_INV_PARTIAL, &partial_128, &defs)?;
            assert_eq!(
                update.slots,
                [(128, 1, 3, None), (0, -1, 0, None)],
                "inv {inv}"
            );
            assert!(routes_as(p::UPDATE_INV_PARTIAL, &partial_128), "inv {inv}");
            cache.apply(update);
            assert_eq!(cache.slot_type(inv, 0, false), -1, "inv {inv}");
            assert_eq!(cache.slot_count(inv, 128, false), 3, "inv {inv}");
            // Slot 0 refilled with obj 1 x7; the large-count PARTIAL form.
            assert_eq!(
                cache.apply(decode(
                    p::UPDATE_INV_PARTIAL,
                    &[hi, lo, 0, 0, 0, 2, 7],
                    &defs
                )?),
                inv
            );
            assert_eq!(
                (
                    cache.slot_type(inv, 0, false),
                    cache.slot_count(inv, 0, false)
                ),
                (1, 7)
            );
            // An empty-size FULL still clears the whole copy first
            // (`clear` before the loop),
            // including the slot 128 the generic PARTIAL wrote.
            cache.apply(decode(p::UPDATE_INV_FULL, &[hi, lo, 0, 0, 0], &defs)?);
            assert_eq!(cache.slot_type(inv, 128, false), -1, "inv {inv}");
            assert_eq!(cache.slot_type(inv, 0, false), -1, "inv {inv}");
            let big = [&[hi, lo, 0, 0][..], BIG].concat();
            assert_eq!(
                decode(p::UPDATE_INV_PARTIAL, &big, &defs)?.slots,
                [(0, COINS, 100_001, None)]
            );
            // Truncation rejects, including a large count cut mid-`g4s`.
            assert!(decode(
                p::UPDATE_INV_PARTIAL,
                &[hi, lo, 0, 0, 3, 228, 255, 0],
                &defs
            )
            .is_err());
            assert!(decode(p::UPDATE_INV_PARTIAL, &big[..10], &defs).is_err());
            assert!(decode(p::UPDATE_INV_PARTIAL, &partial_128[..10], &defs).is_err());

            // STOP_TRANSMIT: flag 128 removes the primary copy, 129 only the
            // secondary one.
            let mut cache = InvCache::default();
            cache.apply(decode(p::UPDATE_INV_FULL, &bytes, &defs)?);
            assert!(
                routes_as(p::UPDATE_INV_STOP_TRANSMIT, &[128, hi, lo]),
                "inv {inv}"
            );
            cache.apply(decode(p::UPDATE_INV_STOP_TRANSMIT, &[128, hi, lo], &defs)?);
            assert!(cache.inventory(inv, false).is_none(), "inv {inv}");
            cache.apply(decode(p::UPDATE_INV_FULL, &bytes, &defs)?);
            cache.apply(decode(
                p::UPDATE_INV_PARTIAL,
                &[hi, lo, 1, 0, 0, 2, 7],
                &defs,
            )?);
            cache.apply(decode(p::UPDATE_INV_STOP_TRANSMIT, &[129, hi, lo], &defs)?);
            assert!(cache.inventory(inv, true).is_none(), "inv {inv}");
            assert!(cache.inventory(inv, false).is_some(), "inv {inv}");
        }
        Ok(())
    }
}
