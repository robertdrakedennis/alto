//! Varp and varbit packets.
//! Atomic packet application retains the intentional server-varbit overflow
//! no-op as an explicit receipt. RESET requests the real +64 transmit counter.
use super::{
    varbits::{Type, ValueError},
    Error, Packet, Result,
};
use crate::entities910::varps::{Outcome, Varps};
use crate::proto as wire;
pub type BitLookup<'a> = dyn Fn(i32) -> Result<Type> + 'a;
/// Varbit reads/writes over the local varp arrays (with a varbit type): moved from `entities910::varps` in Phase 2.2 so
/// entity state does not name the config type.
impl Varps {
    pub fn get_bit(&self, bit: &Type) -> Result<i32> {
        let base = bit
            .binding
            .as_ref()
            .ok_or(Error::UnsupportedContext("unbound local varbit"))?;
        bit.get(self.get(base.id)?)
            .map_err(|_| Error::Invalid("varbit range"))
    }
    pub fn set_bit(&mut self, bit: &Type, value: i32, now: i64, server: bool) -> Result<Outcome> {
        let base = bit
            .binding
            .as_ref()
            .ok_or(Error::UnsupportedContext("unbound local varbit"))?;
        let old = if server {
            *self
                .server
                .get(base.id as usize)
                .ok_or(Error::Invalid("varp index"))?
        } else {
            self.get(base.id)?
        };
        let updated = match bit.set(old, value) {
            Ok(v) => v,
            Err(ValueError::Overflow) if server => {
                return Ok(Outcome {
                    clock_reads: 0,
                    ignored_overflow: true,
                })
            }
            Err(ValueError::Overflow) => return Err(Error::Invalid("local varbit overflow")),
            Err(ValueError::BitRange) => return Err(Error::Invalid("varbit range")),
        };
        if server {
            self.set_server(base.id, updated, now)
        } else {
            self.set_local(base.id, updated, now)
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))] // test-only decode result
pub struct Decoded {
    pub state: Varps,
}
pub fn handles(op: u8) -> bool {
    use wire::server::*;
    matches!(
        op,
        VARP_LARGE | VARP_SMALL | VARBIT_LARGE | VARBIT_SMALL | RESET_CLIENT_VARCACHE
    )
}
#[cfg(any(test, feature = "test-hooks"))] // test-only decode entry
pub fn decode(op: u8, bytes: &[u8], prior: &Varps, now: i64, bits: &BitLookup) -> Result<Decoded> {
    let mut state = prior.clone();
    apply(op, bytes, &mut state, now, bits)?;
    Ok(Decoded { state })
}
pub struct Applied {
    pub consumed: usize,
    pub outcome: Outcome,
    pub transmit_increment: i32,
}
enum Write {
    Server(i32, i32),
    Bit(Type, i32),
    Reset,
}
impl Write {
    fn apply(&self, s: &mut Varps, now: i64) -> Result<(Outcome, i32)> {
        Ok(match self {
            Self::Server(id, value) => (s.set_server(*id, *value, now)?, 0),
            Self::Bit(bit, value) => (s.set_bit(bit, *value, now, true)?, 0),
            Self::Reset => {
                s.reset();
                (Outcome::default(), 64)
            }
        })
    }
}
/// Applies one varp packet in place; on error `s` is unchanged. Every write
/// rejects before it mutates (`Varps::set_server`/`set_bit` check the index
/// and value first), so the only late failure is a payload longer than its
/// fields. That case keeps the original error precedence (the write's own
/// error, else the length) by resolving the write on a scratch copy.
pub fn apply(op: u8, bytes: &[u8], s: &mut Varps, now: i64, bits: &BitLookup) -> Result<Applied> {
    use wire::server::*;
    let mut p = Packet::new(bytes);
    let write = match op {
        VARP_LARGE => {
            let id = p.le2()?;
            let value = p.g2()?.wrapping_shl(16) | p.g2()?;
            Write::Server(id, value)
        }
        VARP_SMALL => {
            let value = p.byte()? as i8 as i32;
            let id = p.alt2()?;
            Write::Server(id, value)
        }
        VARBIT_SMALL => {
            let value = (128u32.wrapping_sub(p.byte()?) & 255) as i32;
            let id = p.alt2()?;
            Write::Bit(bits(id)?, value)
        }
        VARBIT_LARGE => {
            let id = p.alt2()?;
            let b = [p.byte()?, p.byte()?, p.byte()?, p.byte()?];
            let value = (b[0] | b[1] << 8 | b[2] << 16 | b[3] << 24) as i32;
            Write::Bit(bits(id)?, value)
        }
        RESET_CLIENT_VARCACHE => Write::Reset,
        _ => return Err(Error::UnsupportedContext("varp packet opcode")),
    };
    if p.pos != bytes.len() {
        write.apply(&mut s.clone(), now)?;
        return Err(Error::Invalid("varp payload length"));
    }
    let (outcome, transmit_increment) = write.apply(s, now)?;
    Ok(Applied {
        consumed: p.pos,
        outcome,
        transmit_increment,
    })
}
