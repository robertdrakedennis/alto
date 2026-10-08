//! CLI surface. Verbs are 910-only: `dump` / `assemble` (scripts),
//! `dump-interfaces` / `assemble-interface` (components), `validate`
//! (byte-exactness over the runtime packs), `inspect` (the script ⇄
//! component cross-reference index and evidence export), script `repack`, and
//! linked script/interface project `build`.
//!
//! Every assembling verb verifies by default (re-decode must equal the
//! lowered model); bypasses do not exist.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

/// Native 910 cache toolkit.
#[derive(Parser, Debug)]
#[command(
    name = "native910",
    about = "Native 910 cache toolkit: dump, edit, assemble, inspect, repack"
)]
pub struct Cli {
    /// Runtime pack root holding the `client.*.js5` files. Read-only, except
    /// `repack` writes the rebuilt pack beside it — never over it.
    #[arg(long, default_value = "server/data/pack")]
    pub pack_root: PathBuf,
    #[command(subcommand)]
    pub command: Command,
}

/// The verbs. Flags per verb land with the milestone that implements it.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Dump 910 scripts to native source text (one `.rs2` per script, plus
    /// the generated `symbols.txt` that `assemble --symbols` reads back).
    Dump {
        /// Dump only this script, retaining the corpus symbol registry.
        #[arg(long)]
        script: Option<i32>,
        /// Output directory for the `.rs2` files (`<group>_<file>.rs2`).
        #[arg(long)]
        out_dir: PathBuf,
        /// Curated `names.txt` merged into the dump's call names.
        #[arg(long)]
        names: Option<PathBuf>,
        /// Curated `inames.txt` rendering packed ids as `Bank/7` (omit for
        /// numeric-only).
        #[arg(long)]
        inames: Option<PathBuf>,
    },
    /// Assemble one native source file back to verified 910 binary.
    Assemble {
        /// Input `.rs2` source file.
        #[arg(long)]
        input: PathBuf,
        /// Output binary path.
        #[arg(long)]
        output: PathBuf,
        /// Generated `symbols.txt` resolving `~calls` (omit for numeric-only).
        #[arg(long)]
        symbols: Option<PathBuf>,
        /// Curated `inames.txt` resolving `Bank/7` refs (omit for
        /// numeric-only; symbolic refs without it fail loudly).
        #[arg(long)]
        inames: Option<PathBuf>,
    },
    /// Dump 910 interface components to native source text (one `.ifc` per
    /// component, plus the generated `symbols.txt` shared with script dumps).
    DumpInterfaces {
        /// Dump only this interface (numeric ID or a curated name).
        #[arg(long)]
        interface: Option<String>,
        /// Output directory for the `.ifc` files (`<group>_<file>.ifc`).
        #[arg(long)]
        out_dir: PathBuf,
        /// Curated `names.txt` merged into hook `~calls`.
        #[arg(long)]
        names: Option<PathBuf>,
        /// Curated `inames.txt` for interface/child names in identity comments.
        #[arg(long)]
        inames: Option<PathBuf>,
    },
    /// Assemble one native interface source file back to verified binary.
    AssembleInterface {
        /// Input `.ifc` source file.
        #[arg(long)]
        input: PathBuf,
        /// Output binary path.
        #[arg(long)]
        output: PathBuf,
        /// Interface group id (sets the parentlayer the layer field resolves
        /// against).
        #[arg(long)]
        group: u32,
        /// Generated `symbols.txt` resolving hook `~calls` (omit for numeric).
        #[arg(long)]
        symbols: Option<PathBuf>,
        /// Curated `inames.txt` validating the identity comment (omit to
        /// ignore it).
        #[arg(long)]
        inames: Option<PathBuf>,
    },
    /// Validate native source or 910 binary against the 910 client model.
    Validate,
    /// Inspect the script ⇄ component cross-reference index (read-only:
    /// which scripts touch which components, and which hooks install which
    /// scripts).
    Inspect {
        /// Export pinned TSV evidence, including unresolved sites and call/variable edges.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Show one interface's child tree instead of the summary.
        #[arg(long)]
        interface: Option<String>,
        /// Show one script's component touches instead of the summary.
        #[arg(long)]
        script: Option<i32>,
        /// Reverse asset lookup (`kind:id`, e.g. `sprite:123`) instead of
        /// the summary.
        #[arg(long)]
        asset: Option<String>,
        /// Curated `names.txt` for `~name` display.
        #[arg(long)]
        names: Option<PathBuf>,
        /// Curated `inames.txt` for interface/child names.
        #[arg(long)]
        inames: Option<PathBuf>,
    },
    /// Analyze a script entry with proven typed arguments; never executes host commands.
    Analyze {
        #[arg(long)]
        script: i32,
        /// Repeated typed values: i:<int>, l:<long>, s:<text>, n: (null), or i:/l:/s:? (unknown).
        #[arg(long, allow_hyphen_values = true)]
        arg: Vec<String>,
    },
    /// Repack assembled groups into a loadable 910 pack.
    Repack {
        /// Directory of numeric <script-id>.bin replacements/additions.
        #[arg(long)]
        input_dir: PathBuf,
        /// New pack file; existing files are rejected.
        #[arg(long)]
        output: PathBuf,
    },
    /// Build a pinned script/interface project into a new immutable directory.
    Build {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Show encoding and established semantic coverage (TSV).
    Registry,
    /// Trace a script one instruction at a time using the deterministic host.
    Trace {
        #[arg(long)]
        input: PathBuf,
        /// Root cache identity, required when an imported binary has a nonnumeric filename.
        #[arg(long)]
        script: Option<i32>,
        /// Stop before this instruction index in the root script.
        #[arg(long)]
        break_at: Option<usize>,
    },
}

