//! Opcode tables: how one config type is laid out in the cache.
//!
//! A config file is a run of `opcode, payload` records ended by opcode 0. What
//! each opcode means for a given type (which field it sets, how the payload
//! is read, whether it carries no payload at all) is revision data: it lives
//! in one static [`Table`] per type, next to the type. [`Table::run`] is the
//! only loop that walks a file. Payloads that are not one plain read (counted
//! lists, bit-mask tables, opcodes that touch several fields) name a function
//! in the table instead ([`Rule::Custom`]).
//!
//! The reads themselves come from a [`Source`]. [`ConfigReader`] is the
//! `anyhow` flavoured one used by the config stores; the entity decoders in
//! [`crate::types910`] implement [`Source`] for their own packet type.

use std::sync::OnceLock;

use anyhow::Context;
use rs910_core::reader::{Eof, Reader};

/// Which record of which type is being decoded, for error messages.
#[derive(Clone, Copy, Debug)]
pub struct Record {
    /// The type name used in messages (`"loc"`, `"npc"`).
    pub kind: &'static str,
    /// The config id.
    pub id: i64,
}

/// Position of an opcode inside a multi-opcode table entry (`opcode - first`).
pub type Slot = usize;

/// One payload field, for opcodes whose payload is read and dropped.
#[derive(Clone, Copy, Debug)]
pub enum Field {
    Byte,
    Short,
    Medium,
    Int,
    Text,
    /// One or two bytes, see [`Source::smart`].
    Smart,
    /// Two or four bytes, see [`Source::smart_id`].
    SmartId,
}

/// What to do with an opcode that has no table entry.
pub enum Unknown<E> {
    /// Fail with the source's own unknown-opcode error: the payload length is
    /// unknown, so reading on would misalign.
    Reject,
    /// Fail with an error of the table's own.
    Fail(fn(Record, u8) -> E),
    /// Consume nothing and read the next byte as an opcode (types whose
    /// stored data is known to contain such bytes).
    Skip,
}

/// The reads a table needs. Only the byte, peek and text reads are
/// required; the composite reads default to big-endian combinations of
/// bytes, which a source overrides when it has a faster or stricter form.
pub trait Source {
    type Error;

    /// The next opcode. A file that ends before its terminator is an error
    /// that names `record`.
    fn opcode(&mut self, record: Record) -> Result<u8, Self::Error>;
    /// The error for an opcode the table does not know.
    fn unknown_opcode(&self, record: Record, opcode: u8) -> Self::Error;

    fn byte(&mut self) -> Result<u8, Self::Error>;
    /// The next byte, not consumed.
    fn peek(&self) -> Result<u8, Self::Error>;
    /// NUL-terminated Windows-1252 text.
    fn text(&mut self) -> Result<String, Self::Error>;

    fn signed_byte(&mut self) -> Result<i8, Self::Error> {
        Ok(self.byte()? as i8)
    }
    /// Big-endian unsigned 16-bit.
    fn short(&mut self) -> Result<u16, Self::Error> {
        let high = u16::from(self.byte()?);
        Ok(high << 8 | u16::from(self.byte()?))
    }
    /// Big-endian signed 16-bit.
    fn signed_short(&mut self) -> Result<i16, Self::Error> {
        Ok(self.short()? as i16)
    }
    /// Big-endian unsigned 24-bit.
    fn medium(&mut self) -> Result<u32, Self::Error> {
        let high = u32::from(self.short()?);
        Ok(high << 8 | u32::from(self.byte()?))
    }
    /// Big-endian signed 32-bit.
    fn int(&mut self) -> Result<i32, Self::Error> {
        let high = u32::from(self.short()?);
        Ok((high << 16 | u32::from(self.short()?)) as i32)
    }
    /// A byte below 128, or a 16-bit value with the top bit set (minus 32768).
    fn smart(&mut self) -> Result<i32, Self::Error> {
        if self.peek()? < 128 {
            Ok(i32::from(self.byte()?))
        } else {
            Ok(i32::from(self.short()?) - 32768)
        }
    }
    /// [`Source::smart`] with the signed bias (values start at -64 / -16384).
    fn smart_signed(&mut self) -> Result<i32, Self::Error> {
        if self.peek()? < 128 {
            Ok(i32::from(self.byte()?) - 64)
        } else {
            Ok(i32::from(self.short()?) - 49152)
        }
    }
    /// [`Source::smart`] where a stored 0 means "none" (`-1`) and the rest is
    /// shifted down by one.
    fn smart_nullable(&mut self) -> Result<i32, Self::Error> {
        if self.peek()? < 128 {
            Ok(i32::from(self.byte()?) - 1)
        } else {
            Ok(i32::from(self.short()?) - 32769)
        }
    }
    /// A 16-bit id, or (top bit set) a 31-bit id in four bytes; 32767 is `-1`.
    fn smart_id(&mut self) -> Result<i32, Self::Error> {
        if self.peek()? >= 128 {
            Ok(self.int()? & i32::MAX)
        } else {
            let value = i32::from(self.short()?);
            Ok(if value == 32767 { -1 } else { value })
        }
    }
}

