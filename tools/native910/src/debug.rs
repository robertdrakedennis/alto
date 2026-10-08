//! Debugger control over resumable sessions. Source maps are emitted by project
//! builds; breakpoints are keyed by script identity and instruction index.
use crate::vm::{Host, ScriptProvider, Session, Snapshot, Vm, VmResult};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Location {
    pub script: Option<i32>,
    pub pc: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stop {
    Breakpoint,
    Watchpoint,
    Step,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Watchpoint {
    Variable(crate::vars::VarScope, u16, bool),
    Component(i32, i32),
}

pub struct Debugger {
    session: Session,
    pub breakpoints: BTreeSet<Location>,
    pub watchpoints: HashSet<Watchpoint>,
    pub source_maps: BTreeMap<i32, Vec<usize>>,
    last_break: Option<Location>,
}

impl Debugger {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            breakpoints: BTreeSet::new(),
            source_maps: BTreeMap::new(),
            watchpoints: HashSet::new(),
            last_break: None,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.session.snapshot()
    }
    pub fn location(&self) -> Location {
        let state = self.session.snapshot();
        Location {
            script: state.script_id,
            pc: state.pc,
        }
    }
    pub fn source_line(&self) -> Option<usize> {
        let location = self.location();
        self.source_maps
            .get(&location.script?)
            .and_then(|map| map.get(location.pc))
            .copied()
    }
    /// Add a breakpoint at the first instruction of each occurrence of a line.
    pub fn break_on_line(&mut self, script: i32, line: usize) -> bool {
        let Some(map) = self.source_maps.get(&script) else {
            return false;
        };
        let mut found = false;
        for (pc, candidate) in map.iter().enumerate() {
            if *candidate == line && (pc == 0 || map[pc - 1] != line) {
                self.breakpoints.insert(Location {
                    script: Some(script),
                    pc,
                });
                found = true;
            }
        }
        found
    }
    pub fn step_instruction<H: Host, P: ScriptProvider>(
        &mut self,
        vm: &mut Vm<'_, '_, H, P>,
    ) -> VmResult<Stop> {
        self.last_break = None;
        if vm.step(&mut self.session)? {
            return Ok(Stop::Finished);
        }
        let watched = self.session.snapshot().effects.iter().any(|effect| {
            let target = match effect {
                crate::vm::HostEffect::VariableWrite {
                    domain,
                    id,
                    secondary,
                    ..
                } => Watchpoint::Variable(*domain, *id, *secondary),
                crate::vm::HostEffect::ComponentText { packed, child, .. } => {
                    Watchpoint::Component(*packed, *child)
                }
            };
            self.watchpoints.contains(&target)
        });
        Ok(if watched {
            Stop::Watchpoint
        } else {
            Stop::Step
        })
    }
    /// Enter calls and stop at the next source location; without a map, step one instruction.
    pub fn step_source<H: Host, P: ScriptProvider>(
        &mut self,
        vm: &mut Vm<'_, '_, H, P>,
    ) -> VmResult<Stop> {
        let start = self.location();
        let line = self.source_line();
        loop {
            match self.step_instruction(vm)? {
                Stop::Finished => return Ok(Stop::Finished),
                Stop::Watchpoint => return Ok(Stop::Watchpoint),
                _ => {}
            }
            let next = self.location();
            if line.is_none()
                || next.script != start.script
                || next.pc <= start.pc
                || self.source_line() != line
            {
                return Ok(Stop::Step);
            }
        }
    }
    /// Continue, stopping before a breakpoint. A repeated continue crosses the
    /// current breakpoint once, but can stop there again after a loop/call.
    pub fn resume<H: Host, P: ScriptProvider>(
        &mut self,
        vm: &mut Vm<'_, '_, H, P>,
    ) -> VmResult<Stop> {
        loop {
            if self.session.finished() {
                return Ok(Stop::Finished);
            }
            let at = self.location();
            if self.breakpoints.contains(&at) && self.last_break != Some(at) {
                self.last_break = Some(at);
                return Ok(Stop::Breakpoint);
            }
            match self.step_instruction(vm)? {
                Stop::Finished => return Ok(Stop::Finished),
                Stop::Watchpoint => return Ok(Stop::Watchpoint),
                _ => {}
            }
        }
    }
}
