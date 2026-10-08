use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use cs2::{corpus, profile::Book, wire};
use std::path::PathBuf;

const DEFAULT_SCRIPT_FILE: u32 = 0;
const DEFAULT_CONTEXT_BEFORE: usize = 4;
const DEFAULT_CONTEXT_AFTER: usize = 3;

#[derive(Parser)]
#[command(about = "Inspect CS2 donors using exact client and cache profiles")]
struct Cli {
    #[arg(long)]
    profile: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch a profile-matching cache with checksum-verified resumable groups.
    Fetch {
        /// Omit to select the newest complete cache for this exact build.
        #[arg(long)]
        cache_id: Option<u64>,
        /// Additional data archives; scripts are always included.
        #[arg(long = "archive")]
        archives: Vec<u32>,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = cs2::archive::DEFAULT_WORKERS)]
        workers: usize,
    },
    /// Decode and byte-round-trip every indexed script in an OpenRS2 export.
    Scan {
        #[arg(long)]
        cache_root: PathBuf,
        #[arg(long)]
        build: String,
        /// MD5 reported by the Ghidra program used to recover the profile.
        #[arg(long)]
        client_md5: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Find every opcode use with source identity and instruction context.
    Uses {
        #[arg(long)]
        cache_root: PathBuf,
        /// Exact-profile wire opcode; may be repeated, including unresolved rows.
        #[arg(long = "opcode", required_unless_present = "commands")]
        opcodes: Vec<u16>,
        /// Recovered registration command in the profile; may be repeated.
        #[arg(long = "command", required_unless_present = "opcodes")]
        commands: Vec<String>,
        #[arg(long, default_value_t = DEFAULT_CONTEXT_BEFORE)]
        before: usize,
        #[arg(long, default_value_t = DEFAULT_CONTEXT_AFTER)]
        after: usize,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect typed database schemas, defaults and tuple field selection.
    Database {
        #[arg(long)]
        cache_root: PathBuf,
        /// Exact-client database format and script type catalog.
        #[arg(long)]
        schema: PathBuf,
        #[arg(long, requires = "field", allow_hyphen_values = true)]
        row: Option<i32>,
        #[arg(long, requires = "row", allow_hyphen_values = true)]
        field: Option<i32>,
        #[arg(long, requires = "field", default_value_t = i32::default(), allow_hyphen_values = true)]
        index: i32,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect enum palettes, declared types, defaults and effective lookup.
    Enums {
        #[arg(long)]
        cache_root: PathBuf,
        #[arg(long)]
        schema: PathBuf,
        #[arg(long, allow_hyphen_values = true)]
        enum_id: Option<i32>,
        #[arg(long, requires_all = ["enum_id", "output_type", "key"], allow_hyphen_values = true)]
        input_type: Option<i32>,
        #[arg(long, requires = "input_type", allow_hyphen_values = true)]
        output_type: Option<i32>,
        #[arg(long, requires = "input_type", allow_hyphen_values = true)]
        key: Option<i32>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect source variable definitions, defaults, ranges and script uses.
    Variables {
        #[arg(long)]
        cache_root: PathBuf,
        #[arg(long)]
        schema: PathBuf,
        /// Limit definitions to those used by this script's proven closure.
        #[arg(long)]
        script: Option<u32>,
        #[arg(long)]
        varbit_id: Option<u32>,
        /// Apply the recorded getter to this supplied integer base value.
        #[arg(long, requires = "varbit_id", allow_hyphen_values = true)]
        base_value: Option<i32>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect recorded donor text defaults and cache-backed initial font contracts.
    Frames {
        #[arg(long)]
        cache_root: PathBuf,
        #[arg(long)]
        schema: PathBuf,
        /// Source interface:file; omit for the complete text-frame report.
        #[arg(long)]
        frame: Option<cs2::frames::FrameRef>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Decode one script to JSON, preserving unknown commands and wire operands.
    Dump(ScriptInput),
    /// Resolve named operations and absolute control flow, retaining wire facts.
    Normalize(ScriptInput),
    /// Inspect named calls, typed callback installations and remaining dependency gaps.
    Closure {
        /// Exact donor schema and type catalog for database-backed scripts.
        #[arg(long)]
        database_schema: Option<PathBuf>,
        #[arg(long)]
        cache_root: PathBuf,
        #[arg(long)]
        script: u32,
    },
    /// Prepare a pinned plan; scalar inputs remain unbound until edited.
    Prepare {
        /// Exact donor text-prefix format and initial-font definitions.
        #[arg(long)]
        frame_schema: Option<PathBuf>,
        /// Exact donor schema and type catalog for database-backed scripts.
        #[arg(long)]
        database_schema: Option<PathBuf>,
        /// Exact donor enum definitions for palette and enum queries.
        #[arg(long)]
        enum_schema: Option<PathBuf>,
        /// Exact source variable declarations used by modern bit reads.
        #[arg(long, requires = "variable_symbols")]
        variable_schema: Option<PathBuf>,
        /// Target revision symbol registry directory (varp.sym and varc.sym).
        #[arg(long, requires = "variable_schema")]
        variable_symbols: Option<PathBuf>,
        #[arg(long)]
        cache_root: PathBuf,
        #[arg(long)]
        script: u32,
        #[arg(long)]
        name: String,
        #[arg(long)]
        base_pack: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Lower a reviewed closure with explicit target-frame argument bindings.
    Import {
        /// Exact donor text-prefix format and initial-font definitions.
        #[arg(long)]
        frame_schema: Option<PathBuf>,
        /// Exact donor schema and type catalog for database-backed scripts.
        #[arg(long)]
        database_schema: Option<PathBuf>,
        /// Exact donor enum definitions for palette and enum queries.
        #[arg(long)]
        enum_schema: Option<PathBuf>,
        /// Exact source variable declarations used by modern bit reads.
        #[arg(long, requires = "variable_symbols")]
        variable_schema: Option<PathBuf>,
        /// Target revision symbol registry directory (varp.sym and varc.sym).
        #[arg(long, requires = "variable_schema")]
        variable_symbols: Option<PathBuf>,
        #[arg(long)]
        cache_root: PathBuf,
        #[arg(long)]
        adapter: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        base_pack: PathBuf,
        #[arg(long)]
        inames: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(clap::Args)]
struct ScriptInput {
    #[arg(long, required_unless_present = "script", conflicts_with = "script")]
    input: Option<PathBuf>,
    #[arg(long, requires = "cache_root")]
    script: Option<u32>,
    #[arg(long, requires = "script")]
    cache_root: Option<PathBuf>,
    #[arg(long, default_value_t = DEFAULT_SCRIPT_FILE)]
    file: u32,
}

fn read_database(
    root: &std::path::Path,
    book: &Book,
    schema: Option<&PathBuf>,
) -> Result<Option<cs2::database::Definitions>> {
    schema
        .map(|path| cs2::database::Definitions::load(root, book, &std::fs::read(path)?))
        .transpose()
}

fn read_enums(
    root: &std::path::Path,
    book: &Book,
    schema: Option<&PathBuf>,
) -> Result<Option<cs2::enums::Definitions>> {
    schema
        .map(|path| cs2::enums::Definitions::load(root, book, &std::fs::read(path)?))
        .transpose()
}

fn read_input(input: ScriptInput, book: &Book) -> Result<Vec<u8>> {
    match (input.input, input.script, input.cache_root) {
        (Some(path), None, None) => Ok(std::fs::read(path)?),
        (None, Some(id), Some(root)) => {
            let (_, index) = corpus::load_index(&root, book)?;
            corpus::load_group(&root, book.profile.script_archive, &index, id)?
                .remove(&input.file)
                .with_context(|| format!("script group {id} has no file {}", input.file))
        }
        _ => anyhow::bail!("input bytes or a script ID and cache root are required"),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let book = Book::parse(
        &std::fs::read(&cli.profile).with_context(|| cli.profile.display().to_string())?,
    )?;
    match cli.command {
        Command::Fetch {
            cache_id,
            archives,
            output,
            workers,
        } => {
            let report = cs2::archive::fetch(
                &book,
                cs2::archive::FetchOptions {
                    cache_id,
                    archives: &archives,
                    output: &output,
                    workers,
                },
            )?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        }
        Command::Scan {
            cache_root,
            build,
            client_md5,
            output,
        } => {
            ensure!(
                !output.exists(),
                "output already exists: {}",
                output.display()
            );
            let report = corpus::scan(&cache_root, &book, &build, &client_md5)?;
            let bytes = serde_json::to_vec_pretty(&report)?;
            publish(&output, &bytes)?;
            eprintln!(
                "{}: {} scripts, {} instructions, {} used opcodes; {} instructions have unresolved semantics",
                report.build,
                report.scripts.len(),
                report.instruction_count,
                report.opcodes.len(),
                report.unresolved_instruction_count
            );
        }
        Command::Uses {
            cache_root,
            opcodes,
            commands,
            before,
            after,
            output,
        } => {
            ensure!(
                !output.exists(),
                "output already exists: {}",
                output.display()
            );
            let report = corpus::uses(&cache_root, &book, &opcodes, &commands, before, after)?;
            publish(&output, &serde_json::to_vec_pretty(&report)?)?;
            eprintln!(
                "{}: {} uses in {} scripts; all {} indexed scripts verified; {}",
                report.build,
                report.occurrences,
                report.scripts.len(),
                report.scanned_scripts,
                output.display()
            );
        }
        Command::Enums {
            cache_root,
            schema,
            enum_id,
            input_type,
            output_type,
            key,
            output,
        } => {
            ensure!(
                !output.exists(),
                "output already exists: {}",
                output.display()
            );
            let definitions =
                cs2::enums::Definitions::load(&cache_root, &book, &std::fs::read(schema)?)?;
            let query =
                input_type
                    .zip(output_type)
                    .zip(key)
                    .map(|((input_type, output_type), key)| cs2::enums::Query {
                        input_type,
                        output_type,
                        key,
                    });
            publish(
                &output,
                &serde_json::to_vec_pretty(&definitions.report(enum_id, query)?)?,
            )?;
            eprintln!(
                "{}: {} enums verified; {}",
                book.profile.build,
                definitions.library.definitions.len(),
                output.display()
            );
        }
        Command::Variables {
            cache_root,
            schema,
            script,
            varbit_id,
            base_value,
            output,
        } => {
            ensure!(
                !output.exists(),
                "output already exists: {}",
                output.display()
            );
            let definitions =
                cs2::variables::Definitions::load(&cache_root, &book, &std::fs::read(schema)?)?;
            let scripts = script
                .map(|id| cs2::import910::load_closure(&cache_root, &book, std::iter::once(id)))
                .transpose()?;
            let report = definitions.report(varbit_id, base_value, scripts.as_ref(), &book)?;
            publish(&output, &serde_json::to_vec_pretty(&report)?)?;
            eprintln!(
                "{}: {} source varbits, {} variables; {} decode gaps; {}",
                book.profile.build,
                report["varbit_count"],
                report["variable_count"],
                report["decode_failures"],
                output.display()
            );
        }
        Command::Database {
            cache_root,
            schema,
            row,
            field,
            index,
            output,
        } => {
            ensure!(
                !output.exists(),
                "output already exists: {}",
                output.display()
            );
            let definitions =
                cs2::database::Definitions::load(&cache_root, &book, &std::fs::read(schema)?)?;
            let query =
                row.zip(field)
                    .map(|(row, field)| cs2::database::Query { row, field, index });
            publish(
                &output,
                &serde_json::to_vec_pretty(&definitions.report(query)?)?,
            )?;
            eprintln!(
                "{}: {} tables and {} rows verified; {}",
                book.profile.build,
                definitions.database().tables.len(),
                definitions.database().rows.len(),
                output.display()
            );
        }
        Command::Closure {
            cache_root,
            script,
            database_schema,
        } => {
            let database = read_database(&cache_root, &book, database_schema.as_ref())?;
            let scripts = cs2::import910::load_closure_with_database(
                &cache_root,
                &book,
                std::iter::once(script),
                database.as_ref(),
            )?;
            let inspection =
                cs2::import910::inspect_closure_with_database(&scripts, &book, database.as_ref())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &inspection)?;
        }
        Command::Prepare {
            frame_schema,
            database_schema,
            enum_schema,
            variable_schema,
            variable_symbols,
            cache_root,
            script,
            name,
            base_pack,
            output,
        } => {
            let database = read_database(&cache_root, &book, database_schema.as_ref())?;
            let enums = read_enums(&cache_root, &book, enum_schema.as_ref())?;
            let frames = frame_schema
                .as_ref()
                .map(|path| {
                    cs2::frames::Definitions::load(&cache_root, &book, &std::fs::read(path)?)
                })
                .transpose()?;
            let variables = variable_schema
                .as_ref()
                .map(|path| {
                    cs2::variables::Definitions::load(&cache_root, &book, &std::fs::read(path)?)
                })
                .transpose()?;
            let variable_symbols = variable_symbols
                .as_ref()
                .map(|path| cs2::variable_bindings::Symbols::load(path))
                .transpose()?;
            let scripts = cs2::import910::load_closure_with_database(
                &cache_root,
                &book,
                std::iter::once(script),
                database.as_ref(),
            )?;
            let mut plan =
                cs2::import910::Plan::from_root(&scripts, &book, &base_pack, script, &name)?;
            plan.database = database
                .as_ref()
                .map(cs2::database::Definitions::identity)
                .transpose()?;
            plan.enums = enums
                .as_ref()
                .map(cs2::enums::Definitions::identity)
                .transpose()?;
            plan.frames = frames.as_ref().map(cs2::frames::Definitions::identity);
            plan.variables = variables
                .as_ref()
                .zip(variable_symbols.as_ref())
                .map(|(definitions, symbols)| {
                    cs2::variable_bindings::Plan::prepare_with_database(
                        cs2::variable_bindings::Import {
                            definitions,
                            symbols,
                        },
                        &scripts,
                        &book,
                        &base_pack,
                        database.as_ref(),
                    )
                })
                .transpose()?;
            publish(&output, &serde_json::to_vec_pretty(&plan)?)?;
        }
        Command::Import {
            frame_schema,
            database_schema,
            enum_schema,
            variable_schema,
            variable_symbols,
            cache_root,
            adapter,
            plan,
            base_pack,
            inames,
            output,
        } => {
            let plan: cs2::import910::Plan = serde_json::from_slice(&std::fs::read(plan)?)?;
            let database = read_database(&cache_root, &book, database_schema.as_ref())?;
            let enums = read_enums(&cache_root, &book, enum_schema.as_ref())?;
            let frames = frame_schema
                .as_ref()
                .map(|path| {
                    cs2::frames::Definitions::load(&cache_root, &book, &std::fs::read(path)?)
                })
                .transpose()?;
            let variables = variable_schema
                .as_ref()
                .map(|path| {
                    cs2::variables::Definitions::load(&cache_root, &book, &std::fs::read(path)?)
                })
                .transpose()?;
            let variable_symbols = variable_symbols
                .as_ref()
                .map(|path| cs2::variable_bindings::Symbols::load(path))
                .transpose()?;
            let scripts = cs2::import910::load_closure_with_database(
                &cache_root,
                &book,
                plan.entries.iter().map(|entry| entry.procedure),
                database.as_ref(),
            )?;
            let adapter_bytes = std::fs::read(adapter)?;
            let inames_text = std::fs::read_to_string(inames)?;
            let report = cs2::import910::build(
                cs2::import910::BuildInput {
                    frames: frames.as_ref(),
                    database: database.as_ref(),
                    enums: enums.as_ref(),
                    variables: variables.as_ref().zip(variable_symbols.as_ref()).map(
                        |(definitions, symbols)| cs2::variable_bindings::Import {
                            definitions,
                            symbols,
                        },
                    ),
                    scripts: &scripts,
                    book: &book,
                    adapter_bytes: &adapter_bytes,
                    plan: &plan,
                    pack_root: &base_pack,
                    inames_text: &inames_text,
                },
                &output,
            )?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        }
        Command::Frames {
            cache_root,
            schema,
            frame,
            output,
        } => {
            let frames =
                cs2::frames::Definitions::load(&cache_root, &book, &std::fs::read(schema)?)?;
            publish(&output, &serde_json::to_vec_pretty(&frames.report(frame)?)?)?;
        }
        Command::Normalize(input) => {
            let bytes = read_input(input, &book)?;
            serde_json::to_writer_pretty(
                std::io::stdout().lock(),
                &cs2::semantic::normalize(&bytes, &book)?,
            )?;
        }
        Command::Dump(input) => {
            let bytes = read_input(input, &book)?;
            let script = wire::decode(&bytes, &book)?;
            ensure!(
                wire::encode(&script, &book)? == bytes,
                "script failed byte round trip"
            );
            let ids: std::collections::BTreeSet<_> =
                script.code.iter().map(|row| row.opcode).collect();
            let opcodes: Vec<_> = ids
                .into_iter()
                .map(|id| book.opcode(id))
                .collect::<Result<_>>()?;
            let output = serde_json::json!({
                "build": book.profile.build, "client_md5": book.profile.client_md5,
                "profile_sha256": book.sha256, "script_sha256": cs2::profile::digest(&bytes),
                "script": script, "opcodes": opcodes,
            });
            serde_json::to_writer_pretty(std::io::stdout().lock(), &output)?;
        }
    }
    Ok(())
}

fn publish(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let temporary = parent.join(format!(".cs2-report-{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::hard_link(&temporary, path)?;
        Ok(())
    })();
    drop(file);
    std::fs::remove_file(&temporary)?;
    result
}