/// The source handed to custom payload readers.
pub type Input<'a, E> = &'a mut dyn Source<Error = E>;

/// How one entry of a table reads its payload and where it puts it. The
/// setters receive the record under construction, the [`Slot`] of the opcode
/// inside its entry, and the value.
pub enum Rule<T, E> {
    /// No payload.
    Flag(fn(&mut T, Slot)),
    /// One unsigned byte.
    Byte(fn(&mut T, Slot, u8)),
    /// One signed byte.
    SignedByte(fn(&mut T, Slot, i8)),
    /// A big-endian unsigned 16-bit value.
    Short(fn(&mut T, Slot, u16)),
    /// A big-endian signed 16-bit value.
    SignedShort(fn(&mut T, Slot, i16)),
    /// A big-endian unsigned 24-bit value.
    Medium(fn(&mut T, Slot, u32)),
    /// A big-endian signed 32-bit value.
    Int(fn(&mut T, Slot, i32)),
    /// NUL-terminated text.
    Text(fn(&mut T, Slot, String)),
    /// A one-or-two byte smart, see [`Source::smart`].
    Smart(fn(&mut T, Slot, i32)),
    /// A two-or-four byte id, see [`Source::smart_id`].
    SmartId(fn(&mut T, Slot, i32)),
    /// Payload read and dropped.
    Skip(&'static [Field]),
    /// Anything that is not one plain read: counted lists, bit masks, opcodes
    /// that set several fields.
    Custom(fn(Input<E>, &mut T, Slot) -> Result<(), E>),
}

/// The opcodes `first..=last` share one rule.
pub struct Entry<T, E> {
    pub first: u8,
    pub last: u8,
    pub rule: Rule<T, E>,
}

/// One opcode.
pub const fn at(opcode: u8) -> (u8, u8) {
    (opcode, opcode)
}

/// A run of consecutive opcodes; the rule sees each one's slot.
pub const fn span(first: u8, last: u8) -> (u8, u8) {
    (first, last)
}

impl<T, E> Entry<T, E> {
    pub const fn new(ops: (u8, u8), rule: Rule<T, E>) -> Self {
        Self {
            first: ops.0,
            last: ops.1,
            rule,
        }
    }
}

/// The opcode table of one config type.
pub struct Table<T: 'static, E: 'static> {
    entries: &'static [Entry<T, E>],
    unknown: Unknown<E>,
    index: OnceLock<[u16; 256]>,
}

const NO_ENTRY: u16 = u16::MAX;

impl<T, E> Table<T, E> {
    pub const fn new(entries: &'static [Entry<T, E>], unknown: Unknown<E>) -> Self {
        Self {
            entries,
            unknown,
            index: OnceLock::new(),
        }
    }

    fn entry_for(&self, opcode: u8) -> Option<&Entry<T, E>> {
        let index = self.index.get_or_init(|| {
            let mut index = [NO_ENTRY; 256];
            for (position, entry) in self.entries.iter().enumerate() {
                for opcode in entry.first..=entry.last {
                    assert_eq!(
                        index[usize::from(opcode)],
                        NO_ENTRY,
                        "opcode {opcode} appears twice in one table"
                    );
                    index[usize::from(opcode)] = position as u16;
                }
            }
            index
        });
        match index[usize::from(opcode)] {
            NO_ENTRY => None,
            position => Some(&self.entries[usize::from(position)]),
        }
    }

    /// Read records into `state` until the terminator.
    pub fn run(
        &self,
        source: &mut dyn Source<Error = E>,
        state: &mut T,
        record: Record,
    ) -> Result<(), E> {
        loop {
            let opcode = source.opcode(record)?;
            if opcode == 0 {
                return Ok(());
            }
            let Some(entry) = self.entry_for(opcode) else {
                match self.unknown {
                    Unknown::Skip => continue,
                    Unknown::Fail(error) => return Err(error(record, opcode)),
                    Unknown::Reject => return Err(source.unknown_opcode(record, opcode)),
                }
            };
            let slot = usize::from(opcode - entry.first);
            match &entry.rule {
                Rule::Flag(set) => set(state, slot),
                Rule::Byte(set) => set(state, slot, source.byte()?),
                Rule::SignedByte(set) => set(state, slot, source.signed_byte()?),
                Rule::Short(set) => set(state, slot, source.short()?),
                Rule::SignedShort(set) => set(state, slot, source.signed_short()?),
                Rule::Medium(set) => set(state, slot, source.medium()?),
                Rule::Int(set) => set(state, slot, source.int()?),
                Rule::Text(set) => set(state, slot, source.text()?),
                Rule::Smart(set) => set(state, slot, source.smart()?),
                Rule::SmartId(set) => set(state, slot, source.smart_id()?),
                Rule::Skip(fields) => {
                    for field in *fields {
                        match field {
                            Field::Byte => drop(source.byte()?),
                            Field::Short => drop(source.short()?),
                            Field::Medium => drop(source.medium()?),
                            Field::Int => drop(source.int()?),
                            Field::Text => drop(source.text()?),
                            Field::Smart => drop(source.smart()?),
                            Field::SmartId => drop(source.smart_id()?),
                        }
                    }
                }
                Rule::Custom(read) => read(source, state, slot)?,
            }
        }
    }
}

/// The record reader of the config stores: `anyhow` errors that name the
/// record and the offset of the missing byte.
pub struct ConfigReader<'a> {
    inner: Reader<'a>,
    what: &'static str,
}

