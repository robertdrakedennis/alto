//! Conservative interprocedural resource footprints. Array IDs and variable
//! addresses survive calls; local banks are private to a frame. Host operations
//! remain named opaque effects until their individual resource contracts exist.
use crate::{
    script::{CompiledScript, Operand},
    semantics::{ExecutionFamily, execution_family},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Footprint {
    pub array_reads: BTreeSet<i32>,
    pub array_writes: BTreeSet<i32>,
    /// Domain (None for the global varbit namespace), ID, secondary context.
    pub variable_reads: BTreeSet<(Option<u8>, u16, bool)>,
    pub variable_writes: BTreeSet<(Option<u8>, u16, bool)>,
    /// Commands requiring the selected component; exact component remains dynamic.
    pub component_contexts: BTreeSet<bool>,
    pub host_commands: BTreeSet<String>,
    pub terminal_commands: BTreeSet<String>,
    pub missing_callees: BTreeSet<i32>,
    pub invalid_flow: bool,
}
impl Footprint {
    fn shape(&self) -> (usize, usize, usize, usize, usize, usize, usize, usize, bool) {
        (
            self.array_reads.len(),
            self.array_writes.len(),
            self.variable_reads.len(),
            self.variable_writes.len(),
            self.component_contexts.len(),
            self.host_commands.len(),
            self.terminal_commands.len(),
            self.missing_callees.len(),
            self.invalid_flow,
        )
    }
    fn absorb(&mut self, other: &Self) -> bool {
        let before = self.shape();
        self.array_reads.extend(&other.array_reads);
        self.array_writes.extend(&other.array_writes);
        self.variable_reads.extend(&other.variable_reads);
        self.variable_writes.extend(&other.variable_writes);
        self.component_contexts.extend(&other.component_contexts);
        self.host_commands
            .extend(other.host_commands.iter().cloned());
        self.terminal_commands
            .extend(other.terminal_commands.iter().cloned());
        self.missing_callees.extend(&other.missing_callees);
        self.invalid_flow |= other.invalid_flow;
        self.shape() != before
    }
}
/// Union reachable resource effects through calls, including recursive cycles.
/// Opaque host effects are retained; this is never a purity certificate.
pub fn infer(scripts: &BTreeMap<i32, CompiledScript>) -> BTreeMap<i32, Footprint> {
    let mut output = BTreeMap::new();
    let mut callers: BTreeMap<i32, BTreeSet<i32>> = BTreeMap::new();
    for (id, script) in scripts {
        let mut result = Footprint::default();
        let mut queue = VecDeque::from([0]);
        let mut seen = BTreeSet::new();
        while let Some(pc) = queue.pop_front() {
            if !seen.insert(pc) {
                continue;
            }
            let Some(instruction) = script.code.get(pc) else {
                result.invalid_flow = true;
                continue;
            };
            let name = instruction.command.as_str();
            let edges = crate::returns::flow(instruction, pc, script.code.len());
            result.invalid_flow |= edges.lost;
            queue.extend(edges.next);
            match &instruction.operand {
                Operand::VarRef(var) => {
                    let key = (Some(u8::from(var.domain)), var.id, var.transmog);
                    if name == "push_var" {
                        result.variable_reads.insert(key);
                    } else if name == "pop_var" {
                        result.variable_writes.insert(key);
                    }
                }
                Operand::VarBitRef(var) => {
                    let key = (None, var.id, var.transmog);
                    if name == "push_varbit" {
                        result.variable_reads.insert(key);
                    } else if name == "pop_varbit" {
                        result.variable_writes.insert(key);
                    }
                }
                Operand::Array(array) | Operand::Int(array) if name.contains("array") => {
                    if name == "define_array" {
                        result.array_writes.insert(array >> 16);
                    } else if name.starts_with("pop_array") {
                        result.array_writes.insert(*array);
                    } else if name.starts_with("push_array") {
                        result.array_reads.insert(*array);
                    }
                }
                _ => {}
            }
            if name == "gosub_with_params"
                && let Operand::Script(target) | Operand::Int(target) = instruction.operand
            {
                if scripts.contains_key(&target) {
                    callers.entry(target).or_default().insert(*id);
                } else {
                    result.missing_callees.insert(target);
                }
            }
            if name.starts_with("cc_") || matches!(name, "if_find" | "getparentlayer_alias") {
                result
                    .component_contexts
                    .insert(instruction.operand == Operand::Byte(1));
            }
            if execution_family(name) == ExecutionFamily::Trap {
                result.terminal_commands.insert(name.to_owned());
            }
            if execution_family(name) == ExecutionFamily::HostRequired {
                result.host_commands.insert(name.to_owned());
            }
        }
        output.insert(*id, result);
    }
    let mut queue: VecDeque<_> = scripts.keys().copied().collect();
    let mut queued: BTreeSet<_> = scripts.keys().copied().collect();
    while let Some(id) = queue.pop_front() {
        queued.remove(&id);
        let summary = output[&id].clone();
        if let Some(dependents) = callers.get(&id) {
            for dependent in dependents {
                if output
                    .get_mut(dependent)
                    .expect("rostered caller")
                    .absorb(&summary)
                    && queued.insert(*dependent)
                {
                    queue.push_back(*dependent);
                }
            }
        }
    }
    output
}
