//! The `stockmarket_*` offer queries over `stockmarketSlots`.
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::absent;
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_stockmarket(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let market = usize::try_from(ints.pop().unwrap()).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "negative stockmarket market".into(),
        })?;
        let slot = usize::try_from(ints.pop().unwrap()).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "negative stockmarket slot".into(),
        })?;
        let offer = self
            .stockmarket_slots
            .get(market)
            .and_then(|slots| slots.get(slot))
            .copied()
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "stockmarket index out of range".into(),
            })?;
        let value = match c.command {
            "stockmarket_getoffertype" => offer.offer_type(),
            "stockmarket_getofferitem" => offer.object,
            "stockmarket_getofferprice" => offer.price,
            "stockmarket_getoffercount" => offer.count,
            "stockmarket_getoffercompletedcount" => offer.completed_count,
            "stockmarket_getoffercompletedgold" => offer.completed_gold,
            "stockmarket_isofferempty" => i32::from(offer.status() == 0),
            "stockmarket_isofferstable" => i32::from(offer.status() == 2),
            "stockmarket_isofferfinished" => i32::from(offer.status() == 5),
            "stockmarket_isofferadding" => i32::from(offer.status() == 1),
            _ => {
                return Err(absent(c.command));
            }
        };
        Ok(Some(Value::Int(value)))
    }
}
