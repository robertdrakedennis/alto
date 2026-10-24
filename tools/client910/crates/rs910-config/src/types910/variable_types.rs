//! Player and NPC variable definitions (varp / varn): data type, lifetime and
//! the client-code hook. Player and NPC definitions share one table, with a
//! few opcodes that are valid for the player domain only. An opcode outside
//! the table is an error naming the id and the opcode, even where the stored
//! data would carry no payload.
use super::{script_types::script_type, Error, Packet, Result};
use crate::opcode_table::{at, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown};

/// Which variable list a definition belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Player,
    Npc,
}

impl Domain {
    pub fn id(self) -> u8 {
        match self {
            Self::Player => 0,
            Self::Npc => 1,
        }
    }
}

/// One variable definition. Player and NPC legacy booleans default to -1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub id: i32,
    pub domain: Domain,
    pub data_type: Option<u8>,
    pub lifetime: Option<u8>,
    pub legacy: bool,
    pub client_code: i32,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

impl Variable {
    pub fn empty(domain: Domain, id: i32) -> Self {
        Self {
            id,
            domain,
            data_type: None,
            lifetime: Some(0),
            legacy: true,
            client_code: 0,
            consumed: 0,
            raw: vec![],
        }
    }

    pub fn decode(domain: Domain, id: i32, bytes: &[u8]) -> Result<Self> {
        let mut variable = Self::empty(domain, id);
        variable.raw = bytes.into();
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "variable",
            id: i64::from(id),
        };
        VARIABLE_OPCODES.run(&mut packet, &mut variable, record)?;
        variable.consumed = packet.pos;
        Ok(variable)
    }

    pub fn base_type(&self) -> Option<u8> {
        self.data_type.and_then(script_type).map(|t| t.0)
    }
}

/// Variable opcodes of this revision.
static VARIABLE_OPCODES: Table<Variable, Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(skip_debug_name)),
        Entry::new(
            at(2),
            Rule::Custom(|_, _, _| Err(Error::Invalid("variable domain opcode is not allowed"))),
        ),
        Entry::new(at(3), Rule::Custom(read_data_type)),
        Entry::new(
            at(4),
            Rule::Byte(|v, _, value| v.lifetime = if value <= 2 { Some(value) } else { None }),
        ),
        Entry::new(at(5), Rule::Skip(&[Field::Byte])),
        Entry::new(at(6), Rule::Skip(&[])),
        Entry::new(at(7), Rule::Flag(|v, _| v.legacy = false)),
        Entry::new(
            span(100, 109),
            Rule::Custom(|_, v, slot| player_only(v, 100 + slot)),
        ),
        Entry::new(at(110), Rule::Custom(read_client_code)),
        Entry::new(
            span(111, 117),
            Rule::Custom(|_, v, slot| player_only(v, 111 + slot)),
        ),
    ],
    Unknown::Reject,
);

/// A debug-name prefix: a version byte that must be 0, then the name.
fn skip_debug_name(source: Input<Error>, _: &mut Variable, _: Slot) -> Result<()> {
    if source.byte()? != 0 {
        return Err(Error::Invalid("variable debug-name prefix"));
    }
    source.text()?;
    Ok(())
}

/// The data type must be a known script type.
fn read_data_type(source: Input<Error>, variable: &mut Variable, _: Slot) -> Result<()> {
    let data_type = source.byte()?;
    if script_type(data_type).is_none() {
        return Err(Error::Invalid("unknown script variable type"));
    }
    variable.data_type = Some(data_type);
    Ok(())
}

/// Opcodes that exist for player variables only, without payload.
fn player_only(variable: &Variable, opcode: usize) -> Result<()> {
    if variable.domain == Domain::Player {
        Ok(())
    } else {
        Err(Error::UnsupportedConfig {
            kind: "variable",
            id: variable.id,
            opcode: opcode as u8,
        })
    }
}

/// The client-code hook id; player variables only.
fn read_client_code(source: Input<Error>, variable: &mut Variable, _: Slot) -> Result<()> {
    if variable.domain != Domain::Player {
        return Err(Error::UnsupportedConfig {
            kind: "variable",
            id: variable.id,
            opcode: 110,
        });
    }
    variable.client_code = i32::from(source.short()?);
    Ok(())
}
