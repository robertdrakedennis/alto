//! Script and interface project builds. Manifest: pinned archive hashes,
//! `script <id> <symbol> <path>` and `component <group> <file> <path>`.
//! Optional `names <path>` and `inames <path>` supply curated symbols.
//! Output is a new build directory;
//! the base pack is never overwritten. SHA-256 pins the source script archive.
use crate::config::ConfigTypes;
use crate::error::{NativeError, Result};
use crate::execution::Table;
use crate::inames::InterfaceRegistry;
use crate::opcode::OpcodeBook;
use crate::pack::PackArchive;
use crate::parse::{parse_source, parse_source_mapped};
use crate::script::{Counts, Operand, decode_script, encode_script};
use crate::source::lower_mapped;
use crate::symbols::SymbolRegistry;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// SHA-256 identity for source inputs and generated artifacts.
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid(message: impl Into<String>) -> NativeError {
    NativeError::Invalid(message.into())
}

/// Build into a new, atomically published directory. Existing builds are immutable.
pub fn build(manifest: &Path, pack_root: &Path, output: &Path) -> Result<usize> {
    if output.exists() {
        return Err(invalid(
            "output build already exists; choose a new build directory",
        ));
    }
    let text = std::fs::read_to_string(manifest)?;
    let base = std::fs::read(pack_root.join("client.scripts.js5"))?;
    let mut expected = None;
    let mut entries = BTreeMap::new();
    let mut component_entries = BTreeMap::new();
    let mut expected_interfaces = None;
    let mut names_path = None;
    let mut inames_path = None;
    let mut execution_path = None;
    for (line, raw) in text.lines().enumerate() {
        let row = raw.trim();
        if row.is_empty() || row.starts_with('#') {
            continue;
        }
        let columns: Vec<_> = row.splitn(4, ' ').filter(|s| !s.is_empty()).collect();
        match columns.as_slice() {
            ["base-sha256", value] if expected.is_none() => {
                if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(invalid(format!("line {}: invalid SHA-256", line + 1)));
                }
                expected = Some(value.to_ascii_lowercase());
            }
            ["interfaces-sha256", value] if expected_interfaces.is_none() => {
                expected_interfaces = Some(value.to_ascii_lowercase());
            }
            ["names", path] if names_path.is_none() => names_path = Some(*path),
            ["inames", path] if inames_path.is_none() => inames_path = Some(*path),
            ["execution", path] if execution_path.is_none() => execution_path = Some(*path),
            ["component", group, file, path] => {
                let group: u32 = group
                    .parse()
                    .map_err(|_| invalid(format!("line {}: invalid interface ID", line + 1)))?;
                let file: u32 = file
                    .parse()
                    .map_err(|_| invalid(format!("line {}: invalid component ID", line + 1)))?;
                component_address(group, file)?;
                if component_entries
                    .insert((group, file), path.to_string())
                    .is_some()
                {
                    return Err(invalid(format!("line {}: duplicate component", line + 1)));
                }
            }
            ["script", id, name, path] => {
                let id: u32 = id
                    .parse()
                    .map_err(|_| invalid(format!("line {}: invalid script ID", line + 1)))?;
                if id > i32::MAX as u32
                    || entries
                        .insert(id, (name.to_string(), path.to_string()))
                        .is_some()
                {
                    return Err(invalid(format!(
                        "line {}: duplicate/out-of-range script ID",
                        line + 1
                    )));
                }
            }
            _ => {
                return Err(invalid(format!(
                    "line {}: expected an archive hash, script, component, names or inames row",
                    line + 1
                )));
            }
        }
    }
    if expected.as_deref() != Some(sha256(&base).as_str()) {
        return Err(invalid("base-sha256 missing or mismatched"));
    }
    if entries.is_empty() && component_entries.is_empty() {
        return Err(invalid("project contains no sources"));
    }
    let source_root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let interface_base = if component_entries.is_empty() && expected_interfaces.is_none() {
        None
    } else {
        let bytes = std::fs::read(pack_root.join("client.interfaces.js5"))?;
        if expected_interfaces.as_deref() != Some(sha256(&bytes).as_str()) {
            return Err(invalid("interfaces-sha256 missing or mismatched"));
        }
        Some(bytes)
    };
    let book = OpcodeBook::embedded()?;
    let archive = PackArchive::from_bytes(base.clone())?;
    let mut known = BTreeMap::new();
    let mut compiled_scripts = BTreeMap::new();
    let mut execution = Table::default();
    for id in archive.group_ids() {
        let files = archive
            .group_files(id)?
            .ok_or_else(|| invalid(format!("missing base group {id}")))?;
        let id = i32::try_from(id).map_err(|_| invalid("script ID out of range"))?;
        let (script, metadata) = crate::execution::decode_group(id, &files, &book)?;
        execution.extend(metadata)?;
        known.insert(id, script.args);
        if !entries.contains_key(&(id as u32)) {
            compiled_scripts.insert(id, script);
        }
    }
    if let Some(path) = execution_path {
        execution.extend(Table::parse(&std::fs::read_to_string(
            source_root.join(path),
        )?)?)?;
    }
    let options = execution.analysis_options();
    let configs = ConfigTypes::load(pack_root)?;
    let inames_text = inames_path
        .map(|path| std::fs::read_to_string(source_root.join(path)))
        .transpose()?;
    let inames = if let Some(text) = &inames_text {
        let mut roster = crate::inames::load_interface_roster(pack_root)?;
        for (group, file) in component_entries.keys() {
            let children = roster
                .entry((*group).try_into().map_err(|_| invalid("interface ID"))?)
                .or_default();
            if !children.contains(file) {
                children.push(*file);
                children.sort_unstable();
            }
        }
        let (interfaces, children) = crate::inames::parse_inames_txt(text)?;
        InterfaceRegistry::build(&interfaces, &children, &roster)?
    } else {
        InterfaceRegistry::empty()
    };
    let mut sources = BTreeMap::new();
    for (id, (_, relative)) in &entries {
        let source = std::fs::read_to_string(source_root.join(relative))?;
        let header = source
            .lines()
            .find(|line| !line.trim().is_empty() && !line.trim().starts_with("//"))
            .ok_or_else(|| invalid(format!("script {id}: missing header")))?;
        let parsed = parse_source(header, &SymbolRegistry::empty(), &configs, &inames)?;
        let mut counts = Counts::default();
        for arg in parsed.args {
            match arg.ty {
                crate::source::ValType::Int => counts.int += 1,
                crate::source::ValType::String => counts.obj += 1,
                crate::source::ValType::Long => counts.long += 1,
            }
        }
        known.insert(*id as i32, counts);
        sources.insert(*id, source);
    }
    let names_text = names_path
        .map(|path| std::fs::read_to_string(source_root.join(path)))
        .transpose()?;
    let mut curated = names_text
        .as_deref()
        .map(SymbolRegistry::parse_names_txt)
        .transpose()?
        .unwrap_or_default();
    let edited_ids: BTreeSet<_> = entries.keys().map(|id| *id as i32).collect();
    curated.retain(|(id, _)| !edited_ids.contains(id));
    curated.extend(
        entries
            .iter()
            .map(|(id, (name, _))| (*id as i32, name.clone())),
    );
    let named_roots: Vec<_> = curated.iter().map(|(id, _)| *id).collect();
    let mut symbols = SymbolRegistry::build(curated, &known)?;
    let mut replacements = BTreeMap::new();
    let mut maps = BTreeMap::new();
    while replacements.len() < sources.len() {
        let dependencies = build_dependencies(&compiled_scripts, named_roots.iter().copied());
        let summaries =
            crate::dataflow::infer_summaries_with_options(&dependencies, &configs, &options);
        let counts = summaries
            .0
            .iter()
            .filter_map(|(id, arity)| match *arity {
                crate::returns::ReturnArity::Known { int, obj, long } => {
                    Some((*id, Counts { int, obj, long }))
                }
                crate::returns::ReturnArity::Unknown | crate::returns::ReturnArity::Never => None,
            })
            .collect();
        symbols =
            symbols
                .with_returns(&counts)
                .with_call_context(&dependencies, &configs, &summaries);
        let mut progress = false;
        let mut errors = Vec::new();
        for (id, source) in &sources {
            if replacements.contains_key(id) {
                continue;
            }
            let attempted = (|| -> Result<_> {
                let (parsed, lines) = parse_source_mapped(source, &symbols, &configs, &inames)?;
                let (compiled, statements) = lower_mapped(&parsed, &book, &symbols, &configs)?;
                Ok((compiled, statements, lines))
            })();
            let (compiled, statements, lines) = match attempted {
                Ok(value) => value,
                Err(error) => {
                    errors.push(format!("script {id}: {error}"));
                    continue;
                }
            };
            for instruction in &compiled.code {
                if let Operand::Script(callee) = instruction.operand
                    && !known.contains_key(&callee)
                {
                    return Err(invalid(format!("script {id}: unknown callee {callee}")));
                }
                if !crate::semantics::contract(&book, &instruction.command, &instruction.operand)?
                    .retail_dispatch
                {
                    return Err(invalid(format!(
                        "script {id}: non-retail command {}",
                        instruction.command
                    )));
                }
            }
            let bytes = encode_script(&compiled, &book)?;
            if decode_script(&bytes, &book)? != compiled {
                return Err(invalid("compiled script verification failed"));
            }
            compiled_scripts.insert(*id as i32, compiled);
            replacements.insert(*id, bytes);
            let mut mapping = String::new();
            for (pc, statement) in statements.iter().enumerate() {
                use std::fmt::Write;
                writeln!(mapping, "{pc}\t{}", lines[*statement])
                    .map_err(|_| invalid("source map formatting"))?;
            }
            maps.insert(*id, mapping);
            progress = true;
        }
        if !progress {
            return Err(invalid(errors.join("\n")));
        }
    }
    let dependencies = build_dependencies(&compiled_scripts, entries.keys().map(|id| *id as i32));
    let (returns, values) =
        crate::dataflow::infer_summaries_with_options(&dependencies, &configs, &options);
    let mut stack_analysis = String::from("script\tstatus\tpc\tcommand\treason\n");
    for id in entries.keys() {
        let script = &compiled_scripts[&(*id as i32)];
        let analysis = crate::dataflow::analyze_with_options(
            script,
            &dependencies,
            &returns,
            &values,
            &configs,
            &crate::dataflow::EntryContext::default(),
            &options,
        );
        use std::fmt::Write as _;
        if let Some((pc, reason)) = analysis.failure {
            let command = script
                .code
                .get(pc)
                .map_or("<entry>", |instruction| instruction.command.as_str());
            use crate::returns::FailureKind;
            if matches!(
                reason,
                FailureKind::InvalidOperand
                    | FailureKind::InvalidControlFlow
                    | FailureKind::StackUnderflow
                    | FailureKind::DepthOverflow
                    | FailureKind::MissingCallee
            ) {
                return Err(invalid(format!("script {id} @{pc} {command}: {reason:?}")));
            }
            let _ = writeln!(
                stack_analysis,
                "{id}\tcontext-required\t{pc}\t{command}\t{reason:?}"
            );
        } else {
            let _ = writeln!(stack_analysis, "{id}\tcomplete\t\t\t");
        }
    }
    symbols = symbols.with_returns(
        &returns
            .into_iter()
            .filter_map(|(id, arity)| match arity {
                crate::returns::ReturnArity::Known { int, obj, long } => {
                    Some((id, Counts { int, obj, long }))
                }
                crate::returns::ReturnArity::Unknown | crate::returns::ReturnArity::Never => None,
            })
            .collect(),
    );
    let mut execution_files = BTreeMap::new();
    for id in execution.ids() {
        let script = compiled_scripts
            .get(&id)
            .ok_or_else(|| invalid("execution script missing"))?;
        let bytes = encode_script(script, &book)?;
        execution.validate(id, &bytes, script)?;
        if let Some(metadata) = execution.group_bytes(id) {
            execution_files.insert(id as u32, metadata);
        }
    }
    execution.validate_links(&compiled_scripts)?;
    let packed = crate::repack::scripts_with_metadata(&base, &replacements, &execution_files)?;
    let mut component_sources = BTreeMap::new();
    let mut component_bytes = BTreeMap::new();
    for ((group, file), relative) in &component_entries {
        let source = std::fs::read_to_string(source_root.join(relative))?;
        let packed_id = component_address(*group, *file)?;
        let bytes = crate::isource::assemble_component(&source, packed_id, &symbols, &inames)
            .map_err(|error| invalid(format!("component {group}/{file}: {error}")))?;
        let component = crate::interface::decode_component(&bytes, packed_id)?;
        validate_component_hooks(&component, &compiled_scripts)
            .map_err(|error| invalid(format!("component {group}/{file}: {error}")))?;
        component_sources.insert((*group, *file), source);
        component_bytes.insert((*group, *file), bytes);
    }
    let cached_hook_input = validate_cached_hooks(
        pack_root,
        interface_base.as_deref(),
        &edited_ids,
        &component_entries,
        &compiled_scripts,
    )?;
    let packed_interfaces = interface_base
        .as_ref()
        .map(|bytes| crate::repack::interfaces(bytes, &component_bytes))
        .transpose()?;
    if let Some(bytes) = &packed_interfaces {
        validate_component_hierarchy(bytes, &component_entries)?;
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".cs2-build-{}", std::process::id()));
    std::fs::create_dir(&staging)?;
    let result = (|| -> Result<()> {
        std::fs::write(staging.join("client.scripts.js5"), &packed)?;
        std::fs::write(staging.join("project.manifest"), text)?;
        std::fs::write(staging.join("symbols.txt"), symbols.emit_symbols_txt())?;
        std::fs::write(staging.join("stack-analysis.tsv"), &stack_analysis)?;
        if !execution.is_empty() {
            std::fs::write(
                staging.join(crate::execution::PROJECT_FILENAME),
                execution.emit(),
            )?;
        }
        if let Some(text) = &names_text {
            std::fs::write(staging.join("names.txt"), text)?;
        }
        if let Some(text) = &inames_text {
            std::fs::write(staging.join("inames.txt"), text)?;
        }
        let mut report = format!(
            "base-sha256 {}\noutput-sha256 {}\nregistry-sha256 {}\nconfig-sha256 {}\nenum-sha256 {}\n",
            sha256(&base),
            sha256(&packed),
            sha256(
                &[
                    include_bytes!("semantics.rs").as_slice(),
                    include_bytes!("cs2_stack_contracts.rs").as_slice()
                ]
                .concat()
            ),
            sha256(&std::fs::read(pack_root.join("client.config.js5"))?),
            sha256(&std::fs::read(pack_root.join("client.enum.config.js5"))?)
        );
        use std::fmt::Write;
        if let Some(hash) = &cached_hook_input {
            writeln!(report, "cached-hook-input-sha256 {hash}")
                .map_err(|_| invalid("hook input manifest formatting"))?;
        }
        if let Some((base, packed)) = interface_base.as_ref().zip(packed_interfaces.as_ref()) {
            std::fs::write(staging.join("client.interfaces.js5"), packed)?;
            writeln!(
                report,
                "interfaces-base-sha256 {}\ninterfaces-output-sha256 {}",
                sha256(base),
                sha256(packed)
            )
            .map_err(|_| invalid("interface build manifest formatting"))?;
        }
        for (kind, text) in [("names", &names_text), ("inames", &inames_text)] {
            if let Some(text) = text {
                writeln!(report, "{kind}-sha256 {}", sha256(text.as_bytes()))
                    .map_err(|_| invalid("symbol manifest formatting"))?;
            }
        }
        writeln!(
            report,
            "schema-version {}\nanalysis-sha256 {}",
            crate::semantics::SCHEMA_VERSION,
            sha256(
                &[
                    include_bytes!("dataflow.rs").as_slice(),
                    include_bytes!("config.rs").as_slice(),
                    include_bytes!("dbtable.rs").as_slice()
                ]
                .concat()
            )
        )
        .map_err(|_| invalid("analysis manifest formatting"))?;
        let dbindex = pack_root.join("client.dbtableindex.js5");
        writeln!(
            report,
            "dbindex-sha256 {}",
            if dbindex.is_file() {
                sha256(&std::fs::read(dbindex)?)
            } else {
                "absent".into()
            }
        )
        .map_err(|_| invalid("DB index manifest formatting"))?;
        let mut outputs = BTreeSet::new();
        for (id, bytes) in &replacements {
            std::fs::write(staging.join(format!("{id}.bin")), bytes)?;
            std::fs::write(staging.join(format!("{id}.rs2")), &sources[id])?;
            std::fs::write(staging.join(format!("{id}.map.tsv")), &maps[id])?;
            outputs.insert(format!(
                "script {id} binary-sha256 {} source-sha256 {}\n",
                sha256(bytes),
                sha256(sources[id].as_bytes())
            ));
        }
        for line in outputs {
            report.push_str(&line);
        }
        for ((group, file), bytes) in &component_bytes {
            let stem = format!("{group}_{file}");
            std::fs::write(
                staging.join(format!("{stem}.ifc")),
                &component_sources[&(*group, *file)],
            )?;
            std::fs::write(staging.join(format!("{stem}.ifc.bin")), bytes)?;
            writeln!(
                report,
                "component {group} {file} binary-sha256 {} source-sha256 {}",
                sha256(bytes),
                sha256(component_sources[&(*group, *file)].as_bytes())
            )
            .map_err(|_| invalid("component manifest formatting"))?;
        }
        std::fs::write(staging.join("build.txt"), report)?;
        if output.exists() {
            return Err(invalid("output appeared during build"));
        }
        std::fs::rename(&staging, output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result?;
    Ok(replacements.len() + component_bytes.len())
}

fn component_address(group: u32, file: u32) -> Result<i32> {
    crate::xref::pack_component(crate::xref::ComponentRef {
        iface: group
            .try_into()
            .map_err(|_| invalid("interface ID exceeds i32"))?,
        child: file
            .try_into()
            .map_err(|_| invalid("component ID exceeds i32"))?,
    })
    .ok_or_else(|| invalid("component identity exceeds packed address range"))
}

fn validate_component_hooks(
    component: &crate::interface::InterfaceComponent,
    scripts: &BTreeMap<i32, crate::script::CompiledScript>,
) -> Result<()> {
    for (slot, hook) in crate::isource::hook_slots(&component.hooks) {
        let Some(args) = hook else { continue };
        let Some((crate::interface::HookArg::Int(id), tail)) = args.split_first() else {
            return Err(invalid(format!("{slot}: hook must start with a script ID")));
        };
        let script = scripts
            .get(id)
            .ok_or_else(|| invalid(format!("{slot}: unknown hook script {id}")))?;
        let expected = script.locals;
        let mut actual = Counts::default();
        for argument in tail {
            match argument {
                crate::interface::HookArg::Int(_) => actual.int += 1,
                crate::interface::HookArg::Str(_) => actual.obj += 1,
            }
        }
        // Hook entry fills fresh local banks by type; omitted arguments retain
        // their defaults. Only a tail exceeding a bank's local capacity is
        // invalid. Cached hooks carry no longs, whose fresh slots stay zero.
        if actual.int > expected.int || actual.obj > expected.obj {
            return Err(invalid(format!(
                "{slot}: script {id} local capacity {expected:?}, supplied {actual:?}"
            )));
        }
    }
    Ok(())
}

/// Replacing a script also changes the contract of unedited cached hooks.
/// Check their local-bank capacity without rejecting unrelated retail hooks.
fn validate_cached_hooks(
    pack_root: &Path,
    pinned_interfaces: Option<&[u8]>,
    edited_scripts: &BTreeSet<i32>,
    edited_components: &BTreeMap<(u32, u32), String>,
    scripts: &BTreeMap<i32, crate::script::CompiledScript>,
) -> Result<Option<String>> {
    if edited_scripts.is_empty() {
        return Ok(None);
    }
    let bytes = if let Some(bytes) = pinned_interfaces {
        bytes.to_vec()
    } else {
        let path = pack_root.join("client.interfaces.js5");
        if !path.is_file() {
            return Ok(None);
        }
        std::fs::read(path)?
    };
    let archive = PackArchive::from_bytes(bytes.clone())?;
    for group in archive.group_ids() {
        for (file, bytes) in archive.group_files(group)?.unwrap_or_default() {
            if edited_components.contains_key(&(group, file)) {
                continue;
            }
            let component =
                crate::interface::decode_component(&bytes, component_address(group, file)?)?;
            let affected = crate::isource::hook_slots(&component.hooks).into_iter().any(|(_, hook)|
                hook.as_ref().is_some_and(|args| matches!(args.first(), Some(crate::interface::HookArg::Int(id)) if edited_scripts.contains(id))));
            if affected {
                validate_component_hooks(&component, scripts).map_err(|error| {
                    invalid(format!("cached component {group}/{file}: {error}"))
                })?;
            }
        }
    }
    Ok(Some(sha256(&bytes)))
}

/// Edited groups include their unedited siblings in the hierarchy check.
fn validate_component_hierarchy(pack: &[u8], entries: &BTreeMap<(u32, u32), String>) -> Result<()> {
    const NO_PARENT: i32 = -1;
    let archive = PackArchive::from_bytes(pack.to_vec())?;
    let groups: BTreeSet<_> = entries.keys().map(|(group, _)| *group).collect();
    for group in groups {
        let mut parents = BTreeMap::new();
        for (file, bytes) in archive
            .group_files(group)?
            .ok_or_else(|| invalid("missing authored group"))?
        {
            let address = component_address(group, file)?;
            let component = crate::interface::decode_component(&bytes, address)?;
            parents.insert(address, component.layer);
        }
        for start in parents.keys() {
            let mut seen = BTreeSet::new();
            let mut current = *start;
            while current != NO_PARENT {
                if !seen.insert(current) {
                    return Err(invalid(format!(
                        "interface {group}: cyclic layer chain from {start}"
                    )));
                }
                current = *parents.get(&current).ok_or_else(|| {
                    invalid(format!(
                        "interface {group}: missing layer {current} from {start}"
                    ))
                })?;
            }
        }
    }
    Ok(())
}

/// Only manifest entries have callable source names. Infer their transitive
/// dependencies; unrelated retail scripts belong to the corpus coverage gate.
fn build_dependencies(
    scripts: &std::collections::BTreeMap<i32, crate::script::CompiledScript>,
    roots: impl Iterator<Item = i32>,
) -> std::collections::BTreeMap<i32, crate::script::CompiledScript> {
    crate::dataflow::dependency_closure(scripts, roots)
}

#[cfg(test)]
mod dependency_tests {
    use super::*;
    use crate::script::{CompiledScript, Instruction};
    #[test]
    fn build_scope_keeps_transitive_calls_and_cycles_but_excludes_unrelated_scripts() {
        let body = |target| CompiledScript {
            name: None,
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![Instruction {
                opcode: 0,
                command: "gosub_with_params".into(),
                operand: Operand::Script(target),
            }],
        };
        let scripts = std::collections::BTreeMap::from([
            (1, body(2)),
            (2, body(3)),
            (3, body(2)),
            (4, body(99)),
        ]);
        let selected = build_dependencies(&scripts, [1].into_iter());
        assert_eq!(selected.keys().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(selected[&2], scripts[&2]);
    }
}
