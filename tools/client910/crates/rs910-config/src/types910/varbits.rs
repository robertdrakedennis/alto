//! Varbit definitions: a bit range inside a base variable of some domain.
//! Binding to the base variable happens at each base-variable opcode; a
//! relaxed read of a missing domain keeps the previous binding. An opcode
//! outside the table fails; opcodes 3 to 15 are accepted and carry no
//! payload.
//!
//! This decoder keeps its own loop instead of an opcode table: the binding
//! needs a per-call lookup, and after a failure callers need the exact cursor
//! position, which the shared table reader does not report the same way.
use super::{script_types::script_type, variables::Value, Error};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub domain: u8,
    pub id: i32,
    pub data_type: Option<u8>,
    pub lifetime: Option<u8>,
    pub legacy: bool,
    pub client_code: i32,
}
impl Binding {
    pub fn empty(domain: u8, id: i32) -> Self {
        Self {
            domain,
            id,
            data_type: None,
            lifetime: Some(0),
            legacy: true,
            client_code: 0,
        }
    }
    /// The default value of the variable: legacy player, NPC, client, world
    /// and controller integers and strings default to -1; everything else to
    /// its data type's default.
    pub fn default_value(&self) -> std::result::Result<Value, Error> {
        let id = self
            .data_type
            .ok_or(Error::Invalid("variable has no datatype"))?;
        if self.legacy
            && ((id == 1 && matches!(self.domain, 0 | 1 | 2 | 3 | 8))
                || (id == 0 && self.domain == 2))
        {
            return Ok(Value::Int(-1));
        }
        script_type(id)
            .map(|t| t.1)
            .ok_or(Error::Invalid("unknown variable datatype"))
    }
}
pub type Lookup<'a> = dyn Fn(u8, i32) -> std::result::Result<Option<Binding>, Error> + 'a;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Type {
    pub id: i32,
    pub domain: Option<u8>,
    pub base_id: i32,
    pub start: i32,
    pub end: i32,
    pub binding: Option<Binding>,
    pub consumed: usize,
    pub raw: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub cause: Error,
    /// The cursor after the failure; it may exceed the buffer length.
    pub consumed: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueError {
    BitRange,
    Overflow,
}
impl Type {
    pub fn empty(id: i32) -> Self {
        Self {
            id,
            domain: None,
            base_id: -1,
            start: 0,
            end: 0,
            binding: None,
            consumed: 0,
            raw: vec![],
        }
    }
    pub fn decode(
        id: i32,
        b: &[u8],
        lookup: Option<&Lookup>,
        allow_unbound: bool,
    ) -> std::result::Result<Self, Failure> {
        let mut s = Self::empty(id);
        s.raw = b.into();
        let mut p = Cursor { bytes: b, pos: 0 };
        let result = (|| -> std::result::Result<(), Error> {
            loop {
                match p.byte()? as u8 {
                    0 => return Ok(()),
                    1 => {
                        let d = p.byte()? as u8;
                        if d > 10 {
                            return Err(Error::Invalid("varbit domain"));
                        }
                        s.domain = Some(d);
                        s.base_id = p.smart2()?;
                        if let Some(lookup) = lookup {
                            if let Some(base) = lookup(d, s.base_id)? {
                                s.binding = Some(base);
                            } else if !allow_unbound {
                                return Err(Error::UnsupportedContext("varbit domain registry"));
                            }
                        }
                    }
                    2 => {
                        s.start = p.byte()? as i32;
                        s.end = p.byte()? as i32;
                    }
                    3..=15 => {}
                    opcode => {
                        return Err(Error::UnsupportedConfig {
                            kind: "varbit",
                            id,
                            opcode,
                        })
                    }
                }
            }
        })();
        s.consumed = p.pos;
        match result {
            Ok(()) => Ok(s),
            Err(cause) => Err(Failure {
                cause,
                consumed: p.pos,
            }),
        }
    }
    fn mask(&self) -> std::result::Result<i32, ValueError> {
        let width = self.end.wrapping_sub(self.start);
        if !(0..32).contains(&width) {
            return Err(ValueError::BitRange);
        }
        Ok(if width == 31 {
            -1
        } else {
            (1i32 << (width + 1)).wrapping_sub(1)
        })
    }
    pub fn get(&self, base: i32) -> std::result::Result<i32, ValueError> {
        Ok(base.wrapping_shr(self.start as u32) & self.mask()?)
    }
    pub fn set(&self, base: i32, value: i32) -> std::result::Result<i32, ValueError> {
        let mask = self.mask()?;
        if value < 0 || value > mask {
            return Err(ValueError::Overflow);
        }
        let shifted = mask.wrapping_shl(self.start as u32);
        Ok(base & !shifted | value.wrapping_shl(self.start as u32) & shifted)
    }
}

// A byte or word read advances the cursor before its bounds check fails, so
// the failure position can lie past the end; a smart-id peek fails without
// advancing. Callers report the cursor after a failure, so this is kept.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl Cursor<'_> {
    fn take(&mut self, n: usize) -> std::result::Result<&[u8], Error> {
        let old = self.pos;
        self.pos += n;
        self.bytes.get(old..self.pos).ok_or(Error::Truncated {
            bit: self.bytes.len() * 8,
        })
    }
    fn byte(&mut self) -> std::result::Result<u32, Error> {
        Ok(self.take(1)?[0] as u32)
    }
    fn smart2(&mut self) -> std::result::Result<i32, Error> {
        let first = *self.bytes.get(self.pos).ok_or(Error::Truncated {
            bit: self.bytes.len() * 8,
        })?;
        if first >= 128 {
            let b = self.take(4)?;
            Ok(i32::from_be_bytes(b.try_into().unwrap()) & i32::MAX)
        } else {
            let b = self.take(2)?;
            let v = u16::from_be_bytes(b.try_into().unwrap()) as i32;
            Ok(if v == 32767 { -1 } else { v })
        }
    }
}
