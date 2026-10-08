//! Identity kit configs (player body-part variants). Null model arrays and
//! signed model ids are retained; the legacy avatar chooser drops them and is
//! not the production renderer. An opcode outside the table is an error
//! naming the id and the opcode.
use super::{Error, Packet, Result};
use crate::opcode_table::{at, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown};

/// One identity kit.
#[derive(Clone, Debug, Default)]
pub struct IdentityKit {
    pub models: Option<Vec<i32>>,
    pub recolour: Option<(Vec<i16>, Vec<i16>)>,
    pub retexture: Option<(Vec<i16>, Vec<i16>)>,
}

/// Identity kit opcodes of this revision.
static KIT_OPCODES: Table<IdentityKit, Error> = Table::new(
    &[
        // Body part, dropped.
        Entry::new(at(1), Rule::Skip(&[Field::Byte])),
        Entry::new(at(2), Rule::Custom(read_models)),
        Entry::new(at(3), Rule::Skip(&[])),
        Entry::new(span(40, 41), Rule::Custom(read_pairs)),
        Entry::new(span(44, 45), Rule::Skip(&[Field::Short])),
        // Head models, dropped.
        Entry::new(span(60, 64), Rule::Skip(&[Field::SmartId])),
    ],
    Unknown::Reject,
);

fn read_models(source: Input<Error>, kit: &mut IdentityKit, _: Slot) -> Result<()> {
    let count = source.byte()?;
    kit.models = Some(
        (0..count)
            .map(|_| source.smart_id())
            .collect::<Result<_>>()?,
    );
    Ok(())
}

/// Recolour (first opcode) or retexture pairs: a count, then `(source,
/// destination)` signed shorts.
fn read_pairs(source: Input<Error>, kit: &mut IdentityKit, slot: Slot) -> Result<()> {
    let count = source.byte()?;
    let mut src = vec![];
    let mut dst = vec![];
    for _ in 0..count {
        src.push(source.short()? as i16);
        dst.push(source.short()? as i16);
    }
    if slot == 0 {
        kit.recolour = Some((src, dst));
    } else {
        kit.retexture = Some((src, dst));
    }
    Ok(())
}

impl IdentityKit {
    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut kit = Self::default();
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "identity kit",
            id: i64::from(id),
        };
        KIT_OPCODES.run(&mut packet, &mut kit, record)?;
        Ok(kit)
    }
}
