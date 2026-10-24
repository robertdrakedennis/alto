//! Real cache CS2 scripts executed by the Rust VM, compared instruction by
//! instruction against traces recorded ONCE from the original client's script
//! interpreter.
//!
//! Fixtures (`tests/fixtures/cs2-traces/`):
//! * `cases.txt` — `name<TAB>root script id<TAB>typed args` (`-` = none;
//!   `i:<int>`, `l:<long>`, `s:<UTF-16 units as 4-digit hex>`).
//! * `cache-scripts.txt` — the ids of the cache scripts the cases need (every
//!   root plus its transitive gosub callees). Their bytes are read from
//!   `server/data/pack/client.scripts.js5` at test time and never stored in
//!   the repository, so this test needs the local revision-910 cache.
//! * `scripts/<id>.bin` — three synthetic edge-case scripts (900000..) that
//!   exercise every VM-core command the real pure scripts never reach (long
//!   branches, arrays, bit ranges, int edge cases, mixed-lane gosub args).
//! * `traces/<name>.dig` — the recorded interpreter output of a real script,
//!   one FNV-1a 64 digest per executed instruction (script id, pc, int/object/
//!   long stacks, frame depth, int/object/long locals; see
//!   `common::line_digest`), so a mismatch still names the first divergent
//!   step. `traces/edge.trace` is the same trace of the synthetic scripts,
//!   stored verbatim.
//!
//! The cases were chosen by a corpus scan: scripts whose whole gosub closure
//! uses only VM-core commands, with argument vectors that reach every such
//! command, `divide`/`modulo` on asymmetric and negative operands,
//! `branch_less_than` on equal operands, both `testbit` outcomes and gosubs
//! whose same-lane arguments differ (so argument order is observable). The
//! recordings are not regenerated.
mod common;

use native910::opcode::OpcodeBook;
use native910::runtime::RuntimeHost;
use native910::script::{CompiledScript, decode_script};
use native910::vm::{Session, Value, Vm};
use std::collections::{BTreeSet, HashMap};

const FIXTURE: &str = "cs2-traces";
/// First id reserved for the synthetic edge-case scripts.
const SYNTHETIC_BASE: i32 = 900_000;

struct Case {
    name: String,
    root: i32,
    /// Raw `cases.txt` argument column.
    typed: String,
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(common::fixture(FIXTURE).join("cases.txt")).unwrap();
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut cols = line.split('\t');
            Case {
                name: cols.next().unwrap().to_string(),
                root: cols.next().unwrap().parse().unwrap(),
                typed: cols.next().unwrap().to_string(),
            }
        })
        .collect()
}

/// `cases.txt` arguments as VM values, lane order preserved.
fn values(typed: &str) -> Vec<Value> {
    if typed == "-" {
        return Vec::new();
    }
    typed
        .split(',')
        .map(|arg| {
            let (lane, value) = arg.split_at(2);
            match lane {
                "i:" => Value::Int(value.parse().unwrap()),
                "l:" => Value::Long(value.parse().unwrap()),
                "s:" => {
                    let units: Vec<u16> = (0..value.len())
                        .step_by(4)
                        .map(|i| u16::from_str_radix(&value[i..i + 4], 16).unwrap())
                        .collect();
                    Value::Str(native910::jstr::from_units(&units))
                }
                _ => panic!("bad typed arg {arg}"),
            }
        })
        .collect()
}

/// Every script the cases need, decoded by the code under test and named by
/// id (the recorded traces name scripts by id): the cache scripts listed in
/// `cache-scripts.txt` from the pack, plus the committed synthetic ones.
fn committed_scripts(book: &OpcodeBook) -> HashMap<i32, CompiledScript> {
    let mut scripts = HashMap::new();
    let mut add = |id: i32, bytes: &[u8]| {
        let mut script = decode_script(bytes, book)
            .unwrap_or_else(|error| panic!("decode script {id}: {error}"));
        script.name = Some(id.to_string());
        scripts.insert(id, script);
    };
    let archive =
        native910::pack::PackArchive::open(&common::require_pack_file("client.scripts.js5"))
            .unwrap();
    let listed =
        std::fs::read_to_string(common::fixture(FIXTURE).join("cache-scripts.txt")).unwrap();
    for id in listed
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
    {
        let id: i32 = id.trim().parse().unwrap();
        let bytes = archive
            .group_files(id as u32)
            .unwrap()
            .unwrap_or_else(|| panic!("script {id} is not in the pack"))
            .remove(&0)
            .unwrap();
        add(id, &bytes);
    }
    let dir = common::fixture(FIXTURE).join("scripts");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let id: i32 = path.file_stem().unwrap().to_str().unwrap().parse().unwrap();
        assert!(id >= SYNTHETIC_BASE, "cache script {id} committed");
        add(id, &std::fs::read(&path).unwrap());
    }
    scripts
}