/// Read an optional curated `names.txt` (empty registry when omitted).
fn load_curated(names: Option<&PathBuf>) -> Result<Vec<(i32, String)>> {
    match names {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
            crate::symbols::SymbolRegistry::parse_names_txt(&text)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))
        }
        None => Ok(Vec::new()),
    }
}

/// Read an optional curated `inames.txt` and build it over the pack roster
/// (empty registry when omitted). Fails loudly on bad lines, bad names, and
/// unknown ids — like script names, a typo is never silently ignored.
fn load_inames(
    pack_root: &std::path::Path,
    inames: Option<&PathBuf>,
) -> Result<crate::inames::InterfaceRegistry> {
    match inames {
        None => Ok(crate::inames::InterfaceRegistry::empty()),
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
            let (ifaces, children) = crate::inames::parse_inames_txt(&text)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
            let roster = crate::inames::load_interface_roster(pack_root)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            crate::inames::InterfaceRegistry::build(&ifaces, &children, &roster)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))
        }
    }
}
/// Read an optional generated `symbols.txt` (empty registry when omitted).
fn load_registry(symbols: Option<&PathBuf>) -> Result<crate::symbols::SymbolRegistry> {
    match symbols {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
            crate::symbols::SymbolRegistry::parse_symbols_txt(&text)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))
        }
        None => Ok(crate::symbols::SymbolRegistry::empty()),
    }
}

/// Write assembled bytes, creating parent directories as needed.
fn write_output(output: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| anyhow::anyhow!("{}: {error}", parent.display()))?;
    }
    std::fs::write(output, bytes)
        .map_err(|error| anyhow::anyhow!("{}: {error}", output.display()))?;
    Ok(())
}

