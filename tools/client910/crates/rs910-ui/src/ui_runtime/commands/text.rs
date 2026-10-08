//! Pure value helpers (`random*`, `lowercase`, `escape`,
//! `tostring_localised`, `removetags`, `date_runeday`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_random(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let bound = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let bound = if c.command == "randominc" {
            bound.wrapping_add(1)
        } else {
            bound
        };
        let value = (self.next_double() * f64::from(bound)) as i32;
        Ok(Some(Value::Int(value)))
    }

    // date_runeday.
    pub(super) fn cmd_date_runeday(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let now = crate::logic_clock::monotonic_millis();
        Ok(Some(Value::Int((now / 86_400_000) as i32 - 11745)))
    }

    pub(super) fn cmd_lowercase(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let value = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        Ok(Some(Value::Str(value.to_lowercase())))
    }

    // Pure escape (< -> <lt>, > -> <gt>).
    pub(super) fn cmd_escape(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let input = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            match ch {
                '<' => out.push_str("<lt>"),
                '>' => out.push_str("<gt>"),
                _ => out.push(ch),
            }
        }
        Ok(Some(Value::Str(out)))
    }

    // localised with EN (login wire advertises
    // EN) and no grouping flag. Pops value then grouping flag, pushes formatted.
    pub(super) fn cmd_tostring_localised(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let flag = ints.pop().unwrap() != 0;
        let mut value = ints.pop().unwrap() as i64;
        let negative = value < 0;
        if negative {
            value = -value;
        }
        let digits = value.to_string().into_bytes();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
        for (i, &d) in digits.iter().enumerate() {
            if i > 0 && flag {
                let remaining = digits.len() - i;
                if remaining.is_multiple_of(3) {
                    out.push(',');
                }
            }
            out.push(d as char);
        }
        if negative {
            out = format!("-{out}");
        }
        Ok(Some(Value::Str(out)))
    }

    // removetags: pure tag stripper.
    pub(super) fn cmd_removetags(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let input = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let mut out = String::with_capacity(input.len());
        let mut in_tag = false;
        for ch in input.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ => {
                    if !in_tag {
                        out.push(ch);
                    }
                }
            }
        }
        Ok(Some(Value::Str(out)))
    }
}
