//! Versioned, source-pinned static evidence for agents. An unresolved site is
//! retained separately from a proven edge; resource unions remain conservative.
use crate::{
    config::ConfigTypes,
    error::Result,
    script::Operand,
    xref::{ComponentMap, ScriptMap, Xref},
};
use std::{fmt::Write as _, path::Path};

pub fn export(
    pack_root: &Path,
    output: &Path,
    scripts: &ScriptMap,
    components: &ComponentMap,
    xref: &Xref,
    configs: &ConfigTypes,
) -> Result<()> {
    export_with_summaries(
        pack_root,
        output,
        scripts,
        components,
        xref,
        configs,
        &crate::dataflow::infer_summaries(scripts, configs),
    )
}

pub fn export_with_summaries(
    pack_root: &Path,
    output: &Path,
    scripts: &ScriptMap,
    components: &ComponentMap,
    xref: &Xref,
    configs: &ConfigTypes,
    summaries: &crate::dataflow::Summaries,
) -> Result<()> {
    const SCHEMA_VERSION: u32 = 1;
    let mut edges =
        String::from("relation\tscript\tpc\tinterface\tcomponent\tcallee\tcreator_pc\tcommand\n");
    for touch in xref.script_to_comps.values().flatten() {
        let _ = writeln!(
            edges,
            "component-touch\t{}\t{}\t{}\t{}\t\t\t{}",
            touch.script, touch.pc, touch.target.iface, touch.target.child, touch.command
        );
    }
    for hook in xref.comp_to_scripts.values().flatten() {
        let _ = writeln!(
            edges,
            "cached-hook\t\t\t{}\t{}\t{}\t\t{}",
            hook.iface, hook.child, hook.script, hook.slot
        );
    }
    for hook in xref.hook_sets.values().flatten() {
        let _ = writeln!(
            edges,
            "hook-install\t{}\t{}\t{}\t{}\t{}\t\t{}",
            hook.caller, hook.pc, hook.target.iface, hook.target.child, hook.callee, hook.command
        );
    }
    for creation in xref.creations.values().flatten() {
        let _ = writeln!(
            edges,
            "create-child\t{}\t{}\t{}\t{}\t\t{}\tcc_create",
            creation.script, creation.pc, creation.parent.iface, creation.parent.child, creation.pc
        );
    }
    for touch in xref.dynamic_touches.values().flatten() {
        let _ = writeln!(
            edges,
            "dynamic-touch\t{}\t{}\t{}\t{}\t\t{}\t{}",
            touch.script,
            touch.pc,
            touch
                .parent
                .map_or_else(String::new, |parent| parent.iface.to_string()),
            touch
                .parent
                .map_or_else(String::new, |parent| parent.child.to_string()),
            touch.creator_pc,
            touch.command
        );
    }
    for hook in xref.dynamic_hook_sets.values().flatten() {
        let _ = writeln!(
            edges,
            "dynamic-hook-install\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            hook.caller,
            hook.pc,
            hook.parent
                .map_or_else(String::new, |parent| parent.iface.to_string()),
            hook.parent
                .map_or_else(String::new, |parent| parent.child.to_string()),
            hook.callee,
            hook.creator_pc,
            hook.command
        );
    }
    let mut calls = String::from("script\tpc\tcallee\trostered\n");
    let mut contexts = String::from("script\tinterface\tcomponent\tcondition\n");
    for (script, owner) in &xref.entry_contexts {
        let _ = writeln!(
            contexts,
            "{script}\t{}\t{}\tcached-hook-entry; runtime entries may differ",
            owner.iface, owner.child
        );
    }
    let mut variables = String::from("script\tpc\toperation\tdomain\tid\tsecondary\n");
    for (id, script) in scripts {
        for (pc, instruction) in script.code.iter().enumerate() {
            match &instruction.operand {
                Operand::Script(target) | Operand::Int(target)
                    if instruction.command == "gosub_with_params" =>
                {
                    let _ = writeln!(
                        calls,
                        "{id}\t{pc}\t{target}\t{}",
                        scripts.contains_key(target)
                    );
                }
                Operand::VarRef(var) => {
                    let _ = writeln!(
                        variables,
                        "{id}\t{pc}\t{}\t{:?}\t{}\t{}",
                        instruction.command, var.domain, var.id, var.transmog
                    );
                }
                Operand::VarBitRef(var) => {
                    let _ = writeln!(
                        variables,
                        "{id}\t{pc}\t{}\tvarbit\t{}\t{}",
                        instruction.command, var.id, var.transmog
                    );
                }
                _ => {}
            }
        }
    }
    let mut unresolved = String::from("script\tpc\tcommand\treason\n");
    for site in &xref.unresolved {
        let _ = writeln!(
            unresolved,
            "{}\t{}\t{}\t{}",
            site.script, site.pc, site.command, site.reason
        );
    }
    let mut dangling = String::from("script\tpc\tcommand\tvalue\tkind\n");
    for site in &xref.dangling {
        let _ = writeln!(
            dangling,
            "{}\t{}\t{}\t{}\tcomponent",
            site.script, site.pc, site.command, site.value
        );
    }
    const CLEARED_HOOK: i32 = -1;
    for site in &xref.hook_dangling {
        let _ = writeln!(
            dangling,
            "{}\t{}\t{}\t{}\t{}",
            site.script,
            site.pc,
            site.command,
            site.value,
            if site.value == CLEARED_HOOK {
                "hook-clear"
            } else {
                "missing-hook-script"
            }
        );
    }
    let coverage = crate::coverage::analyze_with_summaries(scripts, configs, summaries);
    let mut resources = String::from("script\toperation\tresource\tid\tdomain\tsecondary\n");
    for (script, footprint) in crate::resources::infer(scripts) {
        for (operation, arrays) in [
            ("read", &footprint.array_reads),
            ("write", &footprint.array_writes),
        ] {
            for id in arrays {
                let _ = writeln!(resources, "{script}\t{operation}\tarray\t{id}\t\t");
            }
        }
        for (operation, variables) in [
            ("read", &footprint.variable_reads),
            ("write", &footprint.variable_writes),
        ] {
            for (domain, id, secondary) in variables {
                let domain = domain.map_or_else(|| "varbit".into(), |id| id.to_string());
                let _ = writeln!(
                    resources,
                    "{script}\t{operation}\tvariable\t{id}\t{domain}\t{secondary}"
                );
            }
        }
        for (kind, commands) in [
            ("opaque-host", &footprint.host_commands),
            ("terminal", &footprint.terminal_commands),
        ] {
            for command in commands {
                let _ = writeln!(resources, "{script}\t{kind}\t{command}\t\t\t");
            }
        }
        for secondary in &footprint.component_contexts {
            let _ = writeln!(resources, "{script}\tcontext\tcomponent\t\t\t{secondary}");
        }
        for id in &footprint.missing_callees {
            let _ = writeln!(resources, "{script}\tmissing-callee\tscript\t{id}\t\t");
        }
        if footprint.invalid_flow {
            let _ = writeln!(resources, "{script}\tinvalid-flow\tcontrol\t\t\t");
        }
    }
    let mut signatures = String::from(
        "script\targs_int\targs_object\targs_long\tlocals_int\tlocals_object\tlocals_long\treturns_int\treturns_object\treturns_long\tstatus\n",
    );
    for (id, script) in scripts {
        let returns = match summaries.0[id] {
            crate::returns::ReturnArity::Known { int, obj, long } => {
                [int.to_string(), obj.to_string(), long.to_string()]
            }
            crate::returns::ReturnArity::Unknown | crate::returns::ReturnArity::Never => {
                std::array::from_fn(|_| String::new())
            }
        };
        let _ = writeln!(
            signatures,
            "{id}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            script.args.int,
            script.args.obj,
            script.args.long,
            script.locals.int,
            script.locals.obj,
            script.locals.long,
            returns[0],
            returns[1],
            returns[2],
            if coverage.known.contains(id) {
                "fixed-return-shape"
            } else if coverage.never.contains(id) {
                "non-returning"
            } else {
                "context-required"
            }
        );
    }
    let mut hierarchy =
        String::from("interface\tcomponent\tparent\ttype\thidden\tx\ty\twidth\theight\n");
    for ((group, file), component) in components {
        let _ = writeln!(
            hierarchy,
            "{group}\t{file}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            component.layer,
            component.type_id,
            component.hide,
            component.x,
            component.y,
            component.width,
            component.height
        );
    }
    let mut inputs = format!(
        "schema-version {SCHEMA_VERSION}\nedge-evidence static; retain entry-contexts.tsv conditions\nfootprint-evidence conservative; opaque hosts are not purity certificates\n"
    );
    for name in [
        "scripts",
        "interfaces",
        "config",
        "enum.config",
        "dbtableindex",
    ] {
        let path = pack_root.join(format!("client.{name}.js5"));
        if path.is_file() {
            let _ = writeln!(
                inputs,
                "{name}-sha256 {}",
                crate::project::sha256(&std::fs::read(path)?)
            );
        }
    }
    let _ = writeln!(
        inputs,
        "analysis-sha256 {}",
        crate::project::sha256(
            &[
                include_bytes!("xref.rs").as_slice(),
                include_bytes!("dataflow.rs").as_slice(),
                include_bytes!("jstr.rs").as_slice(),
                include_bytes!("packet.rs").as_slice(),
                include_bytes!("script.rs").as_slice(),
                include_bytes!("semantics.rs").as_slice(),
                include_bytes!("cs2_stack_contracts.rs").as_slice(),
                include_bytes!("resources.rs").as_slice(),
                include_bytes!("returns.rs").as_slice(),
                include_bytes!("evidence.rs").as_slice(),
                include_bytes!("coverage.rs").as_slice(),
                include_bytes!("config.rs").as_slice(),
                include_bytes!("dbtable.rs").as_slice(),
                include_bytes!("opcode.rs").as_slice(),
                include_bytes!("../data/opcodes-910.txt").as_slice(),
                include_bytes!("../data/opcodes-large-910.txt").as_slice(),
                include_bytes!("../data/opcode-aliases-910.txt").as_slice(),
            ]
            .concat()
        )
    );
    std::fs::create_dir_all(output)?;
    for (name, text) in [
        ("edges.tsv", edges),
        ("calls.tsv", calls),
        ("entry-contexts.tsv", contexts),
        ("variables.tsv", variables),
        ("unresolved.tsv", unresolved),
        ("dangling.tsv", dangling),
        ("scripts.tsv", signatures),
        ("components.tsv", hierarchy),
        ("semantic-blockers.tsv", coverage.tsv()),
        ("resources.tsv", resources),
        ("inputs.txt", inputs),
    ] {
        std::fs::write(output.join(name), text)?;
    }
    Ok(())
}