/// The Rust VM's trace in the recorded format, or the failure after the
/// lines produced so far.
fn native_trace(
    scripts: &HashMap<i32, CompiledScript>,
    root: i32,
    args: &[Value],
) -> (Vec<String>, Option<String>) {
    let mut host = RuntimeHost::default();
    let mut vm = Vm::new(&mut host, scripts);
    let mut session = match Session::new(&scripts[&root], args) {
        Ok(session) => session,
        Err(error) => return (Vec::new(), Some(format!("session: {error}"))),
    };
    let mut lines = Vec::new();
    while !session.finished() {
        let s = session.snapshot();
        lines.push(format!(
            "{}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}",
            s.script_name.as_deref().unwrap_or("null"),
            s.pc,
            s.ints,
            common::utf16_objects(s.strings.iter().map(Option::as_deref)),
            s.longs,
            s.frames.len(),
            s.int_locals,
            common::utf16_objects(s.string_locals.iter().map(Option::as_deref)),
            s.long_locals
        ));
        if let Err(error) = vm.step(&mut session) {
            return (lines, Some(error.to_string()));
        }
    }
    (lines, None)
}

fn command_at(scripts: &HashMap<i32, CompiledScript>, line: &str) -> String {
    let mut cols = line.split('\t');
    let (Some(id), Some(pc)) = (cols.next(), cols.next()) else {
        return "?".into();
    };
    id.parse::<i32>()
        .ok()
        .and_then(|id| scripts.get(&id))
        .zip(pc.parse::<usize>().ok())
        .and_then(|(script, pc)| script.code.get(pc))
        .map_or_else(
            || "?".into(),
            |ins| format!("{} {:?}", ins.command, ins.operand),
        )
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_cache_scripts_match_recorded_traces() {
    let book = OpcodeBook::embedded().unwrap();
    let scripts = committed_scripts(&book);
    let cases = cases();
    assert!(cases.len() >= 20, "{} cases", cases.len());
    let mut executed: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let base = common::fixture(FIXTURE).join("traces").join(&case.name);
        let (native, failure) = native_trace(&scripts, case.root, &values(&case.typed));
        if let Some(step) = common::first_trace_divergence(&base, &native) {
            // The recording may end first, or the Rust VM may have stopped early.
            panic!(
                "{}: first divergence from the recording at step {step} (Rust VM produced {} steps, failure {failure:?}); previous instruction: {}",
                case.name,
                native.len(),
                if step == 0 {
                    "(entry)".to_string()
                } else {
                    command_at(&scripts, &native[step - 1])
                }
            );
        }
        assert_eq!(
            failure,
            None,
            "{}: Rust VM failed after {} matching steps (the recording completed)",
            case.name,
            native.len()
        );
        for line in &native {
            let command = command_at(&scripts, line);
            executed.insert(command.split(' ').next().unwrap().to_string());
        }
    }
    // Every VM-core command (semantics::execution_family != HostRequired)
    // must be executed against the recordings by at least one case, except the
    // var/varbit lane (needs a variable host; covered by timer_acceptance's
    // client var 995) and the retail-trap family. `push_long_constant` never
    // appears as a decoded command: the cache carries long constants as
    // `push_constant_string` with the long tag, which the original client
    // rewrites to a long-constant push at decode and the Rust decoder keeps as
    // `push_constant_string` + `Operand::Long` — the traces cover that path.
    let missing: Vec<&str> = book
        .commands()
        .filter(|name| {
            !matches!(
                native910::semantics::execution_family(name),
                native910::semantics::ExecutionFamily::HostRequired
                    | native910::semantics::ExecutionFamily::Trap
            )
        })
        .filter(|name| {
            !matches!(
                *name,
                "push_var" | "pop_var" | "push_varbit" | "pop_varbit" | "push_long_constant"
            )
        })
        .filter(|name| !executed.contains(*name))
        .collect();
    assert!(
        missing.is_empty(),
        "VM-core commands never traced: {missing:?}"
    );
}
