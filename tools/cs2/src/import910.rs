//! Bind an explicit donor closure to a pinned target pack and publish a project.
//! Exports supply component arguments; internal procedures retain donor arities.
pub use crate::bridge::BridgeReport;
use crate::{
    bridge, corpus, flow,
    lower910::{Adapter, HostRequirement, Requirement},
    profile::{Book, Build, digest},
    semantic::{self, Argument},
};
use anyhow::{Context, Result, ensure};
use native910::{
    config::ConfigTypes,
    dataflow::{self, EntryContext, Value},
    execution::{Role, Specification, Table as ExecutionTable},
    inames::InterfaceRegistry,
    interface::{self, ComponentBody},
    opcode::OpcodeBook,
    pack::PackArchive,
    script::{CompiledScript, Counts, Instruction, Operand},
    source,
    xref::{ComponentRef, pack_component},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::Arc,
};

const IMPORT_FORMAT: u32 = 1;
const NEXT_ID: u32 = 1;
const SCRIPT_FILE: u32 = 0;
const INTEGER_LANE: usize = 0;
const OBJECT_LANE: usize = 1;
const LONG_LANE: usize = 2;
const MAX_BOUND_CONTEXTS: usize = 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub format: u32,
    pub source_build: Build,
    pub source_client_md5: String,
    pub source_script_index_sha256: String,
    pub source_scripts_sha256: BTreeMap<u32, String>,
    pub base_scripts_sha256: String,
    pub base_interfaces_sha256: String,
    pub procedures: BTreeMap<u32, String>,
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub component_uses: Vec<ComponentUse>,
    #[serde(default)]
    pub font_uses: Vec<FontUse>,
    #[serde(default)]
    pub font_maps: Vec<FontMapUse>,
    #[serde(default)]
    pub initial_fonts: Vec<InitialFontUse>,
    #[serde(default)]
    pub frames: Option<crate::frames::Identity>,
    #[serde(default)]
    pub database: Option<crate::database::Identity>,
    #[serde(default)]
    pub enums: Option<crate::enums::Identity>,
    #[serde(default)]
    pub variables: Option<crate::variable_bindings::Plan>,
}

impl Plan {
    /// Start with scalar inputs unbound. Components acquire a role only when
    /// the author selects a curated frame binding in the generated plan.
    pub fn from_root(
        scripts: &BTreeMap<u32, Vec<u8>>,
        book: &Book,
        pack_root: &Path,
        procedure: u32,
        name: &str,
    ) -> Result<Self> {
        ensure!(source::is_valid_name(name), "invalid export name");
        let root = semantic::normalize(
            scripts.get(&procedure).context("root procedure missing")?,
            book,
        )?;
        Ok(Self {
            format: IMPORT_FORMAT,
            source_build: book.profile.build,
            source_client_md5: book.profile.client_md5.clone(),
            source_script_index_sha256: book.profile.script_index_sha256.clone(),
            source_scripts_sha256: scripts
                .iter()
                .map(|(id, bytes)| (*id, digest(bytes)))
                .collect(),
            base_scripts_sha256: digest(&std::fs::read(pack_root.join("client.scripts.js5"))?),
            base_interfaces_sha256: digest(&std::fs::read(
                pack_root.join("client.interfaces.js5"),
            )?),
            procedures: scripts
                .keys()
                .map(|id| {
                    (
                        *id,
                        if *id == procedure {
                            format!("{name}_body")
                        } else {
                            format!("{name}_dependency_{id}")
                        },
                    )
                })
                .collect(),
            component_uses: Vec::new(),
            font_uses: Vec::new(),
            font_maps: Vec::new(),
            initial_fonts: Vec::new(),
            frames: None,
            database: None,
            enums: None,
            variables: None,
            entries: vec![Entry {
                procedure,
                name: name.into(),
                int_arguments: vec![IntArgument::Input; usize::from(root.args.int)],
                string_arguments: vec![None; usize::from(root.args.object)],
                long_arguments: vec![None; usize::from(root.args.long)],
                active_component: None,
            }],
        })
    }
}

/// Bind source initial state for one freshly loaded named target frame.
/// Choose a recorded source frame or an explicit authored source font.
/// A target write makes this declaration unavailable to later reads.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialFontUse {
    pub procedure: u32,
    pub instruction: usize,
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_font: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_frame: Option<crate::frames::FrameRef>,
}

#[derive(Serialize)]
pub struct InitialFontBinding {
    pub source_frame: Option<crate::frames::FontContract>,
    pub procedure: u32,
    pub instruction: usize,
    pub component: String,
    pub source_font: i32,
    pub target_font: i32,
}

/// Select a target font from a named frame at one proven donor font use.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FontUse {
    pub procedure: u32,
    pub instruction: usize,
    pub source_font: i32,
    pub from_component: Option<String>,
}

#[derive(Serialize)]
pub struct FontBinding {
    pub procedure: u32,
    pub instruction: usize,
    pub source_font: i32,
    pub target_font: i32,
    pub from_component: Option<String>,
}

/// Author the closed asset choices for one runtime-valued font consumer.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FontMapUse {
    pub procedure: u32,
    pub instruction: usize,
    pub fonts: Vec<FontChoice>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FontChoice {
    pub source_font: i32,
    pub from_component: Option<String>,
}

#[derive(Serialize)]
pub struct FontMapBinding {
    pub procedure: u32,
    pub instruction: usize,
    pub fonts: Vec<FontChoiceBinding>,
    pub resource_sha256: String,
}

#[derive(Serialize)]
pub struct FontChoiceBinding {
    pub source_font: i32,
    pub target_font: i32,
    pub from_component: Option<String>,
}

/// Bind only the component consumed by this host instruction. Copies and
/// ordinary integer uses of the donor value retain their original meaning.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentUse {
    pub procedure: u32,
    pub instruction: usize,
    pub component: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub procedure: u32,
    pub name: String,
    pub int_arguments: Vec<IntArgument>,
    /// None remains unbound; modern object inputs need an explicit string value.
    #[serde(default)]
    pub string_arguments: Vec<Option<String>>,
    pub long_arguments: Vec<Option<i64>>,
    #[serde(default)]
    pub active_component: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum IntArgument {
    Input,
    Value(i32),
    /// A curated interface/component spelling validated against the target pack.
    Component(String),
}

#[derive(Serialize)]
pub struct Report {
    pub format: u32,
    pub source_build: Build,
    pub source_client_md5: String,
    pub source_profile_sha256: String,
    pub source_script_index_sha256: String,
    pub adapter_sha256: String,
    pub plan_sha256: String,
    pub inames_sha256: String,
    pub database: Option<crate::database::Identity>,
    pub procedures: Vec<Procedure>,
    pub exports: BTreeMap<String, i32>,
    pub text_bindings: Vec<WidgetBinding>,
    pub operation_bindings: Vec<WidgetBinding>,
    pub visibility_bindings: Vec<WidgetBinding>,
    pub paint_bindings: Vec<WidgetBinding>,
    pub text_property_bindings: Vec<WidgetBinding>,
    pub font_bindings: Vec<FontBinding>,
    pub font_map_bindings: Vec<FontMapBinding>,
    pub initial_font_bindings: Vec<InitialFontBinding>,
    pub child_removal_bindings: Vec<WidgetBinding>,
    pub child_slot_bindings: Vec<WidgetBinding>,
    pub child_selection_bindings: Vec<WidgetBinding>,
    pub child_creation_bindings: Vec<WidgetBinding>,
    pub component_selection_bindings: Vec<WidgetBinding>,
    pub callback_bindings: Vec<WidgetBinding>,
    pub bridges: Vec<BridgeReport>,
    pub variable_bindings: Vec<crate::variable_bindings::Bound>,
}

#[derive(Serialize)]
pub struct Procedure {
    pub source: u32,
    pub target: i32,
    pub name: String,
    pub source_sha256: String,
    pub target_sha256: String,
}

#[derive(Serialize, Eq, Ord, PartialEq, PartialOrd)]
pub struct WidgetBinding {
    pub procedure: i32,
    pub instruction: usize,
    pub component: String,
}

/// Named calls and proven callback installations form the dependency closure.
/// Unknown handlers or callback heads remain explicit gaps.
#[derive(Serialize)]
pub struct ClosureInspection {
    pub format: u32,
    pub source_build: Build,
    pub source_client_md5: String,
    pub source_profile_sha256: String,
    pub dependency_complete: bool,
    pub calls: Vec<CallLink>,
    pub callbacks: Vec<flow::Callback>,
    pub source_analysis: Vec<flow::Diagnostic>,
    pub database_fields: Vec<flow::DatabaseField>,
    pub enum_uses: Vec<flow::EnumUse>,
    pub font_uses: Vec<flow::FontUse>,
    pub unresolved_dependencies: Vec<DependencyGap>,
    pub scripts: BTreeMap<u32, semantic::Script>,
}