impl<'a> ConfigReader<'a> {
    /// A reader over one record; `what` names the data in error messages.
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self {
            inner: Reader::new(data),
            what,
        }
    }

    /// The offset of the next unread byte.
    pub fn pos(&self) -> usize {
        self.inner.pos()
    }

    fn truncated(&self, eof: Eof) -> anyhow::Error {
        anyhow::anyhow!("truncated {} data at offset {}", self.what, eof.pos)
    }

    fn read<V>(
        &mut self,
        read: impl FnOnce(&mut Reader<'a>) -> Result<V, Eof>,
    ) -> anyhow::Result<V> {
        read(&mut self.inner).map_err(|eof| self.truncated(eof))
    }
}

impl Source for ConfigReader<'_> {
    type Error = anyhow::Error;

    fn opcode(&mut self, record: Record) -> anyhow::Result<u8> {
        self.byte()
            .with_context(|| format!("{} {}: missing opcode terminator", record.kind, record.id))
    }

    fn unknown_opcode(&self, record: Record, opcode: u8) -> anyhow::Error {
        anyhow::anyhow!("{} {}: unknown opcode {opcode}", record.kind, record.id)
    }

    fn byte(&mut self) -> anyhow::Result<u8> {
        self.read(Reader::g1)
    }

    fn signed_byte(&mut self) -> anyhow::Result<i8> {
        self.read(Reader::g1b)
    }

    fn short(&mut self) -> anyhow::Result<u16> {
        self.read(Reader::g2)
    }

    fn signed_short(&mut self) -> anyhow::Result<i16> {
        self.read(Reader::g2s)
    }

    fn medium(&mut self) -> anyhow::Result<u32> {
        self.read(Reader::g3)
    }

    fn int(&mut self) -> anyhow::Result<i32> {
        self.read(Reader::g4s)
    }

    /// Client Windows-1252 text; a missing terminator is an error.
    fn text(&mut self) -> anyhow::Result<String> {
        let start = self.inner.pos();
        let bytes = self
            .read(Reader::gjstr_bytes)
            .map_err(|_| anyhow::anyhow!("unterminated string starting at offset {start}"))?;
        Ok(bytes
            .iter()
            .copied()
            .map(rs910_core::cp1252::cp1252_decode_byte)
            .collect())
    }

    fn smart(&mut self) -> anyhow::Result<i32> {
        self.read(Reader::gsmart1or2)
    }

    fn smart_signed(&mut self) -> anyhow::Result<i32> {
        self.read(Reader::gsmart1or2s)
    }

    fn smart_nullable(&mut self) -> anyhow::Result<i32> {
        self.read(Reader::gsmart1or2null)
    }

    fn smart_id(&mut self) -> anyhow::Result<i32> {
        self.read(Reader::gsmart2or4s)
    }

    fn peek(&self) -> anyhow::Result<u8> {
        self.inner.peek().map_err(|eof| self.truncated(eof))
    }
}

/// Run `table` over `data` from a fresh state and return the record.
pub fn decode_record<T>(
    table: &Table<T, anyhow::Error>,
    what: &'static str,
    record: Record,
    data: &[u8],
    mut state: T,
) -> anyhow::Result<T> {
    let mut reader = ConfigReader::new(data, what);
    table.run(&mut reader, &mut state, record)?;
    Ok(state)
}

/// Opcode payload helpers shared by several config tables.
pub mod payload {
    use super::Input;

    /// A count byte, then that many `(from, to)` pairs of 16-bit values.
    pub fn pairs<E>(source: Input<E>) -> Result<(Vec<u16>, Vec<u16>), E> {
        let count = source.byte()?;
        let mut from = Vec::with_capacity(usize::from(count));
        let mut to = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            from.push(source.short()?);
            to.push(source.short()?);
        }
        Ok((from, to))
    }

    /// A 16-bit id where 65535 means "none" (`-1`).
    pub fn nullable_short<E>(source: Input<E>) -> Result<i32, E> {
        Ok(match source.short()? {
            65535 => -1,
            value => i32::from(value),
        })
    }
}
