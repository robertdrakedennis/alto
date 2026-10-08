//! List retail commands whose semantic type rule is still unresolved.
use native910::{
    opcode::OpcodeBook,
    script::Operand,
    semantics::{self, TypeRule},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let book = OpcodeBook::embedded()?;
    for (id, name) in book.entries().filter(|(id, _)| *id <= 1431) {
        let contract = semantics::contract(&book, name, &Operand::Byte(0))?;
        if contract.type_rule == TypeRule::Unknown {
            println!("{id}\t{name}");
        }
    }
    Ok(())
}