#[derive(Serialize)]
pub struct CallLink {
    pub caller: u32,
    pub instruction: usize,
    pub callee: u32,
}

#[derive(Serialize)]
pub struct DependencyGap {
    pub caller: u32,
    pub instruction: usize,
    pub opcode: u16,
    pub reason: DependencyGapKind,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyGapKind {
    UnrecoveredHandler,
    CallbackHeadNeedsStackProof,
    MissingNamedCallee,
    MissingCallbackCallee,
    DatabaseFieldNeedsSchema,
}

pub fn inspect_closure(scripts: &BTreeMap<u32, Vec<u8>>, book: &Book) -> Result<ClosureInspection> {
    inspect_closure_with_database(scripts, book, None)
}
pub fn inspect_closure_with_database(
    scripts: &BTreeMap<u32, Vec<u8>>,
    book: &Book,
    database: Option<&crate::database::Definitions>,
) -> Result<ClosureInspection> {
    let normalized = scripts
        .iter()
        .map(|(id, bytes)| Ok((*id, semantic::normalize(bytes, book)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let flow = flow::inspect_with_database(&normalized, book, database)?;
    let mut calls = Vec::new();
    let mut unresolved_dependencies = Vec::new();
    for (id, script) in &normalized {
        for (pc, instruction) in script.instructions.iter().enumerate() {
            let reason = match &instruction.operation {
                None => Some(DependencyGapKind::UnrecoveredHandler),
                Some(operation) if operation.command == "db_getfield" => {
                    let site = &flow.database_fields[&(*id, pc)];
                    (site.field.is_none() || site.pushes.is_none())
                        .then_some(DependencyGapKind::DatabaseFieldNeedsSchema)
                }
                Some(_) if book.opcode(instruction.wire.opcode)?.is_hook() => {
                    match &flow.callbacks[&(*id, pc)].status {
                        flow::CallbackStatus::Installed { callee }
                            if !normalized.contains_key(callee) =>
                        {
                            Some(DependencyGapKind::MissingCallbackCallee)
                        }
                        flow::CallbackStatus::Unresolved { .. } => {
                            Some(DependencyGapKind::CallbackHeadNeedsStackProof)
                        }
                        _ => None,
                    }
                }
                Some(operation) => {
                    if let Argument::Script(callee) = operation.argument {
                        calls.push(CallLink {
                            caller: *id,
                            instruction: pc,
                            callee,
                        });
                        if !normalized.contains_key(&callee) {
                            Some(DependencyGapKind::MissingNamedCallee)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            };
            if let Some(reason) = reason {
                unresolved_dependencies.push(DependencyGap {
                    caller: *id,
                    instruction: pc,
                    opcode: instruction.wire.opcode,
                    reason,
                });
            }
        }
    }
    Ok(ClosureInspection {
        format: IMPORT_FORMAT,
        source_build: book.profile.build,
        source_client_md5: book.profile.client_md5.clone(),
        source_profile_sha256: book.sha256.clone(),
        dependency_complete: unresolved_dependencies.is_empty(),
        calls,
        callbacks: flow.callbacks.into_values().collect(),
        source_analysis: flow.diagnostics,
        database_fields: flow.database_fields.into_values().collect(),
        enum_uses: flow.enum_uses.into_values().collect(),
        font_uses: flow.font_uses.into_values().collect(),
        unresolved_dependencies,
        scripts: normalized,
    })
}

/// Load named calls and callbacks whose heads are proven by complete source
/// flow. Missing index entries and unresolved heads remain inspection gaps;
/// rostered groups still require their checksum-verified bytes locally.
pub fn load_closure(
    root: &Path,
    book: &Book,
    roots: impl Iterator<Item = u32>,
) -> Result<BTreeMap<u32, Vec<u8>>> {
    load_closure_with_database(root, book, roots, None)
}
pub fn load_closure_with_database(
    root: &Path,
    book: &Book,
    roots: impl Iterator<Item = u32>,
    database: Option<&crate::database::Definitions>,
) -> Result<BTreeMap<u32, Vec<u8>>> {
    let (_, index) = corpus::load_index(root, book)?;
    let mut pending: Vec<_> = roots.collect();
    for root in &pending {
        ensure!(
            index.group_id.binary_search(root).is_ok(),
            "root procedure {root} is absent from the donor index"
        );
    }
    let mut scripts = BTreeMap::new();
    loop {
        while let Some(id) = pending.pop() {
            if scripts.contains_key(&id) || index.group_id.binary_search(&id).is_err() {
                continue;
            }
            let bytes = corpus::load_group(root, book.profile.script_archive, &index, id)?
                .remove(&SCRIPT_FILE)
                .with_context(|| format!("donor procedure {id} lacks file zero"))?;
            let script = semantic::normalize(&bytes, book)
                .with_context(|| format!("donor procedure {id}"))?;
            for instruction in script.instructions {
                if let Some(operation) = instruction.operation
                    && let Argument::Script(callee) = operation.argument
                {
                    pending.push(callee);
                }
            }
            scripts.insert(id, bytes);
        }
        let inspection = inspect_closure_with_database(&scripts, book, database)?;
        for callback in inspection.callbacks {
            if let flow::CallbackStatus::Installed { callee } = callback.status
                && !scripts.contains_key(&callee)
                && index.group_id.binary_search(&callee).is_ok()
            {
                pending.push(callee);
            }
        }
        if pending.is_empty() {
            return Ok(scripts);
        }
    }
}

/// Build from verified raw donor bytes. The CLI obtains these from load_closure;
/// callers can also supply independently authored scripts using the same book.
pub struct BuildInput<'a> {
    pub scripts: &'a BTreeMap<u32, Vec<u8>>,
    pub book: &'a Book,
    pub adapter_bytes: &'a [u8],
    pub plan: &'a Plan,
    pub pack_root: &'a Path,
    pub inames_text: &'a str,
    pub database: Option<&'a crate::database::Definitions>,
    pub enums: Option<&'a crate::enums::Definitions>,
    pub variables: Option<crate::variable_bindings::Import<'a>>,
    pub frames: Option<&'a crate::frames::Definitions>,
}

pub fn build(input: BuildInput<'_>, output: &Path) -> Result<Report> {
    let BuildInput {
        scripts,
        book,
        adapter_bytes,
        plan,
        pack_root,
        inames_text,
        database,
        enums,
        variables,
        frames,
    } = input;
    ensure!(
        !output.exists(),
        "import output already exists: {}",
        output.display()
    );
    ensure!(
        plan.format == IMPORT_FORMAT && !plan.entries.is_empty(),
        "invalid or empty import plan"
    );
    ensure!(
        plan.source_build == book.profile.build
            && plan.source_client_md5 == book.profile.client_md5
            && plan.source_script_index_sha256 == book.profile.script_index_sha256,
        "plan donor client or script index changed"
    );
    ensure!(
        plan.database
            == database
                .map(crate::database::Definitions::identity)
                .transpose()?,
        "plan database resource identity changed or is missing"
    );
    let resource: Option<Arc<[u8]>> = database
        .map(|database| database.database().encode_resource().map(Arc::from))
        .transpose()?;
    ensure!(
        plan.enums == enums.map(crate::enums::Definitions::identity).transpose()?,
        "plan enum resource identity changed or is missing"
    );
    let enum_resource: Option<Arc<[u8]>> = enums
        .map(|enums| enums.library.encode_resource().map(Arc::from))
        .transpose()?;
    let variable_bindings = match (variables, &plan.variables) {
        (Some(variables), Some(plan)) => {
            Some(variables.resolve(plan, scripts, book, pack_root, database)?)
        }
        (None, None) => None,
        _ => anyhow::bail!("plan variable bindings changed or are missing"),
    };
    ensure!(
        plan.frames == frames.map(crate::frames::Definitions::identity),
        "plan source frame identity changed or is missing"
    );
    let target = OpcodeBook::embedded()?;
    let adapter = Adapter::parse(adapter_bytes, book, &target)?;
    let base = std::fs::read(pack_root.join("client.scripts.js5"))?;
    let interface_bytes = std::fs::read(pack_root.join("client.interfaces.js5"))?;
    ensure!(
        digest(&base) == plan.base_scripts_sha256,
        "target script pack changed since the plan was prepared"
    );
    ensure!(
        digest(&interface_bytes) == plan.base_interfaces_sha256,
        "target interface pack changed since the plan was prepared"
    );
    let archive = PackArchive::from_bytes(base)?;
    let interfaces = PackArchive::from_bytes(interface_bytes)?;
    let (interface_names, component_names) = native910::inames::parse_inames_txt(inames_text)?;
    let names = InterfaceRegistry::build(
        &interface_names,
        &component_names,
        &native910::inames::load_interface_roster(pack_root)?,
    )?;
    let mut next = archive
        .group_ids()
        .max()
        .unwrap_or_default()
        .checked_add(NEXT_ID)
        .context("target ID space exhausted")?;
    let mut allocate = || -> Result<i32> {
        let id = i32::try_from(next)?;
        next = next
            .checked_add(NEXT_ID)
            .context("target ID space exhausted")?;
        Ok(id)
    };
    let inspection = inspect_closure_with_database(scripts, book, database)?;
    let mut pending: Vec<_> = plan.entries.iter().map(|entry| entry.procedure).collect();
    let mut donors = BTreeMap::new();
    while let Some(id) = pending.pop() {
        if donors.contains_key(&id) {
            continue;
        }
        let bytes = scripts
            .get(&id)
            .with_context(|| format!("missing donor procedure {id}"))?;
        let script =
            semantic::normalize(bytes, book).with_context(|| format!("donor procedure {id}"))?;
        for instruction in &script.instructions {
            if let Some(operation) = &instruction.operation
                && let Argument::Script(callee) = operation.argument
            {
                pending.push(callee);
            }
        }
        for callback in &inspection.callbacks {
            if callback.caller == id
                && let flow::CallbackStatus::Installed { callee } = callback.status
            {
                pending.push(callee);
            }
        }
        donors.insert(id, script);
    }
    ensure!(
        plan.procedures.keys().copied().collect::<BTreeSet<_>>()
            == donors.keys().copied().collect(),
        "procedure names must cover exactly the selected closure: {:?}",
        donors.keys().collect::<Vec<_>>()
    );
    ensure!(
        plan.source_scripts_sha256
            == donors
                .iter()
                .map(|(id, script)| (*id, script.source_sha256.clone()))
                .collect(),
        "plan donor closure bytes changed"
    );
    let links = donors
        .keys()
        .map(|id| Ok((*id, allocate()?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut lowered = BTreeMap::new();
    let mut requirements = BTreeMap::new();
    let mut curated = Vec::new();
    let mut procedures = Vec::new();
    let mut declarations = BTreeMap::new();
    let mut use_bindings = BTreeMap::new();
    for binding in &plan.component_uses {
        let donor = donors
            .get(&binding.procedure)
            .context("component use is outside the selected closure")?;
        let instruction = donor
            .instructions
            .get(binding.instruction)
            .context("component use instruction is absent")?;
        ensure!(
            matches!(
                adapter.target_for(instruction)?.requirement,
                Requirement::ExplicitComponentText
                    | Requirement::ExplicitPlainText
                    | Requirement::ExplicitOperationLabel
                    | Requirement::ExplicitVisibility
                    | Requirement::ExplicitComponentPaint
                    | Requirement::ExplicitRuntimeChildren
                    | Requirement::ExplicitRuntimeChildSlot
                    | Requirement::ExplicitRuntimeChildSelection
                    | Requirement::FlatChildSelection
                    | Requirement::FlatTextChildCreation
                    | Requirement::ExplicitComponentSelection
                    | Requirement::OperationCallback
            ),
            "instruction has no bindable component use"
        );
        ensure!(
            use_bindings
                .insert(
                    (binding.procedure, binding.instruction),
                    resolve_component(&binding.component, &names, &interfaces)?
                )
                .is_none(),
            "duplicate component use binding"
        );
    }
    let mut font_uses = BTreeMap::new();
    let mut font_bindings = Vec::new();
    for binding in &plan.font_uses {
        let donor = donors
            .get(&binding.procedure)
            .context("font use procedure missing")?;
        let instruction = donor
            .instructions
            .get(binding.instruction)
            .context("font use instruction missing")?;
        let target_use = adapter.target_for(instruction)?;
        ensure!(
            matches!(
                target_use.host_operation,
                Some(native910::execution::HostOperation::ComponentText {
                    property: native910::execution::TextProperty::Font { .. },
                    ..
                })
            ),
            "instruction has no bindable font use"
        );
        let target_font = resolve_font_style(
            binding.source_font,
            binding.from_component.as_deref(),
            &names,
            &interfaces,
        )?;
        let mapping = native910::execution::FontMapping {
            source: binding.source_font,
            target: target_font,
        };
        ensure!(
            font_uses
                .insert((binding.procedure, binding.instruction), mapping)
                .is_none(),
            "duplicate font use binding"
        );
        font_bindings.push(FontBinding {
            procedure: binding.procedure,
            instruction: binding.instruction,
            source_font: binding.source_font,
            target_font,
            from_component: binding.from_component.clone(),
        });
    }
    let mut font_maps = BTreeMap::new();
    let mut font_map_bindings = Vec::new();
    for binding in &plan.font_maps {
        let site = (binding.procedure, binding.instruction);
        ensure!(
            !font_uses.contains_key(&site),
            "font map overlaps a constant font binding"
        );
        let instruction = donors
            .get(&binding.procedure)
            .context("font map procedure missing")?
            .instructions
            .get(binding.instruction)
            .context("font map instruction missing")?;
        ensure!(
            matches!(
                adapter.target_for(instruction)?.host_operation,
                Some(native910::execution::HostOperation::ComponentText {
                    property: native910::execution::TextProperty::Font { .. },
                    ..
                })
            ),
            "font map needs a font setter"
        );
        let mut choices = Vec::new();
        let mut mappings = Vec::new();
        for font in &binding.fonts {
            let target_font = resolve_font_style(
                font.source_font,
                font.from_component.as_deref(),
                &names,
                &interfaces,
            )?;
            mappings.push(native910::execution::FontMapping {
                source: font.source_font,
                target: target_font,
            });
            choices.push(FontChoiceBinding {
                source_font: font.source_font,
                target_font,
                from_component: font.from_component.clone(),
            });
        }
        let resource = native910::execution::FontMappings::new(mappings)?.resource();
        choices.sort_by_key(|choice| choice.source_font);
        font_map_bindings.push(FontMapBinding {
            procedure: binding.procedure,
            instruction: binding.instruction,
            fonts: choices,
            resource_sha256: digest(&resource),
        });
        ensure!(
            font_maps
                .insert(site, Arc::<[u8]>::from(resource))
                .is_none(),
            "duplicate font map use"
        );
    }
    let mut initial_fonts = BTreeMap::new();
    let mut initial_font_bindings = Vec::new();
    let mut initial_frames = BTreeMap::new();
    for binding in &plan.initial_fonts {
        let (source_font, source_frame) = match (binding.source_font, binding.source_frame) {
            (Some(font), None) => (font, None),
            (None, Some(frame)) => {
                let contract = frames
                    .context("initial font needs donor frame definitions")?
                    .font_contract(frame)?;
                (contract.font, Some(contract))
            }
            _ => anyhow::bail!("initial font needs exactly one source_font or source_frame"),
        };
        let instruction = donors
            .get(&binding.procedure)
            .context("initial font procedure missing")?
            .instructions
            .get(binding.instruction)
            .context("initial font instruction missing")?;
        ensure!(
            matches!(
                adapter.target_for(instruction)?.host_operation,
                Some(native910::execution::HostOperation::ComponentText {
                    property: native910::execution::TextProperty::ReadFont { .. },
                    ..
                })
            ),
            "initial font declaration needs a font reader"
        );
        let component = resolve_component(&binding.component, &names, &interfaces)?;
        const ABSENT_FONT: i32 = -1;
        let target_font = component
            .font
            .or_else(|| component.plain_text.then_some(ABSENT_FONT))
            .context("initial font declaration needs a text frame")?;
        ensure!(
            (source_font == ABSENT_FONT) == (target_font == ABSENT_FONT),
            "initial font presence differs between source and target"
        );
        let mapping = native910::execution::FontMapping {
            source: source_font,
            target: target_font,
        };
        if let Some(previous) = initial_frames.insert(component.packed, mapping) {
            ensure!(
                previous == mapping,
                "conflicting source initial font declarations on one target frame"
            );
        }
        let initial = native910::execution::InitialFontMapping {
            component: component.packed,
            mapping,
        };
        ensure!(
            initial_fonts
                .insert((binding.procedure, binding.instruction), initial)
                .is_none(),
            "duplicate initial font declaration"
        );
        initial_font_bindings.push(InitialFontBinding {
            procedure: binding.procedure,
            instruction: binding.instruction,
            component: binding.component.clone(),
            source_font,
            source_frame,
            target_font,
        });
    }
    let mut bridges = Vec::new();
    let mut execution = ExecutionTable::default();
    for (id, donor) in &donors {
        let name = &plan.procedures[id];
        let mut uses = BTreeMap::new();
        for (pc, instruction) in donor.instructions.iter().enumerate() {
            let component = use_bindings.get(&(*id, pc));
            let target_use = adapter.target_for(instruction).ok();
            let callback = target_use.is_some_and(|target| {
                matches!(
                    target.requirement,
                    Requirement::OperationCallback | Requirement::PlayerVariableCallback
                )
            });
            let database_use = target_use.is_some_and(|target| {
                matches!(
                    target.requirement,
                    Requirement::DatabaseField | Requirement::DatabaseFieldCount
                )
            });
            let enum_use = target_use.is_some_and(|target| target.requirement == Requirement::Enum);
            let variable_use =
                target_use.is_some_and(|target| target.requirement == Requirement::VariableBit);
            if !callback
                && !enum_use
                && !variable_use
                && !database_use
                && component.is_none()
                && target_use.is_none_or(|target| target.host_operation.is_none())
            {
                continue;
            }
            let target_use = target_use.context("component use has no reviewed lowering")?;
            let (secondary, variable) = if variable_use {
                let Some(crate::semantic::Operation {
                    argument: Argument::Varbit { id, secondary },
                    ..
                }) = &instruction.operation
                else {
                    anyhow::bail!("bit use needs its full source reference");
                };
                (
                    *secondary,
                    Some(
                        variable_bindings
                            .as_ref()
                            .context("bit use has no named live bindings")?
                            .bit(*id, *secondary)?,
                    ),
                )
            } else {
                let crate::wire::Operand::Byte(secondary) = instruction.wire.operand else {
                    anyhow::bail!("bound host use needs a byte operand");
                };
                (secondary, None)
            };
            let callback_site = callback
                .then(|| {
                    inspection
                        .callbacks
                        .iter()
                        .find(|site| site.caller == *id && site.instruction == pc)
                })
                .flatten();
            let triggers = if target_use.requirement == Requirement::PlayerVariableCallback {
                Some(
                    variable_bindings
                        .as_ref()
                        .context("callback has no named variable bindings")?
                        .triggers(callback_site.context("callback has no source proof")?)?,
                )
            } else {
                None
            };
            let bridge_id = allocate()?;
            let bridge_name = format!("{name}_use_{pc}");
            let mut generated = bridge::generate(
                bridge::Request {
                    target: target_use,
                    operand: &Operand::Byte(secondary),
                    callback: callback_site,
                    triggers: triggers.as_deref(),
                    component,
                    policy: adapter.callback_policy.as_ref(),
                    links: &links,
                    name: &bridge_name,
                    variable,
                    font: font_uses
                        .get(&(*id, pc))
                        .copied()
                        .map(bridge::FontUse::Write)
                        .or_else(|| {
                            font_maps
                                .get(&(*id, pc))
                                .cloned()
                                .map(bridge::FontUse::Mapped)
                        })
                        .or_else(|| {
                            initial_fonts
                                .get(&(*id, pc))
                                .copied()
                                .map(bridge::FontUse::Initial)
                        }),
                    enums: if enum_use {
                        Some(bridge::EnumUse {
                            site: inspection
                                .enum_uses
                                .iter()
                                .find(|site| site.caller == *id && site.instruction == pc)
                                .context("enum use has no source traffic proof")?,
                            definitions: enums.context("enum definitions missing")?,
                            resource: enum_resource.clone().context("enum resource missing")?,
                        })
                    } else {
                        None
                    },
                    database: if database_use {
                        Some(bridge::DatabaseUse {
                            site: inspection
                                .database_fields
                                .iter()
                                .find(|site| site.caller == *id && site.instruction == pc)
                                .context("database use has no source field proof")?,
                            definitions: database.context("database definitions missing")?,
                            resource: resource.clone().context("database resource missing")?,
                        })
                    } else {
                        None
                    },
                },
                &target,
            )
            .with_context(|| format!("donor procedure {id} @{pc}"))?;
            if let Some(native910::execution::HostOperation::CreateFlatTextChild {
                source, ..
            }) = &mut generated.host_operation
            {
                *source = Some((links[id], pc));
            }
            uses.insert(pc, bridge_id);
            if let Some((pc, component)) = generated.declaration {
                declarations
                    .entry(bridge_id)
                    .or_insert_with(BTreeMap::new)
                    .insert(pc, component);
            }
            let host_pc = generated.requirement.pc;
            let host_operations = generated
                .host_operation
                .map(|operation| (host_pc, operation))
                .into_iter()
                .collect();
            let bytes = execution.bind_import(
                bridge_id,
                &mut generated.script,
                &target,
                Specification {
                    resource: generated.resource.clone(),
                    role: Role::Adapter,
                    adapter_calls: BTreeMap::new(),
                    host_operations,
                },
            )?;
            bridges.push(bridge::BridgeReport {
                source: *id,
                instruction: pc,
                target: bridge_id,
                name: bridge_name.clone(),
                target_sha256: digest(&bytes),
                component: component.map(|component| component.symbol.clone()),
                callback_source: generated.callback.map(|(source, _)| source),
                callback_target: generated.callback.map(|(_, target)| target),
                resource_sha256: generated.resource.as_ref().map(|resource| digest(resource)),
                host_operation: generated
                    .host_operation
                    .map(|operation| operation.spelling()),
            });
            curated.push((bridge_id, bridge_name));
            requirements.insert(bridge_id, vec![generated.requirement]);
            lowered.insert(bridge_id, generated.script);
        }
        let mut imported = adapter
            .lower_with_uses(donor, &links, name, &uses)
            .with_context(|| format!("donor procedure {id}"))?;
        let bytes = execution.bind_import(
            links[id],
            &mut imported.script,
            &target,
            Specification {
                resource: None,
                role: Role::Source,
                adapter_calls: uses,
                host_operations: BTreeMap::new(),
            },
        )?;
        ensure!(
            native910::script::decode_script(&bytes, &target)? == imported.script,
            "lowered procedure did not round trip"
        );
        procedures.push(Procedure {
            source: *id,
            target: links[id],
            name: name.clone(),
            source_sha256: donor.source_sha256.clone(),
            target_sha256: digest(&bytes),
        });
        curated.push((links[id], name.clone()));
        requirements.insert(links[id], imported.host_requirements);
        lowered.insert(links[id], imported.script);
    }
    let mut exports = BTreeMap::new();
    for entry in &plan.entries {
        let body = &lowered[&links[&entry.procedure]];
        ensure!(
            entry.int_arguments.len() == usize::from(body.args.int)
                && entry.string_arguments.len() == usize::from(body.args.obj)
                && entry.long_arguments.len() == usize::from(body.args.long),
            "entry {} argument count mismatch",
            entry.name
        );
        let id = allocate()?;
        ensure!(
            source::is_valid_name(&entry.name) && exports.insert(entry.name.clone(), id).is_none(),
            "invalid or duplicate export name"
        );
        let instruction = |command: &str, operand| -> Result<Instruction> {
            Ok(Instruction {
                opcode: target.opcode_for(command)?,
                command: command.into(),
                operand,
            })
        };
        let mut code = Vec::new();
        let mut components = BTreeMap::new();
        let mut args = Counts::default();
        let mut entry_operations = BTreeMap::new();
        if let Some(symbol) = &entry.active_component {
            let component = resolve_component(symbol, &names, &interfaces)?;
            code.push(instruction(
                "push_constant_string",
                Operand::Int(component.packed),
            )?);
            components.insert(usize::default(), component);
            let select = code.len();
            code.push(instruction("if_find", Operand::Byte(u8::default()))?);
            code.push(instruction(
                "pop_int_discard",
                Operand::Byte(u8::default()),
            )?);
            entry_operations.insert(select, native910::execution::HostOperation::FindComponent);
            requirements.insert(
                id,
                vec![HostRequirement {
                    pc: select,
                    requirement: Requirement::ExplicitComponentSelection,
                }],
            );
        }
        for argument in &entry.int_arguments {
            let operand = match argument {
                IntArgument::Input => {
                    let slot = args.int;
                    args.int = args
                        .int
                        .checked_add(1)
                        .context("too many integer arguments")?;
                    code.push(instruction(
                        "push_int_local",
                        Operand::Local(i32::from(slot)),
                    )?);
                    continue;
                }
                IntArgument::Value(value) => Operand::Int(*value),
                IntArgument::Component(symbol) => {
                    let component = resolve_component(symbol, &names, &interfaces)?;
                    let packed = component.packed;
                    components.insert(code.len(), component);
                    Operand::Int(packed)
                }
            };
            code.push(instruction("push_constant_string", operand)?);
        }
        for argument in &entry.string_arguments {
            let value = argument.as_ref().with_context(|| {
                format!(
                    "entry {} has an unbound object argument; bind a fixed string",
                    entry.name
                )
            })?;
            code.push(instruction(
                "push_constant_string",
                Operand::Str(value.clone()),
            )?);
        }
        for argument in &entry.long_arguments {
            match argument {
                Some(value) => {
                    code.push(instruction("push_constant_string", Operand::Long(*value))?)
                }
                None => {
                    let slot = args.long;
                    args.long = args
                        .long
                        .checked_add(1)
                        .context("too many long arguments")?;
                    code.push(instruction(
                        "push_long_local",
                        Operand::Local(i32::from(slot)),
                    )?);
                }
            }
        }
        code.push(instruction(
            "gosub_with_params",
            Operand::Script(links[&entry.procedure]),
        )?);
        code.push(instruction("return", Operand::Byte(u8::default()))?);
        lowered.insert(
            id,
            CompiledScript {
                name: Some(format!("proc,{}", entry.name)),
                locals: args,
                args,
                code,
            },
        );
        execution
            .bind_import(
                id,
                lowered.get_mut(&id).context("generated entry missing")?,
                &target,
                Specification {
                    resource: None,
                    role: Role::Entry,
                    adapter_calls: BTreeMap::new(),
                    host_operations: entry_operations,
                },
            )
            .with_context(|| format!("target codec cannot encode generated procedure {id}"))?;
        declarations.insert(id, components);
        curated.push((id, entry.name.clone()));
    }
    let configs = ConfigTypes::load(pack_root)?;
    let analysis_options = execution.analysis_options();
    let bindings = verify_bindings(BindingInput {
        scripts: &lowered,
        requirements: &requirements,
        exports: &exports,
        declarations: &declarations,
        configs: &configs,
        callback_policy: adapter.callback_policy.as_ref(),
        options: &analysis_options,
        execution: Some(&execution),
    })?;
    // Bound values and generated bridge literals must survive the actual
    // target codec as well as the original donor bodies.
    for (id, script) in &lowered {
        let bytes = native910::script::encode_script(script, &target)
            .with_context(|| format!("target codec cannot encode generated procedure {id}"))?;
        ensure!(
            native910::script::decode_script(&bytes, &target)? == *script,
            "generated procedure {id} cannot round trip through the target codec"
        );
    }
    let (returns, _) =
        dataflow::infer_summaries_with_options(&lowered, &configs, &analysis_options);
    let symbols = source::registry_for_decoded_scripts(&lowered, &curated, &returns)?;
    let mut sources = BTreeMap::new();
    for (id, script) in &lowered {
        let text = source::format_source(
            &source::lift(script, &symbols, &configs, &names)?,
            &symbols,
            &names,
        )?;
        ensure!(
            source::assemble_source(&text, &target, &symbols, &configs, &names)?
                == native910::script::encode_script(script, &target)?,
            "generated source changed imported procedure {id}"
        );
        sources.insert(*id, text);
    }
    let report = Report {
        format: IMPORT_FORMAT,
        source_build: book.profile.build,
        source_client_md5: book.profile.client_md5.clone(),
        source_profile_sha256: book.sha256.clone(),
        source_script_index_sha256: book.profile.script_index_sha256.clone(),
        adapter_sha256: digest(adapter_bytes),
        plan_sha256: digest(&serde_json::to_vec(plan)?),
        inames_sha256: digest(inames_text.as_bytes()),
        database: plan.database.clone(),
        procedures,
        exports,
        text_bindings: bindings.text.into_iter().collect(),
        operation_bindings: bindings.operations.into_iter().collect(),
        visibility_bindings: bindings.visibility.into_iter().collect(),
        paint_bindings: bindings.paint.into_iter().collect(),
        text_property_bindings: bindings.text_properties.into_iter().collect(),
        font_bindings,
        font_map_bindings,
        initial_font_bindings,
        child_removal_bindings: bindings.child_removal.into_iter().collect(),
        child_slot_bindings: bindings.child_slot.into_iter().collect(),
        child_selection_bindings: bindings.child_selection.into_iter().collect(),
        child_creation_bindings: bindings.child_creation.into_iter().collect(),
        component_selection_bindings: bindings.component_selection.into_iter().collect(),
        callback_bindings: bindings.callbacks.into_iter().collect(),
        bridges,
        variable_bindings: variable_bindings
            .map(|resolved| resolved.bindings.into_values().collect())
            .unwrap_or_default(),
    };
    Prepared {
        sources: &sources,
        curated: &curated,
        scripts: &lowered,
        plan,
        inames: inames_text,
        report: &report,
        book: &target,
        execution: &execution,
    }
    .publish(pack_root, output)?;
    Ok(report)
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ComponentBinding {
    pub(crate) symbol: String,
    pub(crate) packed: i32,
    pub(crate) plain_text: bool,
    pub(crate) container: bool,
    pub(crate) font: Option<i32>,
}

fn resolve_font_style(
    source_font: i32,
    symbol: Option<&str>,
    names: &InterfaceRegistry,
    interfaces: &PackArchive,
) -> Result<i32> {
    const ABSENT_FONT: i32 = -1;
    if let Some(symbol) = symbol {
        ensure!(
            source_font != ABSENT_FONT,
            "an absent donor font must remain absent"
        );
        resolve_component(symbol, names, interfaces)?
            .font
            .context("font binding needs a text frame with a font")
    } else {
        ensure!(
            source_font == ABSENT_FONT,
            "a font asset needs a named target frame style"
        );
        Ok(ABSENT_FONT)
    }
}

fn resolve_component(
    symbol: &str,
    names: &InterfaceRegistry,
    interfaces: &PackArchive,
) -> Result<ComponentBinding> {
    let (interface_name, child) = symbol
        .split_once('/')
        .context("component binding needs an interface/component symbol")?;
    let (group, file) = names
        .resolve_component(interface_name, child)
        .with_context(|| format!("unknown component symbol {symbol}"))?;
    ensure!(
        names.child_name(group, file) == Some(child),
        "component binding needs a curated child name: {symbol}"
    );
    let packed = pack_component(ComponentRef {
        iface: group,
        child: i32::try_from(file)?,
    })
    .context("component address out of range")?;
    let files = interfaces
        .group_files(u32::try_from(group)?)?
        .context("bound interface is absent")?;
    let bytes = files.get(&file).context("bound component is absent")?;
    let body = interface::decode_component(bytes, packed)?.body;
    Ok(ComponentBinding {
        symbol: symbol.into(),
        packed,
        plain_text: matches!(body, ComponentBody::Text { .. }),
        container: matches!(body, ComponentBody::Layer { .. }),
        font: match body {
            ComponentBody::Text { font, .. } if font >= i32::default() => Some(font),
            _ => None,
        },
    })
}

#[derive(Default)]
struct VerifiedBindings {
    text: BTreeSet<WidgetBinding>,
    operations: BTreeSet<WidgetBinding>,
    visibility: BTreeSet<WidgetBinding>,
    paint: BTreeSet<WidgetBinding>,
    text_properties: BTreeSet<WidgetBinding>,
    child_removal: BTreeSet<WidgetBinding>,
    child_slot: BTreeSet<WidgetBinding>,
    child_selection: BTreeSet<WidgetBinding>,
    child_creation: BTreeSet<WidgetBinding>,
    component_selection: BTreeSet<WidgetBinding>,
    callbacks: BTreeSet<WidgetBinding>,
}

type ComponentArguments = Vec<Option<ComponentBinding>>;
type ComponentDeclarations = BTreeMap<i32, BTreeMap<usize, ComponentBinding>>;

// Argument identity tracks a role through copies, locals and agreeing joins.
// Only wrapper instructions explicitly emitted for a named binding can create
// that role; a donor constant with the same numeric value never acquires it.
fn component_binding<'a>(
    value: &Value,
    arguments: &'a ComponentArguments,
    declarations: Option<&'a BTreeMap<usize, ComponentBinding>>,
) -> Option<&'a ComponentBinding> {
    if let Some((INTEGER_LANE, slot)) = value.argument {
        return arguments.get(usize::from(slot)).and_then(Option::as_ref);
    }
    if value.definitions.len() == 1 {
        return declarations?.get(value.definitions.first()?);
    }
    None
}

struct BindingInput<'a> {
    scripts: &'a BTreeMap<i32, CompiledScript>,
    requirements: &'a BTreeMap<i32, Vec<HostRequirement>>,
    exports: &'a BTreeMap<String, i32>,
    declarations: &'a ComponentDeclarations,
    configs: &'a ConfigTypes,
    callback_policy: Option<&'a crate::lower910::CallbackPolicy>,
    options: &'a dataflow::AnalysisOptions,
    execution: Option<&'a ExecutionTable>,
}

fn verify_bindings(input: BindingInput<'_>) -> Result<VerifiedBindings> {
    let BindingInput {
        scripts,
        requirements,
        exports,
        declarations,
        configs,
        callback_policy,
        options,
        execution,
    } = input;
    let (summaries, values) = dataflow::infer_summaries_with_options(scripts, configs, options);
    let mut pending: VecDeque<_> = exports
        .values()
        .map(|id| {
            (
                *id,
                EntryContext::default(),
                ComponentArguments::default(),
                ActiveOwners::default(),
            )
        })
        .collect();
    let mut seen = BTreeSet::new();
    let mut bindings = VerifiedBindings::default();
    while let Some((id, arguments, component_arguments, active_owners)) = pending.pop_front() {
        if !seen.insert((
            id,
            arguments.clone(),
            component_arguments.clone(),
            active_owners.clone(),
        )) {
            continue;
        }
        ensure!(
            seen.len() <= MAX_BOUND_CONTEXTS,
            "too many import call contexts"
        );
        let script = &scripts[&id];
        let analysis = dataflow::analyze_with_options(
            script, scripts, &summaries, &values, configs, &arguments, options,
        );
        ensure!(
            analysis.failure.is_none(),
            "imported procedure {id} has unresolved stack/control flow: {:?}",
            analysis.failure
        );
        ensure!(
            matches!(
                analysis.arity,
                native910::returns::ReturnArity::Known { .. }
            ),
            "imported procedure {id} has no proven normal return"
        );
        let owners = active_owner_flow(OwnerInput {
            id,
            scripts,
            requirements,
            declarations,
            analysis: &analysis,
            components: &component_arguments,
            incoming: active_owners,
        })?;
        let accounting = execution
            .map(|table| table.accounting(id))
            .unwrap_or_default();
        for requirement in requirements.get(&id).into_iter().flatten() {
            let Some(state) = &analysis.before[requirement.pc] else {
                continue;
            };
            match requirement.requirement {
                Requirement::StringJoin => {
                    let Operand::Count(count) = script.code[requirement.pc].operand else {
                        anyhow::bail!("string join requires a modeled part count");
                    };
                    let objects = &state.stacks[OBJECT_LANE];
                    let start = objects
                        .len()
                        .checked_sub(usize::try_from(count)?)
                        .context("string join argument underflow")?;
                    ensure!(
                        objects[start..].iter().all(|value| value.nonnull_string),
                        "procedure {id} @{} needs proven string objects for joining",
                        requirement.pc
                    );
                }
                Requirement::ExplicitComponentText
                | Requirement::ExplicitPlainText
                | Requirement::ExplicitOperationLabel
                | Requirement::ExplicitVisibility
                | Requirement::ExplicitComponentPaint
                | Requirement::ExplicitRuntimeChildren
                | Requirement::ExplicitRuntimeChildSlot
                | Requirement::ExplicitRuntimeChildSelection
                | Requirement::FlatChildSelection
                | Requirement::FlatTextChildCreation
                | Requirement::ExplicitComponentSelection
                | Requirement::OperationCallback => {
                    let plain_text = requirement.requirement == Requirement::ExplicitPlainText;
                    let scalar_text = requirement.requirement == Requirement::ExplicitComponentText;
                    let container = matches!(
                        requirement.requirement,
                        Requirement::ExplicitRuntimeChildSlot
                            | Requirement::ExplicitRuntimeChildSelection
                            | Requirement::FlatTextChildCreation
                    );
                    let component = state.stacks[INTEGER_LANE]
                        .last()
                        .and_then(|value| {
                            component_binding(value, &component_arguments, declarations.get(&id))
                        })
                        .filter(|binding| {
                            state.int_from_top(0) == Some(binding.packed)
                                && (!plain_text || binding.plain_text)
                                && (!scalar_text || binding.plain_text || binding.container)
                                && (!container || binding.container)
                        })
                        .with_context(|| {
                            format!(
                                "procedure {id} @{} needs a bound {}component",
                                requirement.pc,
                                if plain_text {
                                    "plain-text "
                                } else if container {
                                    "container "
                                } else {
                                    ""
                                }
                            )
                        })?;
                    if requirement.requirement == Requirement::FlatTextChildCreation {
                        let accounting = execution
                            .context("creation metadata missing")?
                            .accounting(id);
                        let Some(native910::execution::HostOperation::CreateFlatTextChild {
                            kind_argument,
                            ..
                        }) = accounting.host_operations.get(&requirement.pc)
                        else {
                            anyhow::bail!("creation has no constructor contract");
                        };
                        let template =
                            rs910_config::ui_component_fields::TextChildTemplate::decode_resource(
                                accounting
                                    .resource
                                    .as_ref()
                                    .context("creation resource missing")?
                                    .bytes(),
                            )?;
                        ensure!(
                            matches!(state.locals[INTEGER_LANE].get(usize::from(*kind_argument))
                            .and_then(|value| value.constant.as_ref()), Some(dataflow::Constant::Int(kind))
                            if *kind as u8 == template.source_kind),
                            "creation kind needs a proven recorded constructor on every reaching path"
                        );
                    }
                    if matches!(
                        requirement.requirement,
                        Requirement::ExplicitPlainText | Requirement::ExplicitOperationLabel
                    ) {
                        ensure!(
                            state.stacks[OBJECT_LANE]
                                .last()
                                .is_some_and(|value| value.nonnull_string),
                            "procedure {id} @{} needs a proven string object",
                            requirement.pc
                        );
                    }
                    let uses = match requirement.requirement {
                        Requirement::ExplicitComponentText => &mut bindings.text_properties,
                        Requirement::ExplicitPlainText => &mut bindings.text,
                        Requirement::ExplicitOperationLabel => &mut bindings.operations,
                        Requirement::ExplicitVisibility => &mut bindings.visibility,
                        Requirement::ExplicitComponentPaint => &mut bindings.paint,
                        Requirement::ExplicitRuntimeChildren => &mut bindings.child_removal,
                        Requirement::ExplicitRuntimeChildSlot => &mut bindings.child_slot,
                        Requirement::FlatTextChildCreation => &mut bindings.child_creation,
                        Requirement::ExplicitRuntimeChildSelection
                        | Requirement::FlatChildSelection => &mut bindings.child_selection,
                        Requirement::ExplicitComponentSelection => {
                            &mut bindings.component_selection
                        }
                        _ => &mut bindings.callbacks,
                    };
                    uses.insert(WidgetBinding {
                        procedure: id,
                        instruction: requirement.pc,
                        component: component.symbol.clone(),
                    });
                }
                Requirement::ActiveComponentText
                | Requirement::PlayerVariableCallback
                | Requirement::ActiveComponentPaint => {
                    let bank = selector_bank(&script.code[requirement.pc])?;
                    let component = owners[requirement.pc]
                        .as_ref()
                        .and_then(|owners| owners[bank].as_ref())
                        .context(
                            "active host use needs a named frame selected on every reaching path",
                        )?;
                    ensure!(
                        requirement.requirement != Requirement::ActiveComponentText
                            || component.plain_text
                            || component.container,
                        "active text use needs a recorded text or container projection"
                    );
                    let uses = if requirement.requirement == Requirement::ActiveComponentText {
                        &mut bindings.text_properties
                    } else if requirement.requirement == Requirement::PlayerVariableCallback {
                        &mut bindings.callbacks
                    } else {
                        &mut bindings.paint
                    };
                    uses.insert(WidgetBinding {
                        procedure: id,
                        instruction: requirement.pc,
                        component: component.symbol.clone(),
                    });
                }
                Requirement::Intrinsic
                | Requirement::Enum
                | Requirement::VariableBit
                | Requirement::DatabaseField
                | Requirement::DatabaseFieldCount => {}
            }
        }
        for (pc, operation) in &accounting.host_operations {
            if let native910::execution::HostOperation::ComponentText {
                property:
                    native910::execution::TextProperty::ReadFont {
                        initial: Some(initial),
                        ..
                    },
                explicit,
                ..
            } = operation
                && let Some(state) = &analysis.before[*pc]
            {
                let component = if *explicit {
                    state.stacks[INTEGER_LANE].last().and_then(|value| {
                        component_binding(value, &component_arguments, declarations.get(&id))
                    })
                } else {
                    owners[*pc]
                        .as_ref()
                        .and_then(|owners| owners[selector_bank(&script.code[*pc]).ok()?].as_ref())
                }
                .context(
                    "initial font read needs its declared named frame on every reaching path",
                )?;
                ensure!(
                    component.packed == initial.component
                        && component.font == Some(initial.mapping.target),
                    "initial font read does not select its declared frame/font on every reaching path"
                );
            }
        }
        for (pc, operation) in &accounting.host_operations {
            let native910::execution::HostOperation::ComponentText {
                property:
                    native910::execution::TextProperty::Font {
                        mapping: native910::execution::FontBinding::Constant(mapping),
                        ..
                    },
                explicit,
                ..
            } = operation
            else {
                continue;
            };
            let Some(state) = &analysis.before[*pc] else {
                continue;
            };
            ensure!(
                state.int_from_top(usize::from(*explicit)) == Some(mapping.source),
                "procedure {id} @{pc} font value needs the pinned source font on every reaching path"
            );
        }
        for requirement in requirements.get(&id).into_iter().flatten() {
            if !matches!(
                requirement.requirement,
                Requirement::OperationCallback | Requirement::PlayerVariableCallback
            ) {
                continue;
            }
            let Some(state) = &analysis.before[requirement.pc] else {
                continue;
            };
            let policy = callback_policy.context("callback policy absent")?;
            if let Some(context) = callback_context(CallbackInput {
                instruction: &script.code[requirement.pc],
                state,
                scripts,
                configs,
                policy,
                components: &component_arguments,
                declarations: declarations.get(&id),
                operation: accounting.host_operations.get(&requirement.pc).copied(),
                owner: owners[requirement.pc].as_ref().and_then(|owners| {
                    owners[selector_bank(&script.code[requirement.pc]).ok()?].as_ref()
                }),
                options,
            })
            .with_context(|| format!("procedure {id} @{} callback", requirement.pc))?
            {
                pending.push_back(context);
            }
        }
        for (pc, instruction) in script.code.iter().enumerate() {
            let Some(state) = &analysis.before[pc] else {
                continue;
            };
            let object = match (&*instruction.command, &instruction.operand) {
                ("push_string_local", Operand::Local(slot)) => {
                    state.locals[OBJECT_LANE].get(usize::try_from(*slot)?)
                }
                ("pop_string_local", _) => state.stacks[OBJECT_LANE].last(),
                _ => None,
            };
            if matches!(
                &*instruction.command,
                "push_string_local" | "pop_string_local"
            ) {
                ensure!(
                    object.is_some_and(|value| value.nonnull_string),
                    "procedure {id} @{pc} needs a proven string object for local traffic"
                );
            }
            if let Operand::Script(callee) = instruction.operand {
                let callee_args = scripts
                    .get(&callee)
                    .context("unlinked target procedure")?
                    .args;
                let counts = [callee_args.int, callee_args.obj, callee_args.long];
                let mut arguments = EntryContext::default();
                for lane in [INTEGER_LANE, OBJECT_LANE, LONG_LANE] {
                    let start = state.stacks[lane]
                        .len()
                        .checked_sub(usize::from(counts[lane]))
                        .context("callee argument underflow")?;
                    if lane == OBJECT_LANE {
                        ensure!(
                            state.stacks[lane][start..]
                                .iter()
                                .all(|value| value.nonnull_string),
                            "procedure {id} @{pc} needs proven string objects for call arguments"
                        );
                        arguments.nonnull_strings = state.stacks[lane][start..]
                            .iter()
                            .map(|value| value.nonnull_string)
                            .collect();
                    }
                    arguments.arguments[lane] = state.stacks[lane][start..]
                        .iter()
                        .map(|value| value.constant.clone())
                        .collect();
                }
                let start = state.stacks[INTEGER_LANE].len() - usize::from(callee_args.int);
                let components = state.stacks[INTEGER_LANE][start..]
                    .iter()
                    .map(|value| {
                        component_binding(value, &component_arguments, declarations.get(&id))
                            .cloned()
                    })
                    .collect();
                pending.push_back((
                    callee,
                    arguments,
                    components,
                    owners[pc].clone().unwrap_or_default(),
                ));
            }
        }
    }
    Ok(bindings)
}

type ActiveOwners = [Option<ComponentBinding>; 2];
type BoundContext = (i32, EntryContext, ComponentArguments, ActiveOwners);

fn selector_bank(instruction: &Instruction) -> Result<usize> {
    let Operand::Byte(selector) = instruction.operand else {
        anyhow::bail!("active component selector is unresolved");
    };
    Ok(usize::from(selector != u8::default()))
}

struct OwnerInput<'a> {
    id: i32,
    scripts: &'a BTreeMap<i32, CompiledScript>,
    requirements: &'a BTreeMap<i32, Vec<HostRequirement>>,
    declarations: &'a ComponentDeclarations,
    analysis: &'a dataflow::Analysis,
    components: &'a ComponentArguments,
    incoming: ActiveOwners,
}

/// Carry only named owners through reachable selection sites. Calls may preserve
/// an existing owner or select the same owner; changing an inherited frame needs
/// an explicit effect model rather than an assumed global active component.
fn active_owner_flow(input: OwnerInput<'_>) -> Result<Vec<Option<ActiveOwners>>> {
    let OwnerInput {
        id,
        scripts,
        requirements,
        declarations,
        analysis,
        components,
        incoming,
    } = input;
    let script = &scripts[&id];
    let selection = |requirement: Requirement| {
        matches!(
            requirement,
            Requirement::ExplicitComponentSelection
                | Requirement::FlatChildSelection
                | Requirement::ExplicitRuntimeChildSelection
                | Requirement::FlatTextChildCreation
        )
    };
    let mut selections = BTreeMap::new();
    for (pc, instruction) in script.code.iter().enumerate() {
        let Some(state) = &analysis.before[pc] else {
            continue;
        };
        let direct = requirements
            .get(&id)
            .into_iter()
            .flatten()
            .find(|site| site.pc == pc && selection(site.requirement));
        let site = if direct.is_some() {
            let value = state.stacks[INTEGER_LANE]
                .last()
                .context("selection has no parent")?;
            component_binding(value, components, declarations.get(&id))
                .cloned()
                .map(|component| {
                    (
                        selector_bank(instruction),
                        component,
                        direct.is_some_and(|site| {
                            site.requirement == Requirement::FlatTextChildCreation
                        }),
                    )
                })
        } else if let Operand::Script(callee) = instruction.operand {
            if let Some(site) = requirements
                .get(&callee)
                .into_iter()
                .flatten()
                .find(|site| selection(site.requirement))
            {
                let consumer = &scripts[&callee].code[site.pc];
                let declared = declarations
                    .get(&callee)
                    .and_then(|rows| rows.values().next())
                    .cloned();
                let start = state.stacks[INTEGER_LANE]
                    .len()
                    .checked_sub(usize::from(scripts[&callee].args.int))
                    .context("selection call underflow")?;
                let slot = if matches!(
                    site.requirement,
                    Requirement::FlatChildSelection
                        | Requirement::ExplicitRuntimeChildSelection
                        | Requirement::FlatTextChildCreation
                ) {
                    start
                } else {
                    state.stacks[INTEGER_LANE].len() - usize::from(true)
                };
                declared
                    .or_else(|| {
                        component_binding(
                            &state.stacks[INTEGER_LANE][slot],
                            components,
                            declarations.get(&id),
                        )
                        .cloned()
                    })
                    .map(|component| {
                        (
                            selector_bank(consumer),
                            component,
                            site.requirement == Requirement::FlatTextChildCreation,
                        )
                    })
            } else {
                None
            }
        } else {
            None
        };
        if let Some((bank, component, conditional)) = site {
            selections.insert(pc, (bank?, component, conditional));
        } else if direct.is_some() {
            anyhow::bail!("procedure {id} @{pc} selection has no named owner");
        }
    }
    let mut before: Vec<Option<ActiveOwners>> = vec![None; script.code.len()];
    if before.is_empty() {
        return Ok(before);
    }
    before[usize::default()] = Some(incoming);
    let mut pending = VecDeque::from([usize::default()]);
    while let Some(pc) = pending.pop_front() {
        if analysis.before[pc].is_none() {
            continue;
        }
        let mut owners = before[pc]
            .clone()
            .context("component ownership path is missing")?;
        if let Some((bank, component, conditional)) = selections.get(&pc) {
            ensure!(
                owners[*bank].as_ref().is_none_or(|old| old == component),
                "procedure {id} @{pc} changes an inherited active owner without a modeled call effect"
            );
            // Creation can preserve the prior reference. Keep a named owner
            // only when both that path and successful construction agree.
            owners[*bank] = if *conditional && owners[*bank].as_ref() != Some(component) {
                None
            } else {
                Some(component.clone())
            };
        }
        for next in analysis.successors(script, pc) {
            if analysis.before[next].is_none() {
                continue;
            }
            let merged = match &before[next] {
                None => owners.clone(),
                Some(old) => std::array::from_fn(|bank| {
                    if old[bank] == owners[bank] {
                        old[bank].clone()
                    } else {
                        None
                    }
                }),
            };
            if before[next].as_ref() != Some(&merged) {
                before[next] = Some(merged);
                pending.push_back(next);
            }
        }
    }
    Ok(before)
}

struct CallbackInput<'a> {
    instruction: &'a Instruction,
    state: &'a dataflow::State,
    scripts: &'a BTreeMap<i32, CompiledScript>,
    configs: &'a ConfigTypes,
    policy: &'a crate::lower910::CallbackPolicy,
    components: &'a ComponentArguments,
    declarations: Option<&'a BTreeMap<usize, ComponentBinding>>,
    operation: Option<native910::execution::HostOperation>,
    owner: Option<&'a ComponentBinding>,
    options: &'a dataflow::AnalysisOptions,
}

fn callback_context(input: CallbackInput<'_>) -> Result<Option<BoundContext>> {
    use native910::{dataflow::Constant, semantics::Effect};
    let CallbackInput {
        instruction,
        state,
        scripts,
        configs,
        policy,
        components,
        declarations,
        operation,
        owner,
        options,
    } = input;
    let variable = matches!(
        operation,
        Some(native910::execution::HostOperation::RetainedPlayerTransmit { .. })
    );
    let effect = operation.map_or_else(
        || dataflow::resolved_effect_with_options(instruction, state, configs, options),
        |operation| operation.effect(&instruction.operand),
    );
    let Effect::Fixed { pops, .. } = effect else {
        anyhow::bail!("target callback traffic is unresolved");
    };
    let mut starts = [usize::default(); 3];
    for (lane, count) in pops.iter().enumerate() {
        starts[lane] = state.stacks[lane]
            .len()
            .checked_sub(usize::from(*count))
            .context("callback argument underflow")?;
    }
    let Some(Constant::Int(callee)) = state.stacks[INTEGER_LANE][starts[INTEGER_LANE]].constant
    else {
        anyhow::bail!("callback head is unlinked");
    };
    if callee == policy.clear_script_id {
        return Ok(None);
    }
    let script = scripts.get(&callee).context("unlinked callback body")?;
    let counts = [script.args.int, script.args.obj, script.args.long];
    let integer_overhead = if variable {
        let count = state
            .int_from_top(usize::default())
            .context("callback trigger count is unresolved")?;
        ensure!(
            count > i32::default(),
            "retained callback trigger replacement is unresolved"
        );
        u16::try_from(count)?
            .checked_add(u16::from(true) + u16::from(true))
            .context("callback trigger count overflow")?
    } else {
        u16::from(true) + u16::from(true)
    };
    let installed = [
        pops[INTEGER_LANE].checked_sub(integer_overhead),
        pops[OBJECT_LANE].checked_sub(u16::from(true)),
        Some(pops[LONG_LANE]),
    ];
    ensure!(
        installed
            .into_iter()
            .zip(counts)
            .all(|(installed, count)| installed == Some(count)),
        "callback signature does not populate the declared arguments"
    );
    starts[INTEGER_LANE] += usize::from(true);
    let mut arguments = EntryContext::default();
    for lane in [INTEGER_LANE, OBJECT_LANE, LONG_LANE] {
        let values = &state.stacks[lane][starts[lane]..starts[lane] + usize::from(counts[lane])];
        for value in values {
            match lane {
                INTEGER_LANE if !variable => ensure!(
                    matches!(value.constant, Some(Constant::Int(value))
                    if !policy.int_event_tokens.contains(&value)),
                    "callback integer requires a proven value without an event substitution"
                ),
                OBJECT_LANE => ensure!(
                    value.nonnull_string
                        && matches!(&value.constant,
                    Some(Constant::String(value)) if !policy.string_event_tokens.contains(value)),
                    "callback object requires a proven string without an event substitution"
                ),
                _ => {}
            }
        }
        arguments.arguments[lane] = values.iter().map(|value| value.constant.clone()).collect();
        if lane == OBJECT_LANE {
            arguments.nonnull_strings = values.iter().map(|value| value.nonnull_string).collect();
        }
    }
    let mut components: ComponentArguments = state.stacks[INTEGER_LANE]
        [starts[INTEGER_LANE]..starts[INTEGER_LANE] + usize::from(counts[INTEGER_LANE])]
        .iter()
        .map(|value| component_binding(value, components, declarations).cloned())
        .collect();
    if variable {
        const PARENT_EVENT: usize = 2;
        const CHILD_EVENT: usize = 4;
        const MISSING_EVENT_FIELDS: [usize; 2] = [5, 6];
        let owner = owner.context("variable callback has no named frame owner")?;
        for (slot, value) in arguments.arguments[INTEGER_LANE].iter_mut().enumerate() {
            let Some(Constant::Int(mut integer)) = value.clone() else {
                continue;
            };
            for (event, token) in policy.int_event_tokens.iter().enumerate() {
                if integer != *token {
                    continue;
                }
                if event == CHILD_EVENT {
                    *value = None;
                    components[slot] = None;
                    break;
                }
                integer = if event == PARENT_EVENT {
                    owner.packed
                } else if MISSING_EVENT_FIELDS.contains(&event) {
                    -i32::from(true)
                } else {
                    i32::default()
                };
                components[slot] = (event == PARENT_EVENT).then(|| owner.clone());
                *value = Some(Constant::Int(integer));
            }
        }
    }
    // Callback execution acquires its own state; its selection commands establish
    // active owners from the component roles projected into event arguments.
    Ok(Some((
        callee,
        arguments,
        components,
        ActiveOwners::default(),
    )))
}

struct Prepared<'a> {
    sources: &'a BTreeMap<i32, String>,
    curated: &'a [(i32, String)],
    scripts: &'a BTreeMap<i32, CompiledScript>,
    plan: &'a Plan,
    inames: &'a str,
    report: &'a Report,
    book: &'a OpcodeBook,
    execution: &'a ExecutionTable,
}

impl Prepared<'_> {
    fn publish(self, pack_root: &Path, output: &Path) -> Result<()> {
        let Self {
            sources,
            curated,
            scripts,
            plan,
            inames,
            report,
            book,
            execution,
        } = self;
        use std::fmt::Write;
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".cs2-import-{}", std::process::id()));
        std::fs::create_dir(&staging)?;
        let result = (|| {
            let mut manifest = format!(
                "base-sha256 {}\ninterfaces-sha256 {}\ninames inames.txt\nexecution execution.tsv\n",
                plan.base_scripts_sha256, plan.base_interfaces_sha256
            );
            for (id, name) in curated {
                writeln!(manifest, "script {id} {name} {id}.rs2")?;
                std::fs::write(staging.join(format!("{id}.rs2")), &sources[id])?;
            }
            std::fs::write(staging.join("inames.txt"), inames)?;
            std::fs::write(staging.join("project.txt"), manifest)?;
            std::fs::write(
                staging.join(native910::execution::PROJECT_FILENAME),
                execution.emit(),
            )?;
            let build = staging.join("build");
            native910::project::build(&staging.join("project.txt"), pack_root, &build)?;
            let archive = PackArchive::open(&build.join("client.scripts.js5"))?;
            for (id, script) in scripts {
                ensure!(
                    archive
                        .group_files(u32::try_from(*id)?)?
                        .context("published procedure missing")?[&SCRIPT_FILE]
                        == native910::script::encode_script(script, book)?,
                    "project changed imported procedure {id}"
                );
            }
            std::fs::write(
                build.join("import.json"),
                serde_json::to_vec_pretty(report)?,
            )?;
            std::fs::write(
                build.join("import-plan.json"),
                serde_json::to_vec_pretty(plan)?,
            )?;
            ensure!(!output.exists(), "import output appeared during build");
            std::fs::rename(build, output)?;
            Ok(())
        })();
        let cleanup = std::fs::remove_dir_all(&staging);
        result?;
        cleanup?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn verify_callback_fixture(
    scripts: &BTreeMap<i32, CompiledScript>,
    requirements: &BTreeMap<i32, Vec<HostRequirement>>,
    declarations: &ComponentDeclarations,
    policy: &crate::lower910::CallbackPolicy,
) -> Result<()> {
    verify_bindings(BindingInput {
        scripts,
        requirements,
        declarations,
        configs: &ConfigTypes::empty(),
        callback_policy: Some(policy),
        options: &dataflow::AnalysisOptions::default(),
        execution: None,
        exports: &BTreeMap::from([(
            "fixture".into(),
            *scripts.keys().next().context("fixture root absent")?,
        )]),
    })
    .map(|_| ())
}
