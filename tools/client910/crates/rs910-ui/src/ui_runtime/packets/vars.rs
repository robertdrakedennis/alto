//! Variable-owner packets (inventories, stats, clan vars, run energy/weight,
//! varc acknowledgement, stockmarket slots).
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::Runtime;
use super::super::StockmarketSlot;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_vars_packet(
        &mut self,
        vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::StockmarketSlot {
                market,
                slot,
                state,
                object,
                price,
                count,
                completed_count,
                completed_gold,
            } => {
                self.engine.stockmarket_slots[*market][*slot] = StockmarketSlot {
                    state: *state,
                    object: *object,
                    price: *price,
                    count: *count,
                    completed_count: *completed_count,
                    completed_gold: *completed_gold,
                };
                // The offer table changed: the interfaces' `onstocktransmit`
                // hooks run on the next redraw (the Grand Exchange's offer and
                // collection windows draw their slots from it).
                self.state.life.cycles.stock = self.state.life.cycles.redraw;
                Ok(())
            }
            crate::server_prot::UiEvent::Inventory { opcode, bytes } => {
                let defs = vars
                    .definitions
                    .definitions
                    .get(&5)
                    .ok_or_else(|| anyhow::anyhow!("object variable definitions missing"))?;
                let update = crate::ui_inv::decode(*opcode, bytes, defs)?;
                let id = self.engine.inv_cache.apply(update);
                vars.state.inv_transmit.push(id);
                Ok(())
            }
            crate::server_prot::UiEvent::Stat {
                skill,
                current_level,
                xp,
            } => {
                let stats = vars
                    .state
                    .stats
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("player stats owner missing"))?;
                stats.update_stat(*skill, *current_level, *xp)?;
                stats.logged_in_members = self.engine.account.logged_in_members;
                // The stat transmit ring: write the id at the current slot, then
                // increment with signed wraparound.
                vars.state.stat_transmit.push(i32::from(*skill));
                Ok(())
            }
            crate::server_prot::UiEvent::VarClanEnable => {
                self.engine.social.enable_clan_vars();
                vars.state.clan = Some(std::collections::BTreeMap::new());
                Ok(())
            }
            crate::server_prot::UiEvent::VarClanDisable => {
                self.engine.social.disable_clan_vars();
                vars.state.clan = None;
                Ok(())
            }
            crate::server_prot::UiEvent::VarClan { bytes } => {
                let definitions = vars
                    .definitions
                    .definitions
                    .get(&6)
                    .ok_or_else(|| anyhow::anyhow!("clan variable definitions missing"))?;
                self.engine.social.apply_clan_var(bytes, |id| {
                    definitions
                        .get(&id)
                        .and_then(|def| def.data_type)
                        .and_then(crate::protocol910::script_types::script_type)
                        .map(|(base, _)| base)
                })?;
                vars.state.clan = self.engine.social.clan_vars.clone();
                let id = i32::from(u16::from_be_bytes([bytes[0], bytes[1]]));
                vars.state.varclan_transmit.push(id);
                Ok(())
            }
            crate::server_prot::UiEvent::RunEnergy { value } => {
                vars.state.queries.run_energy = i32::from(*value);
                self.state.life.cycles.misc = self.state.life.cycles.redraw;
                Ok(())
            }
            crate::server_prot::UiEvent::RunWeight { value } => {
                vars.state.queries.run_weight = i32::from(*value);
                self.state.life.cycles.misc = self.state.life.cycles.redraw;
                Ok(())
            }
            crate::server_prot::UiEvent::VarcAck => {
                vars.state.persistence.acknowledge();
                Ok(())
            }
            other => anyhow::bail!("packet router: {other:?} is not a vars packet"),
        }
    }
}
