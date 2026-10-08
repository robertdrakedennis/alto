//! Dependency-ranked semantic blockers. Impact follows current first-failure
//! dependencies; it is not a promise that removing one blocker closes a script.
use crate::{
    config::ConfigTypes,
    dataflow,
    returns::{FailureKind, ReturnArity},
    script::{CompiledScript, Operand},
};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Blocker {
    pub script: i32,
    pub pc: usize,
    pub reason: FailureKind,
    pub command: String,
}
#[derive(Clone, Debug)]
pub struct Report {
    pub known: BTreeSet<i32>,
    pub never: BTreeSet<i32>,
    pub failures: BTreeMap<i32, Blocker>,
    /// Each unresolved script's chain, including its terminal blocker or cycle.
    pub chains: BTreeMap<i32, Vec<i32>>,
    pub roots: BTreeMap<Blocker, BTreeSet<i32>>,
}
pub fn analyze(scripts: &BTreeMap<i32, CompiledScript>, configs: &ConfigTypes) -> Report {
    analyze_with_summaries(
        scripts,
        configs,
        &dataflow::infer_summaries(scripts, configs),
    )
}

pub fn analyze_with_summaries(
    scripts: &BTreeMap<i32, CompiledScript>,
    configs: &ConfigTypes,
    inferred: &dataflow::Summaries,
) -> Report {
    let (summaries, values) = inferred;
    let known = summaries
        .iter()
        .filter_map(|(id, a)| matches!(a, ReturnArity::Known { .. }).then_some(*id))
        .collect();
    let failures = scripts
        .iter()
        .filter_map(|(id, script)| {
            if summaries[id] != ReturnArity::Unknown {
                return None;
            }
            let (pc, reason) =
                dataflow::analyze_with_values(script, scripts, summaries, values, configs)
                    .failure?;
            Some((
                *id,
                Blocker {
                    script: *id,
                    pc,
                    reason,
                    command: script
                        .code
                        .get(pc)
                        .map_or("<entry>", |i| i.command.as_str())
                        .to_owned(),
                },
            ))
        })
        .collect();
    let mut report = trace(scripts, known, failures);
    report.never = summaries
        .iter()
        .filter_map(|(id, a)| (*a == ReturnArity::Never).then_some(*id))
        .collect();
    report
}
fn trace(
    scripts: &BTreeMap<i32, CompiledScript>,
    known: BTreeSet<i32>,
    failures: BTreeMap<i32, Blocker>,
) -> Report {
    let mut report = Report {
        known,
        never: BTreeSet::new(),
        failures,
        chains: BTreeMap::new(),
        roots: BTreeMap::new(),
    };
    for id in report.failures.keys().copied() {
        let mut chain = Vec::new();
        let mut cursor = id;
        loop {
            if let Some(start) = chain.iter().position(|prior| *prior == cursor) {
                // Canonicalize a cycle so all entrances share the same root.
                let canonical = *chain[start..].iter().min().expect("nonempty cycle");
                chain.push(cursor);
                cursor = canonical;
                break;
            }
            chain.push(cursor);
            let blocker = &report.failures[&cursor];
            if blocker.reason != FailureKind::UnknownCalleeReturn {
                break;
            }
            let Some(instruction) = scripts.get(&cursor).and_then(|s| s.code.get(blocker.pc))
            else {
                break;
            };
            let (Operand::Script(target) | Operand::Int(target)) = instruction.operand else {
                break;
            };
            if !report.failures.contains_key(&target) {
                break;
            }
            cursor = target;
        }
        report
            .roots
            .entry(report.failures[&cursor].clone())
            .or_default()
            .insert(id);
        report.chains.insert(id, chain);
    }
    report
}
impl Report {
    pub fn ranked(&self) -> Vec<(&Blocker, &BTreeSet<i32>)> {
        let mut roots: Vec<_> = self.roots.iter().collect();
        roots.sort_by_key(|(root, affected)| (std::cmp::Reverse(affected.len()), *root));
        roots
    }
    pub fn markdown(&self) -> String {
        use std::fmt::Write;
        let mut out = format!(
            "# Retail CS2 semantic closure\n\nKnown signatures: **{}**. Non-returning scripts: **{}**. Unresolved scripts: **{}**.\n\nImpact counts follow current first-blocker call chains, include the root script, and do not predict complete resolution. Cycles remain explicit unknown-callee roots.\n\n| Root script | PC | Command | Reason | Affected scripts |\n|---|---|---|---|---|\n",
            self.known.len(),
            self.never.len(),
            self.failures.len()
        );
        for (root, affected) in self.ranked() {
            let _ = writeln!(
                out,
                "| {} | {} | `{}` | {:?} | {} |",
                root.script,
                root.pc,
                root.command,
                root.reason,
                affected.len()
            );
        }
        out
    }
    pub fn tsv(&self) -> String {
        use std::fmt::Write;
        let mut out = String::from("script\tpc\treason\tcommand\tdependency_chain\n");
        for (id, root) in &self.failures {
            let chain = self.chains[id]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" -> ");
            let _ = writeln!(
                out,
                "{id}\t{}\t{:?}\t{}\t{chain}",
                root.pc, root.reason, root.command
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{Counts, Instruction};
    fn script(command: &str, operand: Operand) -> CompiledScript {
        CompiledScript {
            name: None,
            args: Counts::default(),
            locals: Counts::default(),
            code: vec![
                Instruction {
                    opcode: 0,
                    command: command.into(),
                    operand,
                },
                Instruction {
                    opcode: 0,
                    command: "return".into(),
                    operand: Operand::Byte(0),
                },
            ],
        }
    }
    #[test]
    fn first_blocker_impact_counts_unique_callers() {
        let scripts = BTreeMap::from([
            (1, script("gosub_with_params", Operand::Script(2))),
            (2, script("unknown", Operand::Byte(0))),
            (3, script("gosub_with_params", Operand::Script(2))),
        ]);
        let report = analyze(&scripts, &ConfigTypes::empty());
        assert_eq!(report.roots.len(), 1);
        assert_eq!(report.ranked()[0].0.script, 2);
        assert_eq!(report.ranked()[0].1.len(), 3);
        assert_eq!(report.chains[&1], vec![1, 2]);
    }
    #[test]
    fn cycles_have_one_canonical_root_and_terminate() {
        let scripts = BTreeMap::from([
            (1, script("gosub_with_params", Operand::Script(2))),
            (2, script("gosub_with_params", Operand::Script(1))),
            (3, script("gosub_with_params", Operand::Script(2))),
        ]);
        let report = analyze(&scripts, &ConfigTypes::empty());
        assert_eq!(report.roots.len(), 1);
        assert_eq!(report.ranked()[0].0.script, 1);
        assert_eq!(report.ranked()[0].1.len(), 3);
    }
}