/// Dispatch the CLI. Each arm bails until its milestone lands.
pub fn run(cli: &Cli) -> Result<()> {
    match cli.command {
        Command::Dump {
            script,
            ref out_dir,
            ref names,
            ref inames,
        } => {
            check_pack_root(&cli.pack_root)?;
            let curated = load_curated(names.as_ref())?;
            let inames = load_inames(&cli.pack_root, inames.as_ref())?;
            let report = crate::source::dump_selected_scripts_pack(
                &cli.pack_root,
                out_dir,
                &curated,
                &inames,
                script,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
            println!(
                "dump: {} script(s) to {} ({} named call(s))",
                report.files,
                out_dir.display(),
                report.named_calls
            );
            Ok(())
        }
        Command::Assemble {
            ref input,
            ref output,
            ref symbols,
            ref inames,
        } => {
            let text = std::fs::read_to_string(input)
                .map_err(|error| anyhow::anyhow!("{}: {error}", input.display()))?;
            let mut registry = load_registry(symbols.as_ref())?;
            let inames = load_inames(&cli.pack_root, inames.as_ref())?;
            // Config tables come from the pack root when present; when absent
            // (numeric-only source assembled outside a repo checkout) the
            // empty resolver still assembles every non-config line and fails
            // loudly on any config-typed read — never guessed.
            let configs = if cli.pack_root.is_dir() {
                crate::config::ConfigTypes::load(&cli.pack_root)
                    .map_err(|error| anyhow::anyhow!("{error}"))?
            } else {
                crate::config::ConfigTypes::empty()
            };
            if !registry.is_empty() && cli.pack_root.join("client.scripts.js5").is_file() {
                let scripts = crate::xref::load_scripts(&cli.pack_root)?;
                let summaries = crate::dataflow::infer_summaries(&scripts, &configs);
                registry = registry.with_call_context(&scripts, &configs, &summaries);
            }
            let bytes = crate::source::assemble_source(
                &text,
                &crate::opcode::OpcodeBook::embedded()?,
                &registry,
                &configs,
                &inames,
            )
            .map_err(|error| anyhow::anyhow!("{}: {error}", input.display()))?;
            write_output(output, &bytes)?;
            println!(
                "assemble: {} → {} ({} bytes, verified)",
                input.display(),
                output.display(),
                bytes.len()
            );
            Ok(())
        }
        Command::DumpInterfaces {
            ref interface,
            ref out_dir,
            ref names,
            ref inames,
        } => {
            check_pack_root(&cli.pack_root)?;
            let curated = load_curated(names.as_ref())?;
            let inames = load_inames(&cli.pack_root, inames.as_ref())?;
            let selected = interface
                .as_deref()
                .map(|name| {
                    inames
                        .resolve_iface(name)
                        .and_then(|id| u32::try_from(id).ok())
                        .ok_or_else(|| anyhow::anyhow!("unknown interface '{name}'"))
                })
                .transpose()?;
            let report = crate::isource::dump_selected_interfaces_pack(
                &cli.pack_root,
                out_dir,
                &curated,
                &inames,
                selected,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
            println!(
                "dump-interfaces: {} component(s) to {} ({} named hook(s))",
                report.files,
                out_dir.display(),
                report.named_hooks
            );
            Ok(())
        }
        Command::AssembleInterface {
            ref input,
            ref output,
            group,
            ref symbols,
            ref inames,
        } => {
            let text = std::fs::read_to_string(input)
                .map_err(|error| anyhow::anyhow!("{}: {error}", input.display()))?;
            let registry = load_registry(symbols.as_ref())?;
            let inames = load_inames(&cli.pack_root, inames.as_ref())?;
            let parentlayer = (group << 16) as i32;
            let bytes = crate::isource::assemble_component(&text, parentlayer, &registry, &inames)
                .map_err(|error| anyhow::anyhow!("{}: {error}", input.display()))?;
            write_output(output, &bytes)?;
            println!(
                "assemble-interface: {} → {} ({} bytes, verified)",
                input.display(),
                output.display(),
                bytes.len()
            );
            Ok(())
        }
        Command::Analyze {
            script: id,
            ref arg,
        } => {
            check_pack_root(&cli.pack_root)?;
            let scripts = crate::xref::load_scripts(&cli.pack_root)?;
            let script = scripts
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("missing script {id}"))?;
            let configs = crate::config::ConfigTypes::load(&cli.pack_root)?;
            let dependencies = crate::dataflow::dependency_closure(&scripts, std::iter::once(id));
            let (summaries, values) = crate::dataflow::infer_summaries(&dependencies, &configs);
            let mut arguments = crate::dataflow::EntryArguments::default();
            for value in arg {
                let (kind, text) = value
                    .split_once(':')
                    .ok_or_else(|| anyhow::anyhow!("argument expects kind:value, got {value:?}"))?;
                use crate::dataflow::Constant;
                let (lane, constant) = match (kind, text) {
                    ("i", "?") => (0, None),
                    ("l", "?") => (2, None),
                    ("s", "?") => (1, None),
                    ("i", text) => (0, Some(Constant::Int(text.parse()?))),
                    ("l", text) => (2, Some(Constant::Long(text.parse()?))),
                    ("s", text) => (1, Some(Constant::String(crate::jstr::from_text(text)))),
                    ("n", "") => (1, Some(Constant::Null)),
                    _ => bail!("invalid argument {value:?}; use i:, l:, s: or n:"),
                };
                arguments[lane].push(constant);
            }
            let analysis = crate::dataflow::analyze_in_context(
                script,
                &dependencies,
                &summaries,
                &values,
                &configs,
                &arguments,
            );
            println!(
                "# script: {id}\n# scope: supplied entry arguments\n# arguments: {arguments:?}"
            );
            println!(
                "# universal-return: {:?}\n# entry-return: {:?}\n# failure: {:?}",
                summaries[&id], analysis.arity, analysis.failure
            );
            println!(
                "# state-evidence: {}",
                if analysis.failure.is_none() {
                    "complete"
                } else {
                    "partial; unprocessed predecessors may change values"
                }
            );
            println!("pc\tcommand\tint-stack\tobject-stack\tlong-stack");
            for (pc, instruction) in script.code.iter().enumerate() {
                if let Some(state) = &analysis.before[pc] {
                    let constants: [Vec<_>; 3] = std::array::from_fn(|lane| {
                        state.stacks[lane].iter().map(|v| &v.constant).collect()
                    });
                    println!(
                        "{pc}\t{}\t{:?}\t{:?}\t{:?}",
                        instruction.command, constants[0], constants[1], constants[2]
                    );
                } else {
                    println!(
                        "{pc}\t{}\tunvisited\tunvisited\tunvisited",
                        instruction.command
                    );
                }
            }
            Ok(())
        }
        Command::Inspect {
            ref out_dir,
            ref interface,
            script,
            ref asset,
            ref names,
            ref inames,
        } => {
            check_pack_root(&cli.pack_root)?;
            let curated = load_curated(names.as_ref())?;
            let inames = load_inames(&cli.pack_root, inames.as_ref())?;
            let (scripts, components) = crate::xref::load_corpus(&cli.pack_root)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let configs = crate::config::ConfigTypes::load(&cli.pack_root)?;
            let summaries = crate::dataflow::infer_summaries(&scripts, &configs);
            let registry =
                crate::source::registry_for_decoded_scripts(&scripts, &curated, &summaries.0)?;
            let xref =
                crate::xref::build_xref_with_summaries(&scripts, &components, &configs, &summaries);
            if let Some(output) = out_dir {
                crate::evidence::export_with_summaries(
                    &cli.pack_root,
                    output,
                    &scripts,
                    &components,
                    &xref,
                    &configs,
                    &summaries,
                )?;
            }
            let rosters = crate::assets::load_rosters(&cli.pack_root)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let assets = crate::assets::build_asset_index(&components, &rosters);
            if let Some(spec) = asset {
                let (kind_word, id_text) = spec.split_once(':').ok_or_else(|| {
                    anyhow::anyhow!("--asset expects kind:id (e.g. sprite:123), got '{spec}'")
                })?;
                let kind = crate::assets::AssetKind::parse_word(kind_word).ok_or_else(|| {
                    anyhow::anyhow!("unknown asset kind '{kind_word}' (sprite, model, anim, font)")
                })?;
                let id: i32 = id_text
                    .parse()
                    .map_err(|_| anyhow::anyhow!("bad asset id '{id_text}'"))?;
                print!(
                    "{}",
                    crate::assets::format_asset_users(
                        crate::assets::AssetRef { kind, id },
                        &assets
                    )
                );
                return Ok(());
            }
            match (interface, script) {
                (Some(spec), _) => {
                    let iface = inames
                        .resolve_iface(spec)
                        .ok_or_else(|| anyhow::anyhow!("unknown interface '{spec}'"))?;
                    print!(
                        "{}",
                        crate::xref::format_interface(
                            iface,
                            &components,
                            &xref,
                            &registry,
                            Some(&assets),
                            &inames
                        )
                    );
                }
                (None, Some(id)) => print!("{}", crate::xref::format_script(id, &xref, &registry)),
                (None, None) => {
                    print!("{}", crate::xref::format_summary(&xref, &registry, &inames));
                    print!("{}", crate::assets::format_asset_summary(&assets));
                }
            }
            Ok(())
        }
        Command::Validate => {
            check_pack_root(&cli.pack_root)?;
            let scripts = crate::validate::validate_scripts_pack(&cli.pack_root)?;
            println!(
                "scripts: {}, instructions: {}, failures: {}",
                scripts.scripts,
                scripts.instructions,
                scripts.failures.len()
            );
            for failure in &scripts.failures {
                println!("  FAIL {failure}");
            }
            let interfaces = crate::validate::validate_interfaces_pack(&cli.pack_root)?;
            println!(
                "interfaces: {} groups, {} components, failures: {}",
                interfaces.groups,
                interfaces.components,
                interfaces.failures.len()
            );
            for failure in &interfaces.failures {
                println!("  FAIL {failure}");
            }
            let mut configs = crate::validate::validate_configs_pack(&cli.pack_root)?;
            crate::validate::validate_dbtable_pack(&cli.pack_root, &mut configs)?;
            for (kind, count) in &configs.entries {
                println!("configs {kind}: {count} entries");
            }
            println!("configs failures: {}", configs.failures.len());
            for failure in &configs.failures {
                println!("  FAIL {failure}");
            }
            let sprites = crate::validate::validate_sprites_pack(&cli.pack_root)?;
            println!(
                "sprites: {} sheets, failures: {}",
                sprites.sheets,
                sprites.failures.len()
            );
            for failure in &sprites.failures {
                println!("  FAIL {failure}");
            }
            if scripts.is_clean()
                && interfaces.is_clean()
                && configs.is_clean()
                && sprites.is_clean()
            {
                println!(
                    "validate: BYTE-EXACT ({} scripts, {} components, {} configs, {} sprites)",
                    scripts.scripts,
                    interfaces.components,
                    configs.total(),
                    sprites.sheets
                );
                Ok(())
            } else {
                bail!(
                    "validate: {} script failure(s), {} interface failure(s), {} config failure(s), {} sprite failure(s)",
                    scripts.failures.len(),
                    interfaces.failures.len(),
                    configs.failures.len(),
                    sprites.failures.len()
                );
            }
        }
        Command::Build {
            ref manifest,
            ref output,
        } => {
            let count = crate::project::build(manifest, &cli.pack_root, output)?;
            println!("built {count} sources in {}", output.display());
            Ok(())
        }
        Command::Registry => {
            let book = crate::opcode::OpcodeBook::embedded()?;
            println!("opcode\tname\tretail_dispatch\teffect\tstate");
            for (opcode, _) in book.entries() {
                let row = crate::semantics::contract_for_opcode(
                    &book,
                    opcode,
                    &crate::script::Operand::Byte(0),
                )?;
                println!(
                    "{}\t{}\t{}\t{:?}\t{:?}",
                    row.opcode, row.name, row.retail_dispatch, row.effect, row.state
                );
            }
            Ok(())
        }
        Command::Repack {
            ref input_dir,
            ref output,
        } => {
            use std::collections::BTreeMap;
            use std::io::Write;
            let mut replacements = BTreeMap::new();
            for entry in std::fs::read_dir(input_dir)? {
                let path = entry?.path();
                if path.extension().and_then(|v| v.to_str()) != Some("bin") {
                    continue;
                }
                let id: u32 = path
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| anyhow::anyhow!("invalid script filename"))?
                    .parse()?;
                if replacements.insert(id, std::fs::read(&path)?).is_some() {
                    bail!("duplicate script ID {id}");
                }
            }
            if replacements.is_empty() {
                bail!("no numeric .bin scripts in input directory");
            }
            let base = std::fs::read(cli.pack_root.join("client.scripts.js5"))?;
            let bytes = crate::repack::scripts(&base, &replacements)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)?;
            file.write_all(&bytes)?;
            println!(
                "repacked {} scripts to {}",
                replacements.len(),
                output.display()
            );
            Ok(())
        }
        Command::Trace {
            ref input,
            script: root_script,
            break_at,
        } => {
            let book = crate::opcode::OpcodeBook::embedded()?;
            let script = crate::script::decode_script(&std::fs::read(input)?, &book)?;
            let mut host = crate::runtime::RuntimeHost::default();
            let mut provider = crate::vm::Programs::default();
            let archive =
                crate::pack::PackArchive::open(&cli.pack_root.join("client.scripts.js5"))?;
            for id in archive.group_ids() {
                if let Some(files) = archive.group_files(id)?
                    && let Some(bytes) = files.get(&0)
                {
                    let decoded = crate::script::decode_script(bytes, &book)?;
                    let accounting = crate::execution::decode_accounting(
                        id as i32,
                        bytes,
                        files
                            .get(&crate::execution::METADATA_FILE)
                            .map(Vec::as_slice),
                        &decoded,
                    )?;
                    provider.accounting.insert(id as i32, accounting);
                    provider.scripts.insert(id as i32, decoded);
                }
            }
            let mut vm = crate::vm::Vm::new(&mut host, &provider);
            let inferred_id = input
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<i32>().ok())
                .filter(|id| provider.scripts.get(id) == Some(&script));
            let root_id = root_script.or(inferred_id);
            if let Some(id) = root_id
                && provider.scripts.get(&id) != Some(&script)
            {
                bail!("trace root {id} does not match the input binary in this pack");
            }
            let mut session = if let Some(id) = root_id {
                crate::vm::Session::for_script(id, &script, &[], None)?
            } else {
                crate::vm::Session::new(&script, &[])?
            };
            while !session.finished() {
                let snapshot = session.snapshot();
                println!("{snapshot:?}");
                if snapshot.frames.is_empty() && break_at == Some(snapshot.pc) {
                    println!("breakpoint");
                    return Ok(());
                }
                vm.step(&mut session).map_err(|error| {
                    anyhow::anyhow!(
                        "script {:?} pc {}: {error}",
                        snapshot.script_name,
                        snapshot.pc
                    )
                })?;
            }
            println!("finished {:?}", session.snapshot());
            Ok(())
        }
    }
}

/// `--pack-root` defaults to the repo-relative runtime pack, so running from
/// anywhere but the repo root (or a moved pack) fails here with a pointer,
/// not pages later as an OS error on a joined path.
fn check_pack_root(pack_root: &Path) -> Result<()> {
    if !pack_root.is_dir() {
        bail!(
            "pack root not found: {} (run from the alto repo root or pass --pack-root)",
            pack_root.display()
        );
    }
    Ok(())
}
