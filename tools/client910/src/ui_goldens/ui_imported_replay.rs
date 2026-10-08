//! The ordinary authored-UI replay also exercises revision lowering. Its
//! default donor is independently authored; an explicit local case can select
//! a real archived donor without committing game scripts as test fixtures.
use cs2::{
    import910::{self, BuildInput, ComponentUse, Entry, IntArgument, Plan},
    profile::Book,
    wire,
};
use native910::{
    interface::{HookArg, InterfaceComponent},
    isource, project,
    xref::{pack_component, ComponentRef},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const IMPORT_PLAN_FORMAT: u32 = 1;
const COMPONENT_ARGUMENT: i32 = 0;
const SELECTOR_ARGUMENT: i32 = 1;
const INT_ARGUMENTS: u16 = 3;
const SELECTOR_ARGUMENTS: u16 = 2;
const OPERATION_FRAME_SLOT: i32 = 2;
const ACTIVE_TEXT_TARGET: usize = 4;
const INACTIVE_TEXT_TARGET: usize = 10;
const OPERATION_TARGET: usize = 15;
const BRANCH_ADVANCE: i32 = 1;
const COMPARISON_PC: usize = 2;
const ELSE_BRANCH_PC: usize = 3;
const EXIT_BRANCH_PC: usize = 9;
const ACTIVE_EXPORT: &str = "modern_status_active";
const INACTIVE_EXPORT: &str = "modern_status_inactive";
const LABEL_SYMBOL: &str = "workshop/label";
const FRAME_SYMBOL: &str = "workshop/frame";
const FIRST_OPERATION: i32 = 1;
const OPERATION_PARTS: i32 = 3;
const OPERATION_PREFIX: &str = "Open ";
const OPERATION_SUFFIX: &str = "!";
const ACTIVE_STRING_ARGUMENT: i32 = 0;
const INACTIVE_STRING_ARGUMENT: i32 = 1;
const STRING_ARGUMENTS: u16 = 2;
const LONG_ARGUMENTS: u16 = 1;
const BOUND_LONG_ARGUMENT: i64 = 1;
const STRING_LOCAL_SLOT: i32 = 2;
const STRING_LOCALS: u16 = 3;
const NEXT_SOURCE_ID: u32 = 1;
const CALLBACK_INT_ARGUMENTS: u16 = 2;
const CALLBACK_STRING_ARGUMENTS: u16 = 1;
const CALLBACK_FRAME_ARGUMENT: i32 = 1;
const DONOR_OPERATION_COMPONENT: i32 = i32::MAX;
const CALLBACK_TEXT: &str = "Imported callback fired";
const AUTHORED_TEXT_ENUM_OFFSET: i32 = 2;
const UNBOUND_FONT_IDENTITY_ERROR: &str = "font identity is unbound";
const UNMAPPED_FONT_ERROR: &str = "no reviewed asset mapping";
const FONT_READER_VALUES: usize = 2;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RootComponent {
    #[default]
    Label,
    Frame,
}
impl RootComponent {
    fn symbol(self) -> &'static str {
        match self {
            Self::Label => LABEL_SYMBOL,
            Self::Frame => FRAME_SYMBOL,
        }
    }
    fn binding_error(self) -> &'static str {
        match self {
            Self::Label => "bound plain-text component",
            Self::Frame => "bound component",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalCase {
    cache_root: PathBuf,
    procedure: u32,
    source_sha256: String,
    active_text: String,
    inactive_text: String,
    #[serde(default)]
    root_component: RootComponent,
    callback: Option<CallbackCase>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackCase {
    procedure: u32,
    source_sha256: String,
    entry_steps: usize,
    steps: usize,
    text: String,
    operation_label: String,
    arguments: Vec<CallbackArgument>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum CallbackArgument {
    Label,
    Frame,
    Text(String),
}

pub(super) struct Frame<'a> {
    pub pack_root: &'a Path,
    pub directory: &'a Path,
    pub group: u32,
    pub root_file: u32,
    pub root: InterfaceComponent,
    pub label_file: u32,
    pub label: InterfaceComponent,
    pub setup_script_id: u32,
}

pub(super) struct Imported {
    pub pack_root: PathBuf,
    pub active_text: String,
    pub inactive_text: String,
    pub operation_labels: Option<[String; 2]>,
    pub callback: Option<CallbackReplay>,
    pub retained_long_stack: bool,
    pub palette: PaletteReplay,
    pub flat_child_script: i32,
    pub retained_hook_script: i32,
}

pub(super) struct CallbackReplay {
    pub script: i32,
    pub before_script: i32,
    pub text: String,
    pub arguments: Vec<crate::ui_components::Arg>,
    pub entry_steps: usize,
    pub steps: usize,
}

pub(super) fn prepare(mut frame: Frame<'_>) -> anyhow::Result<Imported> {
    use anyhow::{ensure, Context};
    let book = Book::parse(include_bytes!("../../../../revisions/950/cs2/950-1.json"))?;
    let adapter = include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json");
    replay_database_scalars(&book, frame.setup_script_id)?;
    replay_component_paint(&book, frame.setup_script_id)?;
    replay_enum_queries(&book, frame.setup_script_id)?;
    let (
        source_id,
        scripts,
        active_text,
        inactive_text,
        operation_labels,
        root_component,
        callback,
    ) = if let Some(path) = std::env::var_os("ALTO_MODERN_UI_CASE") {
        let case: LocalCase = serde_json::from_slice(&std::fs::read(path)?)?;
        let scripts =
            import910::load_closure(&case.cache_root, &book, std::iter::once(case.procedure))?;
        ensure!(
            cs2::profile::digest(&scripts[&case.procedure]) == case.source_sha256,
            "local donor case bytes changed"
        );
        if let Some(callback) = &case.callback {
            let bytes = scripts
                .get(&callback.procedure)
                .context("callback absent from linked closure")?;
            ensure!(
                cs2::profile::digest(bytes) == callback.source_sha256,
                "local callback bytes changed"
            );
        }
        let operations = case.callback.as_ref().map(|callback| {
            [
                callback.operation_label.clone(),
                callback.operation_label.clone(),
            ]
        });
        (
            case.procedure,
            scripts,
            case.active_text,
            case.inactive_text,
            operations,
            case.root_component,
            case.callback,
        )
    } else {
        const ENTRY_STEPS: usize = 67;
        const CALLBACK_STEPS: usize = 30;
        let active = "Imported active".to_string();
        let inactive = "Imported inactive".to_string();
        let scripts = authored_donor(&book, frame.setup_script_id, &frame.label)?;
        let operations = Some([
            format!("{OPERATION_PREFIX}{active}{OPERATION_SUFFIX}"),
            format!("{OPERATION_PREFIX}{inactive}{OPERATION_SUFFIX}"),
        ]);
        let callback_id = frame.setup_script_id + NEXT_SOURCE_ID;
        let callback = CallbackCase {
            procedure: callback_id,
            source_sha256: cs2::profile::digest(&scripts[&callback_id]),
            entry_steps: ENTRY_STEPS,
            steps: CALLBACK_STEPS,
            text: CALLBACK_TEXT.into(),
            operation_label: operations.as_ref().unwrap()[0].clone(),
            arguments: vec![
                CallbackArgument::Label,
                CallbackArgument::Frame,
                CallbackArgument::Text(CALLBACK_TEXT.into()),
            ],
        };
        (
            frame.setup_script_id,
            scripts,
            active,
            inactive,
            operations,
            RootComponent::Label,
            Some(callback),
        )
    };
    let database = if std::env::var_os("ALTO_MODERN_UI_CASE").is_none() {
        Some(authored_database(&book)?)
    } else {
        None
    };
    let enums = if std::env::var_os("ALTO_MODERN_UI_CASE").is_none() {
        Some(authored_enums(&book, &frame.label)?)
    } else {
        None
    };
    let root_args = wire::decode(&scripts[&source_id], &book)?.args;
    ensure!(
        root_args.int >= 1
            && root_args.int <= INT_ARGUMENTS
            && root_args.object <= STRING_ARGUMENTS
            && root_args.long <= LONG_ARGUMENTS,
        "UI case needs a bound component, optional selector and operation component, at most two strings and one long"
    );
    let inames = std::fs::read_to_string(frame.pack_root.join("inames.txt"))?;
    let base_scripts = std::fs::read(frame.pack_root.join("client.scripts.js5"))?;
    let component_uses = if callback.is_some() {
        let normalized = cs2::semantic::normalize(&scripts[&source_id], &book)?;
        normalized
            .instructions
            .iter()
            .enumerate()
            .filter_map(|(pc, instruction)| {
                instruction
                    .operation
                    .as_ref()
                    .filter(|operation| operation.command == "if_setonop")
                    .map(|_| ComponentUse {
                        procedure: source_id,
                        instruction: pc,
                        component: FRAME_SYMBOL.into(),
                    })
            })
            .collect()
    } else {
        vec![]
    };
    let plan = Plan {
        font_uses: Vec::new(),
        font_maps: Vec::new(),
        initial_fonts: Vec::new(),
        frames: None,
        variables: None,
        enums: enums
            .as_ref()
            .map(cs2::enums::Definitions::identity)
            .transpose()?,
        database: database
            .as_ref()
            .map(cs2::database::Definitions::identity)
            .transpose()?,
        component_uses,
        format: IMPORT_PLAN_FORMAT,
        source_build: book.profile.build,
        source_client_md5: book.profile.client_md5.clone(),
        source_script_index_sha256: book.profile.script_index_sha256.clone(),
        source_scripts_sha256: scripts
            .iter()
            .map(|(id, bytes)| (*id, cs2::profile::digest(bytes)))
            .collect(),
        base_scripts_sha256: project::sha256(&base_scripts),
        base_interfaces_sha256: project::sha256(&std::fs::read(
            frame.pack_root.join("client.interfaces.js5"),
        )?),
        procedures: scripts
            .keys()
            .map(|id| {
                (
                    *id,
                    if *id == source_id {
                        "modern_status_body".to_string()
                    } else {
                        format!("modern_status_dependency_{id}")
                    },
                )
            })
            .collect(),
        entries: [(ACTIVE_EXPORT, true), (INACTIVE_EXPORT, false)]
            .into_iter()
            .map(|(name, active)| {
                let mut integers = vec![IntArgument::Component(root_component.symbol().into())];
                if root_args.int >= SELECTOR_ARGUMENTS {
                    integers.push(IntArgument::Value(i32::from(active)));
                }
                if root_args.int == INT_ARGUMENTS {
                    integers.push(IntArgument::Component(FRAME_SYMBOL.into()));
                }
                let strings = if root_args.object == STRING_ARGUMENTS {
                    vec![Some(active_text.clone()), Some(inactive_text.clone())]
                } else if root_args.object > 0 {
                    vec![Some(if active {
                        active_text.clone()
                    } else {
                        inactive_text.clone()
                    })]
                } else {
                    vec![]
                };
                Entry {
                    procedure: source_id,
                    name: name.into(),
                    int_arguments: integers,
                    string_arguments: strings,
                    long_arguments: vec![Some(BOUND_LONG_ARGUMENT); usize::from(root_args.long)],
                    active_component: None,
                }
            })
            .collect(),
    };
    let imported = frame.directory.join("modern-import");
    let report = import910::build(
        BuildInput {
            frames: None,
            variables: None,
            enums: enums.as_ref(),
            database: database.as_ref(),
            scripts: &scripts,
            book: &book,
            adapter_bytes: adapter,
            plan: &plan,
            pack_root: frame.pack_root,
            inames_text: &inames,
        },
        &imported,
    )?;
    if let Some(enums) = &enums {
        let mut changed = plan.clone();
        changed.enums = None;
        let rejected = frame.directory.join("modern-enum-without-identity");
        let error = import910::build(
            BuildInput {
                frames: None,
                variables: None,
                enums: Some(enums),
                database: database.as_ref(),
                scripts: &scripts,
                book: &book,
                adapter_bytes: adapter,
                plan: &changed,
                pack_root: frame.pack_root,
                inames_text: &inames,
            },
            &rejected,
        )
        .err()
        .context("enum resource identity was silently inferred")?;
        assert!(error
            .to_string()
            .contains("plan enum resource identity changed or is missing"));
        assert!(!rejected.exists());
        let enum_bridges = report
            .bridges
            .iter()
            .filter(|bridge| {
                bridge
                    .host_operation
                    .as_deref()
                    .is_some_and(|operation| operation.starts_with("enum/"))
            })
            .collect::<Vec<_>>();
        assert!(!enum_bridges.is_empty());
        for bridge in enum_bridges {
            assert_eq!(
                bridge.resource_sha256.as_ref(),
                Some(&enums.identity()?.resource_sha256)
            );
        }
    }
    complete_pack(&imported, frame.pack_root)?;
    if matches!(root_component, RootComponent::Label) {
        assert!(
            !report.text_bindings.is_empty(),
            "exported entries have verified text bindings"
        );
    }
    let uses_slot_query = scripts.values().any(|bytes| {
        wire::decode(bytes, &book)
            .unwrap()
            .code
            .iter()
            .any(|instruction| {
                book.opcode(instruction.opcode).unwrap().command.as_deref()
                    == Some("if_getnextcategorysubid")
            })
    });
    let uses_child_selection = scripts.values().any(|bytes| {
        wire::decode(bytes, &book)
            .unwrap()
            .code
            .iter()
            .any(|instruction| {
                book.opcode(instruction.opcode).unwrap().command.as_deref()
                    == Some("cc_findbycategory")
            })
    });
    let uses_component_selection = scripts.values().any(|bytes| {
        wire::decode(bytes, &book)
            .unwrap()
            .code
            .iter()
            .any(|instruction| {
                book.opcode(instruction.opcode).unwrap().command.as_deref() == Some("if_find")
            })
    });
    if uses_component_selection {
        assert!(report
            .component_selection_bindings
            .iter()
            .any(|use_site| use_site.component == FRAME_SYMBOL));
    }
    if uses_child_selection {
        assert!(report
            .child_selection_bindings
            .iter()
            .any(|use_site| use_site.component == FRAME_SYMBOL));
    }
    if uses_slot_query {
        assert!(report
            .child_slot_bindings
            .iter()
            .any(|use_site| use_site.component == FRAME_SYMBOL));
        let mut wrong_kind = plan.clone();
        let query_use = scripts
            .iter()
            .find_map(|(id, bytes)| {
                wire::decode(bytes, &book)
                    .unwrap()
                    .code
                    .iter()
                    .enumerate()
                    .find_map(|(pc, instruction)| {
                        (book.opcode(instruction.opcode).unwrap().command.as_deref()
                            == Some("if_getnextcategorysubid"))
                        .then_some(ComponentUse {
                            procedure: *id,
                            instruction: pc,
                            component: LABEL_SYMBOL.into(),
                        })
                    })
            })
            .unwrap();
        wrong_kind.component_uses.push(query_use);
        let rejected = frame.directory.join("modern-query-wrong-kind");
        let error = import910::build(
            BuildInput {
                frames: None,
                variables: None,
                enums: enums.as_ref(),
                database: database.as_ref(),
                scripts: &scripts,
                book: &book,
                adapter_bytes: adapter,
                plan: &wrong_kind,
                pack_root: frame.pack_root,
                inames_text: &inames,
            },
            &rejected,
        )
        .err()
        .context("text widget unexpectedly accepted as a query container")?;
        assert!(
            format!("{error:#}").contains("needs a bound container component"),
            "{error:#}"
        );
        assert!(!rejected.exists());
    }
    if callback.is_some() {
        assert!(report
            .child_removal_bindings
            .iter()
            .any(|use_site| use_site.component == FRAME_SYMBOL));
        assert!(report
            .bridges
            .iter()
            .any(|bridge| bridge.host_operation.as_deref() == Some("clear-runtime-children")));
    }
    if matches!(root_component, RootComponent::Label) && operation_labels.is_some() {
        assert!(report
            .operation_bindings
            .iter()
            .any(|use_site| use_site.component == FRAME_SYMBOL));
    }
    if matches!(root_component, RootComponent::Label) {
        // A layer accepts operation labels but cannot acquire text-widget capabilities.
        let mut wrong_kind = plan.clone();
        wrong_kind.entries[0].int_arguments[0] = IntArgument::Component(FRAME_SYMBOL.into());
        let rejected = frame.directory.join("modern-wrong-widget-kind");
        let error = import910::build(
            BuildInput {
                frames: None,
                variables: None,
                enums: enums.as_ref(),
                database: database.as_ref(),
                scripts: &scripts,
                book: &book,
                adapter_bytes: adapter,
                plan: &wrong_kind,
                pack_root: frame.pack_root,
                inames_text: &inames,
            },
            &rejected,
        )
        .err()
        .context("layer unexpectedly accepted as a text widget")?;
        assert!(
            format!("{error:#}").contains(root_component.binding_error()),
            "{error:#}"
        );
        assert!(!rejected.exists());
    }
    let old = native910::pack::PackArchive::from_bytes(base_scripts.clone())?;
    let published = native910::pack::PackArchive::open(&imported.join("client.scripts.js5"))?;
    if uses_child_selection || uses_component_selection {
        let native = native910::opcode::OpcodeBook::embedded()?;
        for bridge in report.bridges.iter().filter(|bridge| {
            bridge.host_operation.as_deref().is_some_and(|operation| {
                operation == "find-component" || operation.starts_with("find-runtime-child/")
            })
        }) {
            let files = published
                .group_files(u32::try_from(bridge.target)?)?
                .context("selection bridge absent")?;
            let target = native910::script::decode_script(
                files
                    .get(&native910::execution::SCRIPT_FILE)
                    .context("selection bridge bytes absent")?,
                &native,
            )?;
            let source = wire::decode(&scripts[&bridge.source], &book)?;
            let wire::Operand::Byte(selector) = source.code[bridge.instruction].operand else {
                unreachable!()
            };
            let consumer = target
                .code
                .iter()
                .find(|instruction| instruction.command == "if_find")
                .context("selection bridge consumer absent")?;
            assert_eq!(
                consumer.operand,
                native910::script::Operand::Byte(u8::from(
                    if bridge
                        .host_operation
                        .as_deref()
                        .is_some_and(|operation| operation.starts_with("find-runtime-child/"))
                    {
                        selector != u8::default()
                    } else {
                        selector == u8::from(true)
                    }
                ))
            );
        }
    }
    let validation = native910::validate::validate_scripts_pack(&imported)?;
    assert!(validation.is_clean(), "{:?}", validation.failures);
    for id in report
        .exports
        .values()
        .chain(report.procedures.iter().map(|procedure| &procedure.target))
        .chain(report.bridges.iter().map(|bridge| &bridge.target))
    {
        assert!(published
            .group_files(u32::try_from(*id)?)?
            .context("imported group missing")?
            .contains_key(&native910::execution::METADATA_FILE));
    }
    if let Some(adapter) = report
        .bridges
        .iter()
        .find(|bridge| {
            bridge
                .host_operation
                .as_deref()
                .is_some_and(|operation| operation.starts_with("enum/"))
        })
        .or_else(|| {
            report
                .bridges
                .iter()
                .find(|bridge| bridge.host_operation.is_some())
        })
    {
        let dumped = frame.directory.join("dumped-modern-adapter");
        native910::source::dump_selected_scripts_pack(
            &imported,
            &dumped,
            &[],
            &native910::inames::InterfaceRegistry::empty(),
            Some(adapter.target),
        )?;
        let metadata = std::fs::read(dumped.join(native910::execution::PROJECT_FILENAME))?;
        let files = published
            .group_files(u32::try_from(adapter.target)?)?
            .context("adapter group missing")?;
        assert_eq!(metadata, files[&native910::execution::METADATA_FILE]);
        let source_path = format!(
            "{}_{}.rs2",
            adapter.target,
            native910::execution::SCRIPT_FILE
        );
        let manifest = format!(
            "base-sha256 {}\nexecution {}\nscript {} {} {}\n",
            plan.base_scripts_sha256,
            native910::execution::PROJECT_FILENAME,
            adapter.target,
            adapter.name,
            source_path
        );
        std::fs::write(dumped.join("project.txt"), &manifest)?;
        let rebuilt = dumped.join("rebuilt");
        project::build(&dumped.join("project.txt"), frame.pack_root, &rebuilt)?;
        let rebuilt_archive =
            native910::pack::PackArchive::open(&rebuilt.join("client.scripts.js5"))?;
        assert_eq!(
            rebuilt_archive
                .group_files(u32::try_from(adapter.target)?)?
                .context("rebuilt adapter missing")?,
            files
        );
        let bytes = &files[&native910::execution::SCRIPT_FILE];
        assert!(native910::repack::scripts(
            &base_scripts,
            &BTreeMap::from([(u32::try_from(adapter.target)?, bytes.clone())])
        )
        .is_err());
        std::fs::write(
            dumped.join("project.txt"),
            manifest
                .lines()
                .filter(|line| !line.starts_with("execution "))
                .collect::<Vec<_>>()
                .join("\n"),
        )?;
        let rejected = dumped.join("without-metadata");
        let error = project::build(&dumped.join("project.txt"), frame.pack_root, &rejected)
            .err()
            .context("imported adapter rebuilt without metadata")?;
        assert!(
            error
                .to_string()
                .contains("execution metadata missing or changed"),
            "{error}"
        );
        assert!(!rejected.exists());
        let additional = *report.exports.values().next().context("export missing")?;
        for id in [additional, adapter.target] {
            native910::source::dump_selected_scripts_pack(
                &imported,
                &dumped,
                &[],
                &native910::inames::InterfaceRegistry::empty(),
                Some(id),
            )?;
        }
        let merged = native910::execution::Table::parse(&std::fs::read_to_string(
            dumped.join(native910::execution::PROJECT_FILENAME),
        )?)?;
        assert_eq!(merged.group_bytes(adapter.target), Some(metadata));
        assert_eq!(
            merged.group_bytes(additional),
            Some(
                published
                    .group_files(u32::try_from(additional)?)?
                    .context("export group missing")?[&native910::execution::METADATA_FILE]
                    .clone()
            )
        );
    }
    for id in old.group_ids() {
        assert_eq!(old.group_container(id), published.group_container(id));
    }
    assert_eq!(
        std::fs::read(frame.pack_root.join("client.scripts.js5"))?,
        base_scripts
    );
    assert!(report
        .procedures
        .iter()
        .all(|row| row.target as u32 > old.group_ids().max().unwrap()));
    let packed = pack_component(ComponentRef {
        iface: i32::try_from(frame.group)?,
        child: i32::try_from(match root_component {
            RootComponent::Label => frame.label_file,
            RootComponent::Frame => frame.root_file,
        })?,
    })
    .context("bound component address out of range")?;
    // A raw integer equal to a packed ID is still not a declared frame binding.
    let mut unbound = plan.clone();
    for entry in &mut unbound.entries {
        entry.int_arguments[0] = IntArgument::Value(packed);
    }
    let rejected = frame.directory.join("modern-unbound");
    let error = import910::build(
        BuildInput {
            frames: None,
            variables: None,
            enums: enums.as_ref(),
            database: database.as_ref(),
            scripts: &scripts,
            book: &book,
            adapter_bytes: adapter,
            plan: &unbound,
            pack_root: frame.pack_root,
            inames_text: &inames,
        },
        &rejected,
    )
    .err()
    .context("unbound component unexpectedly accepted")?;
    assert!(
        format!("{error:#}").contains(root_component.binding_error()),
        "{error:#}"
    );
    assert!(!rejected.exists());
    if root_args.object > 0 {
        let mut unbound = plan.clone();
        unbound.entries[0].string_arguments[0] = None;
        let rejected = frame.directory.join("modern-unbound-string");
        let error = import910::build(
            BuildInput {
                frames: None,
                variables: None,
                enums: enums.as_ref(),
                database: database.as_ref(),
                scripts: &scripts,
                book: &book,
                adapter_bytes: adapter,
                plan: &unbound,
                pack_root: frame.pack_root,
                inames_text: &inames,
            },
            &rejected,
        )
        .err()
        .context("unbound string argument unexpectedly accepted")?;
        assert!(
            error.to_string().contains("unbound object argument"),
            "{error:#}"
        );
        assert!(!rejected.exists());
        let mut unrepresentable = plan.clone();
        unrepresentable.entries[0].string_arguments[0] = Some("unrepresentable snowman ☃".into());
        let rejected = frame.directory.join("modern-unrepresentable-string");
        let error = import910::build(
            BuildInput {
                frames: None,
                variables: None,
                enums: enums.as_ref(),
                database: database.as_ref(),
                scripts: &scripts,
                book: &book,
                adapter_bytes: adapter,
                plan: &unrepresentable,
                pack_root: frame.pack_root,
                inames_text: &inames,
            },
            &rejected,
        )
        .err()
        .context("unrepresentable string was accepted")?;
        assert!(error.to_string().contains("target codec"), "{error:#}");
        assert!(!rejected.exists());
    }
    // A donor literal cannot borrow a role from an unused bound argument.
    let mut collision = wire::decode(&scripts[&source_id], &book)?;
    let constant_opcode = book
        .profile
        .opcodes
        .iter()
        .find(|row| row.command.as_deref() == Some("push_constant"))
        .context("missing constant opcode")?
        .id;
    let local_opcode = book
        .profile
        .opcodes
        .iter()
        .find(|row| row.command.as_deref() == Some("push_int_local"))
        .context("missing local opcode")?
        .id;
    let mut replacements = 0;
    for instruction in &mut collision.code {
        if instruction.opcode == local_opcode
            && instruction.operand == wire::Operand::Int(COMPONENT_ARGUMENT)
        {
            instruction.opcode = constant_opcode;
            instruction.operand = wire::Operand::ConstantInt(packed);
            replacements += 1;
        }
    }
    ensure!(
        replacements > 0,
        "donor case lacks a component argument read"
    );
    let mut collision_scripts = scripts.clone();
    let collision_bytes = wire::encode(&collision, &book)?;
    let mut collision_plan = plan.clone();
    collision_plan
        .source_scripts_sha256
        .insert(source_id, cs2::profile::digest(&collision_bytes));
    collision_scripts.insert(source_id, collision_bytes);
    let rejected = frame.directory.join("modern-collision");
    let error = import910::build(
        BuildInput {
            frames: None,
            variables: None,
            enums: enums.as_ref(),
            database: database.as_ref(),
            scripts: &collision_scripts,
            book: &book,
            adapter_bytes: adapter,
            plan: &collision_plan,
            pack_root: frame.pack_root,
            inames_text: &inames,
        },
        &rejected,
    )
    .err()
    .context("unmapped donor literal unexpectedly accepted")?;
    assert!(
        format!("{error:#}").contains(root_component.binding_error()),
        "{error:#}"
    );
    assert!(!rejected.exists());
    frame.root.hooks.onload = Some(vec![HookArg::Int(i32::try_from(frame.setup_script_id)?)]);
    if let Some(callback) = &callback {
        frame.root.ops = vec![callback.operation_label.clone()];
    }
    frame.label.hooks.onload = Some(vec![HookArg::Int(report.exports[ACTIVE_EXPORT])]);
    frame.label.hooks.onclick = Some(vec![HookArg::Int(report.exports[INACTIVE_EXPORT])]);
    let manifest = frame.directory.join("modern-project.txt");
    std::fs::write(
        &manifest,
        format!(
            "base-sha256 {}\ninterfaces-sha256 {}\ncomponent {} {} modern-label.ifc\ncomponent {} {} modern-frame.ifc\n",
            project::sha256(&std::fs::read(imported.join("client.scripts.js5"))?),
            project::sha256(&std::fs::read(imported.join("client.interfaces.js5"))?),
            frame.group,
            frame.label_file,
            frame.group,
            frame.root_file
        ),
    )?;
    std::fs::write(
        frame.directory.join("modern-label.ifc"),
        isource::format_component(&isource::lift_component(&frame.label), &Default::default()),
    )?;
    std::fs::write(
        frame.directory.join("modern-frame.ifc"),
        isource::format_component(&isource::lift_component(&frame.root), &Default::default()),
    )?;
    let final_pack = frame.directory.join("modern-ui-build");
    project::build(&manifest, &imported, &final_pack)?;
    complete_pack(&final_pack, &imported)?;
    let rebuilt_scripts =
        native910::pack::PackArchive::open(&final_pack.join("client.scripts.js5"))?;
    for id in report.exports.values() {
        assert_eq!(
            published.group_container(u32::try_from(*id)?),
            rebuilt_scripts.group_container(u32::try_from(*id)?)
        );
    }
    eprintln!(
        "modern UI import evidence: {}",
        imported.join("import.json").display()
    );
    let palette = prepare_palette(&book, &frame, &final_pack)?;
    let flat_child_script =
        prepare_flat_child(&book, &frame, &frame.directory.join("colour-helper-style"))?;
    let retained_hook_script = prepare_retained_hook(&frame, flat_child_script, palette.player)?;
    Ok(Imported {
        palette,
        flat_child_script,
        retained_hook_script,
        pack_root: frame.directory.join("retained-hook-import"),
        active_text,
        inactive_text,
        operation_labels,
        retained_long_stack: root_args.long > 0,
        callback: callback.map(|callback| CallbackReplay {
            before_script: report.exports[INACTIVE_EXPORT],
            script: report
                .procedures
                .iter()
                .find(|procedure| procedure.source == callback.procedure)
                .expect("linked callback body")
                .target,
            text: callback.text,
            entry_steps: callback.entry_steps,
            steps: callback.steps,
            arguments: callback
                .arguments
                .into_iter()
                .map(|argument| match argument {
                    CallbackArgument::Label => crate::ui_components::Arg::Int(
                        pack_component(ComponentRef {
                            iface: frame.group as i32,
                            child: frame.label_file as i32,
                        })
                        .unwrap(),
                    ),
                    CallbackArgument::Frame => crate::ui_components::Arg::Int(
                        pack_component(ComponentRef {
                            iface: frame.group as i32,
                            child: frame.root_file as i32,
                        })
                        .unwrap(),
                    ),
                    CallbackArgument::Text(text) => {
                        crate::ui_components::Arg::String(text.encode_utf16().collect())
                    }
                })
                .collect(),
        }),
    })
}

fn complete_pack(output: &Path, base: &Path) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(base)? {
        let path = entry?.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        if !output.join(name).exists() && path.is_file() {
            std::os::unix::fs::symlink(&path, output.join(name))?;
        }
    }
    Ok(())
}

fn replay_component_paint(book: &Book, script_id: u32) -> anyhow::Result<()> {
    use crate::{
        ui_changes::Changes,
        ui_components::{Active, Component, Interface, ScriptHost, Store},
        ui_properties::{Context, State},
    };
    use native910::{
        execution::{decode_accounting, HostOperation, PaintProperty, Role, Specification, Table},
        opcode::OpcodeBook,
        script::{CompiledScript, Counts, Instruction, Operand},
        vm::{Programs, Session, Value, Vm},
    };
    use std::{cell::RefCell, rc::Rc};
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../cs2/fixtures/component-paint.json"))?;
    assert_eq!(serde_json::to_value(book.profile.build)?, fixture["build"]);
    assert_eq!(book.profile.client_md5, fixture["client_md5"]);
    let native = OpcodeBook::embedded()?;
    let adapter = cs2::lower910::Adapter::parse(
        include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json"),
        book,
        &native,
    )?;
    let id = i32::try_from(script_id)?;
    const VALUE_SLOT: u16 = 0;
    const COMPONENT_SLOT: u16 = 1;
    const ONE_INPUT: u16 = 1;
    const GROUP_SHIFT: u32 = u16::BITS;
    let rectangle_class: i64 = serde_json::from_value(fixture["rectangle_runtime_class"].clone())?;
    let non_rectangle: i32 = serde_json::from_value(fixture["non_rectangle_type"].clone())?;
    let rectangle_type = adapter
        .rules
        .iter()
        .flat_map(|rule| rule.operations.values())
        .find_map(|target| match target.host_operation {
            Some(HostOperation::ComponentPaint {
                property: PaintProperty::Fill { rectangle_type },
                ..
            }) => Some(rectangle_type),
            _ => None,
        })
        .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        let expected = &case["output"];
        let name = case["operation"].as_str().unwrap();
        let targets = adapter
            .rules
            .iter()
            .flat_map(|rule| rule.operations.values())
            .filter(|target| match target.host_operation {
                Some(HostOperation::ComponentPaint {
                    property: PaintProperty::Colour { .. },
                    ..
                }) => name == "colour",
                Some(HostOperation::ComponentPaint {
                    property: PaintProperty::Fill { .. },
                    ..
                }) => name == "fill",
                Some(HostOperation::ComponentPaint {
                    property: PaintProperty::Transparency,
                    ..
                }) => name == "transparency",
                _ => false,
            });
        for target in targets {
            let operation = target.host_operation.unwrap();
            assert_eq!(HostOperation::parse(&operation.spelling())?, operation);
            let HostOperation::ComponentPaint { explicit, .. } = operation else {
                unreachable!()
            };
            let group: i32 = serde_json::from_value(input["group"].clone())?;
            let file: u16 = serde_json::from_value(input["file"].clone())?;
            let packed = group.wrapping_shl(GROUP_SHIFT) | i32::from(file);
            // Reserved identities are observed through retained active references.
            if explicit && (group == i32::from(u16::MAX) || file == u16::MAX) {
                continue;
            }
            for secondary in [false, true] {
                let mut component = Component::default();
                component.f.parentlayer = packed;
                component.f.id = serde_json::from_value(input["child"].clone())?;
                component.f.r#type = if input["runtime_class"].as_i64() == Some(rectangle_class) {
                    rectangle_type
                } else {
                    non_rectangle
                };
                component.f.colour = serde_json::from_value(input["colour"].clone())?;
                let alpha: u8 = serde_json::from_value(input["alpha"].clone())?;
                component.f.trans = i32::from(u8::MAX - alpha);
                component.f.fill = serde_json::from_value(input["fill"].clone())?;
                let component = Rc::new(RefCell::new(component));
                let mut files = vec![None; usize::from(file) + usize::from(true)];
                let mut root = Component::default();
                root.f.parentlayer = group.wrapping_shl(GROUP_SHIFT);
                files[usize::default()] = Some(Rc::new(RefCell::new(root)));
                files[usize::from(file)] = Some(component.clone());
                let interface = Interface::new(files);
                interface.borrow_mut().transient =
                    serde_json::from_value(input["transient"].clone())?;
                let mut store = Store::default();
                store
                    .interfaces
                    .insert(packed >> GROUP_SHIFT, interface.clone());
                let mut active = [Active::default(), Active::default()];
                active[usize::from(secondary)] = Active {
                    interface: Some(interface),
                    component: Some(component.clone()),
                };
                let args = Counts {
                    int: ONE_INPUT + u16::from(explicit),
                    ..Counts::default()
                };
                let instruction = |command: &str, operand| -> anyhow::Result<Instruction> {
                    Ok(Instruction {
                        opcode: native.opcode_for(command)?,
                        command: command.into(),
                        operand,
                    })
                };
                let mut code = vec![instruction(
                    "push_int_local",
                    Operand::Local(i32::from(VALUE_SLOT)),
                )?];
                if explicit {
                    code.push(instruction(
                        "push_int_local",
                        Operand::Local(i32::from(COMPONENT_SLOT)),
                    )?);
                }
                let consumer = code.len();
                code.push(instruction(
                    &target.command,
                    Operand::Byte(u8::from(secondary)),
                )?);
                code.push(instruction("return", Operand::Byte(u8::default()))?);
                let mut script = CompiledScript {
                    name: Some("proc,recorded_component_paint".into()),
                    args,
                    locals: args,
                    code,
                };
                let mut execution = Table::default();
                let bytes = execution.bind_import(
                    id,
                    &mut script,
                    &native,
                    Specification {
                        role: Role::Adapter,
                        adapter_calls: BTreeMap::new(),
                        host_operations: BTreeMap::from([(consumer, operation)]),
                        resource: None,
                    },
                )?;
                let metadata = execution.group_bytes(id).unwrap();
                let accounting = decode_accounting(id, &bytes, Some(&metadata), &script)?;
                let programs = Programs {
                    scripts: std::collections::HashMap::from([(id, script.clone())]),
                    accounting: std::collections::HashMap::from([(id, accounting)]),
                };
                let mut engine = crate::ui_runtime::Engine::default();
                let mut state = State::default();
                let mut changes = Changes::default();
                let mut now = || i64::default();
                let mut host = ScriptHost {
                    engine: &mut engine,
                    store: &mut store,
                    active: &mut active,
                    properties: Some(Context {
                        state: &mut state,
                        changes: &mut changes,
                        now: &mut now,
                        nested_count: i32::default(),
                    }),
                };
                let arguments = [
                    Value::Int(serde_json::from_value(input["value"].clone())?),
                    Value::Int(packed),
                ];
                let mut session =
                    Session::for_script(id, &script, &arguments[..usize::from(args.int)], None)?;
                let mut vm = Vm::new(&mut host, &programs);
                while !vm.step(&mut session)? {}
                assert!(session.snapshot().ints.is_empty());
                let actual = component.borrow();
                assert_eq!(
                    serde_json::to_value(actual.f.colour)?,
                    expected["colour"],
                    "{}",
                    case["name"]
                );
                assert_eq!(
                    serde_json::to_value(u8::MAX - u8::try_from(actual.f.trans)?)?,
                    expected["alpha"],
                    "{}",
                    case["name"]
                );
                assert_eq!(
                    serde_json::to_value(actual.f.fill)?,
                    expected["fill"],
                    "{}",
                    case["name"]
                );
                assert!(store.updated.is_empty(), "{}", case["name"]);
                let observed: Vec<_> = changes
                    .cache
                    .values()
                    .flat_map(|change| {
                        [
                            serde_json::json!(["change", change.kind(), change.target() as i32]),
                            serde_json::json!(["schedule"]),
                        ]
                    })
                    .collect();
                assert_eq!(
                    serde_json::to_value(observed)?,
                    expected["calls"],
                    "{}",
                    case["name"]
                );
            }
        }
    }
    Ok(())
}

fn replay_database_scalars(book: &Book, script_id: u32) -> anyhow::Result<()> {
    use native910::{
        execution::{decode_accounting, HostOperation, Role, Specification, Table as Execution},
        opcode::OpcodeBook,
        script::{CompiledScript, Counts, Instruction, Operand},
        vm::{Programs, Session, Value, Vm},
    };
    use rs910_config::ui_db_schema::{Column, FieldValue, Row, RowColumn, Table};
    use std::sync::Arc;
    const FIELD_INPUTS: u16 = 3;
    const COUNT_INPUTS: u16 = 2;
    let native = OpcodeBook::embedded()?;
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../cs2/fixtures/database.json"))?;
    assert_eq!(fixture["client_md5"], book.profile.client_md5);
    let id = i32::try_from(script_id)?;
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        let row_id = i32::try_from(input["row"].as_i64().unwrap())?;
        let field = i32::try_from(case["packed_field"].as_i64().unwrap())?;
        let index = i32::try_from(input["index"].as_i64().unwrap())?;
        let definitions = cs2::database::Definitions::authored_fixture(
            book,
            include_bytes!("../../../../revisions/950/cs2/database.json"),
            |database| {
                let column = usize::try_from(input["column"].as_u64().unwrap())?;
                let types: Vec<u16> = serde_json::from_value(input["types"].clone())?;
                let decode = |value: &serde_json::Value| -> anyhow::Result<FieldValue> {
                    Ok(match value["kind"].as_str().unwrap() {
                        "int" => FieldValue::Int(serde_json::from_value(value["value"].clone())?),
                        "long" => FieldValue::Long(serde_json::from_value(value["value"].clone())?),
                        _ => anyhow::bail!("recording contains a non-scalar push"),
                    })
                };
                let mut table = Table::default();
                table
                    .columns
                    .resize_with(column + usize::from(true), Column::default);
                table.columns[column].types = types.clone();
                table.columns[column].defaults = input["defaults"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(decode)
                    .collect::<anyhow::Result<_>>()?;
                database
                    .tables
                    .insert(u32::try_from(input["table"].as_u64().unwrap())?, table);
                let mut row = Row::default();
                row.columns
                    .resize_with(column + usize::from(true), RowColumn::default);
                row.columns[column].values = input["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(decode)
                    .collect::<anyhow::Result<_>>()?;
                database.rows.insert(row_id, row);
                if let Some(index) = input["unknown_base"].as_u64() {
                    database.types.remove(&types[usize::try_from(index)?]);
                }
                Ok(())
            },
        )?;
        let resource: Arc<[u8]> = definitions.database().encode_resource()?.into();
        let mut engine = crate::ui_runtime::Engine::default();
        for (query, inputs) in [("field", FIELD_INPUTS), ("count", COUNT_INPUTS)] {
            let expected = &case[query];
            let operation = if query == "field" {
                HostOperation::DatabaseField {
                    field,
                    pushes: [
                        u16::try_from(expected["ints"].as_array().unwrap().len())?,
                        u16::default(),
                        u16::try_from(expected["longs"].as_array().unwrap().len())?,
                    ],
                }
            } else {
                HostOperation::DatabaseFieldCount { field }
            };
            let instruction = |command: &str, operand| -> anyhow::Result<Instruction> {
                Ok(Instruction {
                    opcode: native.opcode_for(command)?,
                    command: command.into(),
                    operand,
                })
            };
            let args = Counts {
                int: inputs,
                ..Counts::default()
            };
            let mut code = (0..inputs)
                .map(|slot| instruction("push_int_local", Operand::Local(i32::from(slot))))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let consumer = code.len();
            code.push(instruction(
                operation.command(),
                Operand::Byte(u8::default()),
            )?);
            code.push(instruction("return", Operand::Byte(u8::default()))?);
            let mut script = CompiledScript {
                name: Some("proc,recorded_database".into()),
                args,
                locals: args,
                code,
            };
            let mut execution = Execution::default();
            let bytes = execution.bind_import(
                id,
                &mut script,
                &native,
                Specification {
                    role: Role::Adapter,
                    resource: Some(resource.clone()),
                    adapter_calls: BTreeMap::new(),
                    host_operations: BTreeMap::from([(consumer, operation)]),
                },
            )?;
            let metadata = execution.group_bytes(id).unwrap();
            let accounting = decode_accounting(id, &bytes, Some(&metadata), &script)?;
            let stripped = execution
                .emit()
                .lines()
                .filter(|line| !line.starts_with("resource "))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(decode_accounting(id, &bytes, Some(stripped.as_bytes()), &script).is_err());
            let mut changed = execution.emit();
            let resource_end = changed.find("\nhost-operation").unwrap();
            let byte = &changed[resource_end - usize::from(true)..resource_end];
            let replacement = if byte == "0" { "1" } else { "0" };
            changed.replace_range(resource_end - usize::from(true)..resource_end, replacement);
            assert!(decode_accounting(id, &bytes, Some(changed.as_bytes()), &script).is_err());
            let programs = Programs {
                scripts: std::collections::HashMap::from([(id, script.clone())]),
                accounting: std::collections::HashMap::from([(id, accounting)]),
            };
            let arguments = [Value::Int(row_id), Value::Int(field), Value::Int(index)];
            let mut session =
                Session::for_script(id, &script, &arguments[..usize::from(inputs)], None)?;
            let mut vm = Vm::new(&mut engine, &programs);
            let failed = loop {
                match vm.step(&mut session) {
                    Ok(true) => break false,
                    Ok(false) => {}
                    Err(_) => break true,
                }
            };
            let snapshot = session.snapshot();
            assert_eq!(
                failed,
                expected["failed"].as_bool().unwrap(),
                "{} {query}",
                case["name"]
            );
            assert_eq!(
                serde_json::to_value(snapshot.ints)?,
                expected["ints"],
                "{} {query}",
                case["name"]
            );
            assert_eq!(
                serde_json::to_value(snapshot.longs)?,
                expected["longs"],
                "{} {query}",
                case["name"]
            );
            assert!(snapshot.strings.is_empty());
        }
        assert_eq!(engine.configs.imported_databases.len(), usize::from(true));
    }
    Ok(())
}

fn authored_database(book: &Book) -> anyhow::Result<cs2::database::Definitions> {
    use rs910_config::ui_db_schema::{BaseType, Column, FieldValue, Row, RowColumn, Table};
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../cs2/fixtures/database.json"))?;
    let input = &fixture["cases"][0]["input"];
    let table = u32::try_from(input["table"].as_u64().unwrap())?;
    let row = i32::try_from(input["row"].as_i64().unwrap())?;
    let column = usize::try_from(input["column"].as_u64().unwrap())?;
    cs2::database::Definitions::authored_fixture(
        book,
        include_bytes!("../../../../revisions/950/cs2/database.json"),
        |database| {
            let text_type = *database
                .types
                .iter()
                .find(|(_, kind)| kind.base == BaseType::Text)
                .unwrap()
                .0;
            let mut definition = Table::default();
            definition
                .columns
                .resize_with(column + usize::from(true), Column::default);
            definition.columns[column].types = vec![text_type];
            database.tables.insert(table, definition);
            let mut definition = Row::default();
            definition
                .columns
                .resize_with(column + usize::from(true), RowColumn::default);
            definition.columns[column].types = vec![text_type];
            definition.columns[column].values = vec![
                FieldValue::Text("Imported active".into()),
                FieldValue::Text("Imported inactive".into()),
            ];
            database.rows.insert(row, definition);
            Ok(())
        },
    )
}
fn authored_database_address(book: &Book) -> anyhow::Result<(i32, i32)> {
    let database = authored_database(book)?;
    let (table, definition) = database.database().tables.iter().next().unwrap();
    let column = definition
        .columns
        .iter()
        .position(|column| !column.types.is_empty())
        .unwrap();
    Ok((
        *database.database().rows.keys().next().unwrap(),
        database
            .database()
            .layout
            .pack(*table, column, usize::default())?,
    ))
}

fn authored_donor(
    book: &Book,
    source_id: u32,
    label: &InterfaceComponent,
) -> anyhow::Result<BTreeMap<u32, Vec<u8>>> {
    use anyhow::Context;
    const ROOT_CHILD_BANK: i32 = 0;
    const NEXT_FRAME_CHILD_SLOT: i32 = 1;
    const PRESENT_CHILD_SLOT: i32 = 0;
    const MISSING_CHILD_SLOT: i32 = 1;
    const COMPONENT_COMPARISON_PC: usize = 3;
    const PRESENT_COMPARISON_PC: usize = 9;
    const MISSING_COMPARISON_PC: usize = 15;
    const SOURCE_SECONDARY_SELECTOR: u8 = 1;
    const SOURCE_PRIMARY_SELECTOR: u8 = u8::MAX;
    const SLOT_QUERY_COMPARISON_PC: usize = 20;
    const CALLBACK_RETURN_PC: usize = 29;
    let instruction = |command: &str, operand| -> anyhow::Result<wire::Instruction> {
        Ok(wire::Instruction {
            opcode: book
                .profile
                .opcodes
                .iter()
                .find(|row| row.command.as_deref() == Some(command))
                .context("missing authored donor opcode")?
                .id,
            operand,
        })
    };
    let branch = |pc: usize, target: usize| -> anyhow::Result<wire::Operand> {
        Ok(wire::Operand::Int(
            i32::try_from(target)? - i32::try_from(pc)? - BRANCH_ADVANCE,
        ))
    };
    let counts = wire::Counts {
        int: INT_ARGUMENTS,
        object: STRING_ARGUMENTS,
        long: LONG_ARGUMENTS,
    };
    let callback_id = source_id + NEXT_SOURCE_ID;
    let source = wire::encode(
        &wire::Script {
            name: vec![],
            locals: wire::Counts {
                object: STRING_LOCALS,
                ..counts
            },
            args: counts,
            code: vec![
                instruction("push_int_local", wire::Operand::Int(SELECTOR_ARGUMENT))?,
                instruction("push_constant", wire::Operand::ConstantInt(i32::from(true)))?,
                instruction("branch_equals", branch(COMPARISON_PC, ACTIVE_TEXT_TARGET)?)?,
                instruction("branch", branch(ELSE_BRANCH_PC, INACTIVE_TEXT_TARGET)?)?,
                instruction(
                    "push_string_local",
                    wire::Operand::Int(ACTIVE_STRING_ARGUMENT),
                )?,
                instruction("pop_string_local", wire::Operand::Int(STRING_LOCAL_SLOT))?,
                instruction("push_string_local", wire::Operand::Int(STRING_LOCAL_SLOT))?,
                instruction("push_int_local", wire::Operand::Int(COMPONENT_ARGUMENT))?,
                instruction("if_settext", wire::Operand::Byte(u8::default()))?,
                instruction("branch", branch(EXIT_BRANCH_PC, OPERATION_TARGET)?)?,
                instruction(
                    "push_string_local",
                    wire::Operand::Int(INACTIVE_STRING_ARGUMENT),
                )?,
                instruction("pop_string_local", wire::Operand::Int(STRING_LOCAL_SLOT))?,
                instruction("push_string_local", wire::Operand::Int(STRING_LOCAL_SLOT))?,
                instruction("push_int_local", wire::Operand::Int(COMPONENT_ARGUMENT))?,
                instruction("if_settext", wire::Operand::Byte(u8::default()))?,
                instruction("push_constant", wire::Operand::ConstantInt(FIRST_OPERATION))?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantString(OPERATION_PREFIX.as_bytes().to_vec()),
                )?,
                instruction("push_string_local", wire::Operand::Int(STRING_LOCAL_SLOT))?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantString(OPERATION_SUFFIX.as_bytes().to_vec()),
                )?,
                instruction("join_string", wire::Operand::Int(OPERATION_PARTS))?,
                instruction("push_int_local", wire::Operand::Int(OPERATION_FRAME_SLOT))?,
                instruction("if_setop", wire::Operand::Byte(u8::default()))?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(i32::try_from(callback_id)?),
                )?,
                instruction("push_int_local", wire::Operand::Int(COMPONENT_ARGUMENT))?,
                instruction("push_int_local", wire::Operand::Int(OPERATION_FRAME_SLOT))?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantString(CALLBACK_TEXT.as_bytes().to_vec()),
                )?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantString(b"iis".to_vec()),
                )?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(DONOR_OPERATION_COMPONENT),
                )?,
                instruction("if_setonop", wire::Operand::Byte(u8::default()))?,
                instruction("return", wire::Operand::Byte(u8::default()))?,
            ],
            switches: vec![],
        },
        book,
    )?;
    let mut source = wire::decode(&source, book)?;
    let (row, field) = authored_database_address(book)?;
    let old = source.code;
    let mut code = Vec::new();
    let mut positions = Vec::new();
    for (pc, original) in old.iter().enumerate() {
        positions.push(code.len());
        if pc == ACTIVE_TEXT_TARGET || pc == INACTIVE_TEXT_TARGET {
            let index = i32::from(pc == INACTIVE_TEXT_TARGET);
            code.extend([
                instruction("push_constant", wire::Operand::ConstantInt(row))?,
                instruction("push_constant", wire::Operand::ConstantInt(field))?,
                instruction("db_getfieldcount", wire::Operand::Byte(u8::default()))?,
                instruction("pop_int_local", wire::Operand::Int(SELECTOR_ARGUMENT))?,
                instruction("push_constant", wire::Operand::ConstantInt(row))?,
                instruction("push_constant", wire::Operand::ConstantInt(field))?,
                instruction("push_constant", wire::Operand::ConstantInt(index))?,
                instruction("db_getfield", wire::Operand::Byte(u8::default()))?,
            ]);
            let (enum_id, input_type, _, key) = authored_enum_address()?;
            let string_type = authored_enums(book, label)?.library.string_type;
            code.extend([
                instruction("push_constant", wire::Operand::ConstantInt(input_type))?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(i32::from(string_type)),
                )?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(enum_id + AUTHORED_TEXT_ENUM_OFFSET),
                )?,
                instruction("push_constant", wire::Operand::ConstantInt(key))?,
                instruction("enum", wire::Operand::Byte(u8::default()))?,
                instruction(
                    "join_string",
                    wire::Operand::Int(i32::from(STRING_ARGUMENTS)),
                )?,
            ]);
        } else {
            code.push(original.clone());
        }
    }
    positions.push(code.len());
    for (old_pc, original) in old.iter().enumerate() {
        let row = book.opcode(original.opcode)?;
        if row
            .command
            .as_ref()
            .is_some_and(|command| native910::script::is_branch_command(command))
        {
            let wire::Operand::Int(delta) = original.operand else {
                anyhow::bail!("authored branch has no relative target");
            };
            let target = usize::try_from(i32::try_from(old_pc)? + delta + BRANCH_ADVANCE)?;
            code[positions[old_pc]].operand = branch(positions[old_pc], positions[target])?;
        }
    }
    let native910::interface::ComponentBody::Text { colour, trans, .. } = label.body else {
        anyhow::bail!("authored label is not text");
    };
    let final_return = code.pop().context("authored donor return absent")?;
    code.extend([
        instruction("push_int_local", wire::Operand::Int(COMPONENT_ARGUMENT))?,
        instruction("if_find", wire::Operand::Byte(SOURCE_PRIMARY_SELECTOR))?,
        instruction("pop_int_local", wire::Operand::Int(SELECTOR_ARGUMENT))?,
    ]);
    for (active, explicit, value) in [
        ("cc_setcolour", "if_setcolour", colour),
        ("cc_setfill", "if_setfill", i32::from(true)),
        (
            "cc_settrans",
            "if_settrans",
            i32::from(trans) + i32::from(u8::MAX) + i32::from(true),
        ),
    ] {
        for (command, explicit_target) in [(active, false), (explicit, true)] {
            if command.ends_with("setcolour") {
                let (enum_id, input_type, output_type, key) = authored_enum_address()?;
                code.extend([
                    instruction("push_constant", wire::Operand::ConstantInt(input_type))?,
                    instruction("push_constant", wire::Operand::ConstantInt(output_type))?,
                    instruction("push_constant", wire::Operand::ConstantInt(enum_id))?,
                    instruction("push_int_local", wire::Operand::Int(SELECTOR_ARGUMENT))?,
                    instruction("add", wire::Operand::Byte(u8::default()))?,
                    instruction("push_constant", wire::Operand::ConstantInt(key))?,
                    instruction("enum", wire::Operand::Byte(u8::default()))?,
                ]);
            } else {
                code.push(instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(value),
                )?);
            }
            if explicit_target {
                code.push(instruction(
                    "push_int_local",
                    wire::Operand::Int(COMPONENT_ARGUMENT),
                )?);
            }
            code.push(instruction(
                command,
                wire::Operand::Byte(if explicit_target {
                    u8::default()
                } else {
                    SOURCE_PRIMARY_SELECTOR
                }),
            )?);
        }
    }
    code.push(final_return);
    source.code = code;
    let source = wire::encode(&source, book)?;
    let counts = wire::Counts {
        int: CALLBACK_INT_ARGUMENTS,
        object: CALLBACK_STRING_ARGUMENTS,
        ..Default::default()
    };
    let callback = wire::encode(
        &wire::Script {
            name: vec![],
            locals: counts,
            args: counts,
            switches: vec![],
            code: vec![
                instruction(
                    "push_int_local",
                    wire::Operand::Int(CALLBACK_FRAME_ARGUMENT),
                )?,
                instruction("if_find", wire::Operand::Byte(SOURCE_PRIMARY_SELECTOR))?,
                instruction("push_constant", wire::Operand::ConstantInt(i32::from(true)))?,
                instruction(
                    "branch_not",
                    branch(COMPONENT_COMPARISON_PC, CALLBACK_RETURN_PC)?,
                )?,
                instruction(
                    "push_int_local",
                    wire::Operand::Int(CALLBACK_FRAME_ARGUMENT),
                )?,
                instruction("push_constant", wire::Operand::ConstantInt(ROOT_CHILD_BANK))?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(PRESENT_CHILD_SLOT),
                )?,
                instruction(
                    "cc_findbycategory",
                    wire::Operand::Byte(SOURCE_PRIMARY_SELECTOR),
                )?,
                instruction("push_constant", wire::Operand::ConstantInt(i32::from(true)))?,
                instruction(
                    "branch_not",
                    branch(PRESENT_COMPARISON_PC, CALLBACK_RETURN_PC)?,
                )?,
                instruction(
                    "push_int_local",
                    wire::Operand::Int(CALLBACK_FRAME_ARGUMENT),
                )?,
                instruction("push_constant", wire::Operand::ConstantInt(ROOT_CHILD_BANK))?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(MISSING_CHILD_SLOT),
                )?,
                instruction(
                    "cc_findbycategory",
                    wire::Operand::Byte(SOURCE_SECONDARY_SELECTOR),
                )?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(i32::from(false)),
                )?,
                instruction(
                    "branch_not",
                    branch(MISSING_COMPARISON_PC, CALLBACK_RETURN_PC)?,
                )?,
                instruction("push_constant", wire::Operand::ConstantInt(ROOT_CHILD_BANK))?,
                instruction(
                    "push_int_local",
                    wire::Operand::Int(CALLBACK_FRAME_ARGUMENT),
                )?,
                instruction(
                    "if_getnextcategorysubid",
                    wire::Operand::Byte(u8::default()),
                )?,
                instruction(
                    "push_constant",
                    wire::Operand::ConstantInt(NEXT_FRAME_CHILD_SLOT),
                )?,
                instruction(
                    "branch_not",
                    branch(SLOT_QUERY_COMPARISON_PC, CALLBACK_RETURN_PC)?,
                )?,
                instruction(
                    "push_string_local",
                    wire::Operand::Int(ACTIVE_STRING_ARGUMENT),
                )?,
                instruction("push_int_local", wire::Operand::Int(COMPONENT_ARGUMENT))?,
                instruction("if_settext", wire::Operand::Byte(u8::default()))?,
                instruction(
                    "push_int_local",
                    wire::Operand::Int(CALLBACK_FRAME_ARGUMENT),
                )?,
                instruction("cc_deleteall", wire::Operand::Byte(u8::default()))?,
                instruction("push_constant", wire::Operand::ConstantInt(i32::from(true)))?,
                instruction(
                    "push_int_local",
                    wire::Operand::Int(CALLBACK_FRAME_ARGUMENT),
                )?,
                instruction("if_sethide", wire::Operand::Byte(u8::default()))?,
                instruction("return", wire::Operand::Byte(u8::default()))?,
            ],
        },
        book,
    )?;
    Ok(BTreeMap::from([
        (source_id, source),
        (callback_id, callback),
    ]))
}

fn authored_enum_address() -> anyhow::Result<(i32, i32, i32, i32)> {
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../cs2/fixtures/enums.json"))?;
    let args: Vec<i32> = serde_json::from_value(fixture["queries"][0]["input"]["args"].clone())?;
    const INPUT_TYPE: usize = 1;
    const OUTPUT_TYPE: usize = 2;
    const ENUM_ID: usize = 3;
    const KEY: usize = 4;
    Ok((
        args[ENUM_ID],
        args[INPUT_TYPE],
        args[OUTPUT_TYPE],
        args[KEY],
    ))
}
fn authored_enums(
    book: &Book,
    label: &InterfaceComponent,
) -> anyhow::Result<cs2::enums::Definitions> {
    let native910::interface::ComponentBody::Text { colour, .. } = label.body else {
        anyhow::bail!("authored label is not text")
    };
    let (enum_id, input_type, output_type, key) = authored_enum_address()?;
    cs2::enums::Definitions::authored_fixture(
        book,
        include_bytes!("../../../../revisions/950/cs2/enums.json"),
        |library, codec| {
            let mut definition = codec.empty();
            definition.input_type = Some(u16::try_from(input_type)?);
            definition.output_type = Some(u16::try_from(output_type)?);
            definition
                .sparse
                .insert(key, rs910_config::ui_enum_schema::Value::Int(colour));
            library.definitions.insert(enum_id, definition.clone());
            library
                .definitions
                .insert(enum_id + i32::try_from(NEXT_SOURCE_ID)?, definition);
            let mut text = codec.empty();
            text.input_type = Some(u16::try_from(input_type)?);
            text.output_type = Some(library.string_type);
            text.sparse.insert(
                key,
                rs910_config::ui_enum_schema::Value::Text(String::new()),
            );
            library
                .definitions
                .insert(enum_id + AUTHORED_TEXT_ENUM_OFFSET, text);
            Ok(())
        },
    )
}
fn replay_enum_queries(book: &Book, script_id: u32) -> anyhow::Result<()> {
    use native910::{
        execution::{decode_accounting, HostOperation, Role, Specification, Table as Execution},
        opcode::OpcodeBook,
        script::{CompiledScript, Counts, Instruction, Operand},
        vm::{Programs, Session, Value, Vm},
    };
    use rs910_config::ui_enum_schema::Value as EnumValue;
    use std::sync::Arc;
    const OUTPUT_TYPE: usize = 2;
    const ENUM_ID: usize = 3;
    let native = OpcodeBook::embedded()?;
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../cs2/fixtures/enums.json"))?;
    assert_eq!(fixture["client_md5"], book.profile.client_md5);
    let decode_value = |value: &serde_json::Value| -> anyhow::Result<Option<EnumValue>> {
        if value.is_null() || value["kind"] == "hole" {
            return Ok(None);
        }
        Ok(Some(match value["kind"].as_str().unwrap() {
            "int" => EnumValue::Int(serde_json::from_value(value["value"].clone())?),
            "text" => EnumValue::Text(serde_json::from_value(value["value"].clone())?),
            _ => anyhow::bail!("recorded enum value has an unknown tag"),
        }))
    };
    let id = i32::try_from(script_id)?;
    for case in fixture["queries"].as_array().unwrap() {
        let input = &case["input"];
        let arguments: Vec<i32> = serde_json::from_value(input["args"].clone())?;
        let definitions = cs2::enums::Definitions::authored_fixture(
            book,
            include_bytes!("../../../../revisions/950/cs2/enums.json"),
            |library, codec| {
                let mut definition = codec.empty();
                definition.input_type = serde_json::from_value(input["input_type"].clone())?;
                definition.output_type = serde_json::from_value(input["output_type"].clone())?;
                let Some(EnumValue::Int(value)) = decode_value(&input["int_default"])? else {
                    anyhow::bail!("enum integer default tag")
                };
                definition.integer_default = value;
                let Some(EnumValue::Text(value)) = decode_value(&input["text_default"])? else {
                    anyhow::bail!("enum text default tag")
                };
                definition.text_default = value;
                definition.dense = input["dense"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(decode_value)
                    .collect::<anyhow::Result<_>>()?;
                for pair in input["sparse"].as_array().unwrap() {
                    definition.sparse.insert(
                        serde_json::from_value(pair[0].clone())?,
                        decode_value(&pair[1])?.unwrap(),
                    );
                }
                library.definitions.insert(arguments[ENUM_ID], definition);
                Ok(())
            },
        )?;
        let resource: Arc<[u8]> = definitions.library.encode_resource()?.into();
        let operation = HostOperation::Enum {
            output_type: arguments[OUTPUT_TYPE],
            string_type: definitions.library.string_type,
        };
        let instruction = |command: &str, operand| -> anyhow::Result<Instruction> {
            Ok(Instruction {
                opcode: native.opcode_for(command)?,
                command: command.into(),
                operand,
            })
        };
        let args = Counts {
            int: u16::try_from(arguments.len())?,
            ..Counts::default()
        };
        let mut code = (0..args.int)
            .map(|slot| instruction("push_int_local", Operand::Local(i32::from(slot))))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let consumer = code.len();
        code.push(instruction(
            operation.command(),
            Operand::Byte(u8::default()),
        )?);
        code.push(instruction("return", Operand::Byte(u8::default()))?);
        let mut script = CompiledScript {
            name: Some("proc,recorded_enum".into()),
            args,
            locals: args,
            code,
        };
        let mut execution = Execution::default();
        let bytes = execution.bind_import(
            id,
            &mut script,
            &native,
            Specification {
                role: Role::Adapter,
                resource: Some(resource),
                adapter_calls: BTreeMap::new(),
                host_operations: BTreeMap::from([(consumer, operation)]),
            },
        )?;
        let metadata = execution.group_bytes(id).unwrap();
        let accounting = decode_accounting(id, &bytes, Some(&metadata), &script)?;
        let stripped = execution
            .emit()
            .lines()
            .filter(|line| !line.starts_with("resource "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(decode_accounting(id, &bytes, Some(stripped.as_bytes()), &script).is_err());
        let programs = Programs {
            scripts: std::collections::HashMap::from([(id, script.clone())]),
            accounting: std::collections::HashMap::from([(id, accounting)]),
        };
        let inputs: Vec<_> = arguments.iter().map(|value| Value::Int(*value)).collect();
        let expected = &case["result"];
        let events = expected["events"].as_array().unwrap();
        let expected_failed = events
            .iter()
            .any(|event| event[0] == "type_error" || event[0] == "abort");
        let mut engine = crate::ui_runtime::Engine::default();
        // Repeated execution uses the same immutable resource owner.
        for _ in 0..usize::from(true) + usize::from(true) {
            let mut session = Session::for_script(id, &script, &inputs, None)?;
            let mut vm = Vm::new(&mut engine, &programs);
            let failed = loop {
                match vm.step(&mut session) {
                    Ok(true) => break false,
                    Ok(false) => {}
                    Err(_) => break true,
                }
            };
            let snapshot = session.snapshot();
            assert_eq!(failed, expected_failed, "{}", case["name"]);
            assert_eq!(
                serde_json::to_value(snapshot.ints)?,
                expected["ints"],
                "{}",
                case["name"]
            );
            let strings = events
                .iter()
                .filter(|event| event[0] == "object")
                .map(|event| Some(event[1].as_str().unwrap().to_owned()))
                .collect::<Vec<_>>();
            assert_eq!(snapshot.strings, strings, "{}", case["name"]);
            assert!(snapshot.longs.is_empty());
            assert_eq!(engine.configs.imported_enums.len(), usize::from(true));
        }
    }
    Ok(())
}

/// Independently authored palette data is the default. A maintainer can pin
/// actual modern donor bytes and numerical observations through a local case.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteCase {
    cache_root: PathBuf,
    procedure: u32,
    source_sha256: String,
    bindings: Vec<cs2::variable_bindings::Binding>,
    observations: Vec<PaletteObservation>,
    #[serde(default)]
    colour_helper: Option<ColourHelperCase>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColourHelperCase {
    procedure: u32,
    source_sha256: String,
    observations: Vec<PaletteObservation>,
}
#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PaletteObservation {
    player: i32,
    client: i32,
    key: i32,
    colour: i32,
}
pub(super) struct PaletteReplay {
    helper: ColourReplay,
    script: i32,
    player: u16,
    client: u16,
    observations: Vec<PaletteObservation>,
}
fn prepare_palette(
    book: &Book,
    frame: &Frame<'_>,
    pack_root: &Path,
) -> anyhow::Result<PaletteReplay> {
    use anyhow::{ensure, Context};
    use cs2::{
        variable_bindings,
        variables::{Base, Definitions},
    };
    use native910::vars::VarScope;
    const PALETTE_SOURCE_OFFSET: u32 = 2;
    const WRAPPER_SOURCE_OFFSET: u32 = 3;
    const KEY_SLOT: i32 = 1;
    const SINGLE_INPUT: u16 = 1;
    const PAIR_INPUTS: u16 = 2;
    const SYNTHETIC_BIT_ID: u32 = u16::MAX as u32 + 1;
    const SMART_ID_HIGH_BIT: u32 = 1 << (u32::BITS - 1);
    let symbols = variable_bindings::Symbols::parse(
        include_str!("../../../../revisions/910/symbols/varp.sym"),
        include_str!("../../../../revisions/910/symbols/varc.sym"),
    )?;
    let instruction = |command: &str, operand| -> anyhow::Result<wire::Instruction> {
        Ok(wire::Instruction {
            opcode: book
                .profile
                .opcodes
                .iter()
                .find(|row| row.command.as_deref() == Some(command))
                .context("palette source command missing")?
                .id,
            operand,
        })
    };
    let (procedure, mut scripts, enums, variables, bindings, observations) =
        if let Some(path) = std::env::var_os("ALTO_MODERN_PALETTE_CASE") {
            let case: PaletteCase = serde_json::from_slice(&std::fs::read(path)?)?;
            let scripts =
                import910::load_closure(&case.cache_root, book, std::iter::once(case.procedure))?;
            ensure!(
                cs2::profile::digest(&scripts[&case.procedure]) == case.source_sha256,
                "palette donor bytes changed"
            );
            let enums = cs2::enums::Definitions::load(
                &case.cache_root,
                book,
                include_bytes!("../../../../revisions/950/cs2/enums.json"),
            )?;
            let variables = Definitions::load(
                &case.cache_root,
                book,
                include_bytes!("../../../../revisions/950/cs2/variables.json"),
            )?;
            (
                case.procedure,
                scripts,
                enums,
                variables,
                case.bindings,
                case.observations,
            )
        } else {
            let procedure = frame.setup_script_id + PALETTE_SOURCE_OFFSET;
            let fixture: serde_json::Value =
                serde_json::from_slice(include_bytes!("../../../cs2/fixtures/variables.json"))?;
            let wire_for = |name: &str| -> anyhow::Result<Vec<u8>> {
                serde_json::from_value(
                    fixture["base_decodes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|row| row["name"] == name)
                        .context("palette base recording missing")?["wire"]
                        .clone(),
                )
                .map_err(Into::into)
            };
            let player = Base {
                domain: u8::from(VarScope::Player),
                id: i32::MAX,
            };
            let client = Base {
                domain: u8::from(VarScope::Client),
                id: i32::MAX,
            };
            let schema: serde_json::Value = serde_json::from_slice(include_bytes!(
                "../../../../revisions/950/cs2/variables.json"
            ))?;
            let bit_format = &schema["bit_definition"];
            let bit_wire = |base: Base| -> anyhow::Result<Vec<u8>> {
                let mut bytes = vec![
                    serde_json::from_value(bit_format["base"].clone())?,
                    base.domain,
                ];
                bytes.extend((u32::try_from(base.id)? | SMART_ID_HIGH_BIT).to_be_bytes());
                bytes.extend([
                    serde_json::from_value(bit_format["range"].clone())?,
                    u8::default(),
                    u8::default(),
                    serde_json::from_value(bit_format["end"].clone())?,
                ]);
                Ok(bytes)
            };
            let variables = Definitions::authored_fixture(
                book,
                include_bytes!("../../../../revisions/950/cs2/variables.json"),
                &[
                    (player, wire_for("player_theme")?),
                    (client, wire_for("client_theme")?),
                ],
                &[
                    (SYNTHETIC_BIT_ID, bit_wire(player)?),
                    (SYNTHETIC_BIT_ID + NEXT_SOURCE_ID, bit_wire(client)?),
                ],
            )?;
            let (enum_id, input_type, output_type, _) = authored_enum_address()?;
            let observations: Vec<PaletteObservation> =
                serde_json::from_value(fixture["authored_palette"].clone())?;
            let enums = cs2::enums::Definitions::authored_fixture(
                book,
                include_bytes!("../../../../revisions/950/cs2/enums.json"),
                |library, codec| {
                    let mut definition = codec.empty();
                    definition.input_type = Some(u16::try_from(input_type)?);
                    definition.output_type = Some(u16::try_from(output_type)?);
                    for observation in &observations {
                        definition.sparse.insert(
                            observation.player + observation.client,
                            rs910_config::ui_enum_schema::Value::Int(observation.colour),
                        );
                    }
                    library.definitions.insert(enum_id, definition);
                    Ok(())
                },
            )?;
            let args = wire::Counts {
                int: SINGLE_INPUT,
                ..Default::default()
            };
            let source = wire::encode(
                &wire::Script {
                    name: vec![],
                    args,
                    locals: args,
                    switches: vec![],
                    code: vec![
                        instruction("push_constant", wire::Operand::ConstantInt(input_type))?,
                        instruction("push_constant", wire::Operand::ConstantInt(output_type))?,
                        instruction("push_constant", wire::Operand::ConstantInt(enum_id))?,
                        instruction(
                            "push_varbit",
                            wire::Operand::Varbit {
                                id: SYNTHETIC_BIT_ID,
                                secondary: u8::default(),
                            },
                        )?,
                        instruction(
                            "push_varbit",
                            wire::Operand::Varbit {
                                id: SYNTHETIC_BIT_ID + NEXT_SOURCE_ID,
                                secondary: u8::default(),
                            },
                        )?,
                        instruction("add", wire::Operand::Byte(u8::default()))?,
                        instruction("enum", wire::Operand::Byte(u8::default()))?,
                        instruction("return", wire::Operand::Byte(u8::default()))?,
                    ],
                },
                book,
            )?;
            let bindings = vec![
                variable_bindings::Binding {
                    source: player,
                    target: Some("varp.interface_layout".into()),
                },
                variable_bindings::Binding {
                    source: client,
                    target: Some("varc.interface_theme_preview".into()),
                },
            ];
            (
                procedure,
                BTreeMap::from([(procedure, source)]),
                enums,
                variables,
                bindings,
                observations,
            )
        };
    let wrapper = frame.setup_script_id + WRAPPER_SOURCE_OFFSET;
    ensure!(
        !scripts.contains_key(&wrapper),
        "palette wrapper collides with donor closure"
    );
    let args = wire::Counts {
        int: PAIR_INPUTS,
        ..Default::default()
    };
    scripts.insert(
        wrapper,
        wire::encode(
            &wire::Script {
                name: vec![],
                args,
                locals: args,
                switches: vec![],
                code: vec![
                    instruction("push_int_local", wire::Operand::Int(KEY_SLOT))?,
                    instruction(
                        "gosub_with_params",
                        wire::Operand::Int(i32::try_from(procedure)?),
                    )?,
                    instruction("push_int_local", wire::Operand::Int(COMPONENT_ARGUMENT))?,
                    instruction("if_setcolour", wire::Operand::Byte(u8::default()))?,
                    instruction("return", wire::Operand::Byte(u8::default()))?,
                ],
            },
            book,
        )?,
    );
    let mut plan = Plan::from_root(&scripts, book, pack_root, wrapper, "live_palette")?;
    plan.enums = Some(enums.identity()?);
    let input = || variable_bindings::Import {
        definitions: &variables,
        symbols: &symbols,
    };
    let mut variable_plan = variable_bindings::Plan::prepare(input(), &scripts, book, pack_root)?;
    variable_plan.bindings = bindings;
    plan.variables = Some(variable_plan);
    plan.entries[0].int_arguments[0] = IntArgument::Component(LABEL_SYMBOL.into());
    let output = frame.directory.join("live-palette-import");
    let inames = std::fs::read_to_string(pack_root.join("inames.txt"))?;
    let report = import910::build(
        BuildInput {
            frames: None,
            scripts: &scripts,
            book,
            adapter_bytes: include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json"),
            plan: &plan,
            pack_root,
            inames_text: &inames,
            database: None,
            enums: Some(&enums),
            variables: Some(input()),
        },
        &output,
    )?;
    complete_pack(&output, pack_root)?;
    let owner = |domain: &str| -> anyhow::Result<u16> {
        Ok(report
            .variable_bindings
            .iter()
            .find(|binding| binding.target_domain == domain)
            .context("palette live owner missing")?
            .target_id)
    };
    let helper = prepare_colour_helper(ColourInput {
        book,
        frame,
        pack_root: &output,
        palette: procedure,
        palette_wrapper: wrapper,
        scripts: &scripts,
        enums: &enums,
        variables: &variables,
        symbols: &symbols,
        bindings: &plan
            .variables
            .as_ref()
            .context("palette bindings missing")?
            .bindings,
        observations: &observations,
    })?;
    Ok(PaletteReplay {
        helper,
        script: report.exports["live_palette"],
        player: owner(VarScope::Player.as_label())?,
        client: owner(VarScope::Client.as_label())?,
        observations,
    })
}
pub(super) fn replay_palette(
    palette: &PaletteReplay,
    game: &mut crate::client_game::ClientGame,
    ui: &mut crate::ui_runtime::Runtime,
    clock: &mut crate::test_support::SimClock,
    label: i32,
    directory: &Path,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use native910::{vars::VarScope, vm::Value};
    const STATIC_COMPONENT: i32 = -1;
    let mut observations = Vec::new();
    for expected in &palette.observations {
        clock.with(game, |variables| {
            variables.set(
                VarScope::Player,
                palette.player,
                false,
                Value::Int(expected.player),
            )?;
            variables.set(
                VarScope::Client,
                palette.client,
                false,
                Value::Int(expected.client),
            )
        })?;
        let mut payload = b"i\0".to_vec();
        payload.extend(expected.key.to_be_bytes());
        payload.extend(palette.script.to_be_bytes());
        let length = u16::try_from(payload.len())?;
        const SMART_OPCODE_HIGH_MARKER: u16 = 1 << (u16::BITS - 1);
        let mut wire = (u16::from(crate::proto::server::RUNCLIENTSCRIPT)
            | SMART_OPCODE_HIGH_MARKER)
            .to_be_bytes()
            .to_vec();
        wire.extend(length.to_be_bytes());
        wire.extend(payload);
        let (packet, consumed) =
            crate::net::decode_frame(&wire)?.context("palette packet incomplete")?;
        assert_eq!(consumed, wire.len());
        let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?
            .context("palette packet not routed")?;
        clock.with(game, |vars| ui.packet(vars, &event))?;
        clock.tick(game, ui)?;
        let component = ui
            .store
            .get(label, STATIC_COMPONENT)?
            .context("palette label absent")?;
        assert_eq!(
            component.borrow().f.colour,
            expected.colour,
            "player={}, client={}, key={}",
            expected.player,
            expected.client,
            expected.key
        );
        observations.push(serde_json::json!({"input":expected,"actual_colour":component.borrow().f.colour,"script":palette.script}));
    }
    clock.with(game, |variables| {
        variables.set(
            VarScope::Player,
            palette.player,
            false,
            Value::Int(i32::default()),
        )?;
        variables.set(
            VarScope::Client,
            palette.client,
            false,
            Value::Int(i32::default()),
        )
    })?;
    std::fs::write(
        directory.join("live-palette-observations.json"),
        serde_json::to_vec_pretty(&observations)?,
    )?;
    Ok(())
}

fn prepare_flat_child(book: &Book, frame: &Frame<'_>, pack_root: &Path) -> anyhow::Result<i32> {
    use anyhow::Context;
    const SOURCE_OFFSET: u32 = 4;
    const CHILD_SLOT: i32 = 1;
    const COLOUR_SLOT: i32 = 2;
    const INPUTS: u16 = 3;
    const FALSE_BRANCH: i32 = 2;
    const NONBOOLEAN_SELECTOR: u8 = 2;
    let instruction = |name: &str, operand| -> anyhow::Result<wire::Instruction> {
        Ok(wire::Instruction {
            opcode: book
                .profile
                .opcodes
                .iter()
                .find(|row| row.command.as_deref() == Some(name))
                .context("flat child source command missing")?
                .id,
            operand,
        })
    };
    let source = frame.setup_script_id + SOURCE_OFFSET;
    let args = wire::Counts {
        int: INPUTS,
        ..Default::default()
    };
    let scripts = BTreeMap::from([(
        source,
        wire::encode(
            &wire::Script {
                name: Vec::new(),
                args,
                locals: args,
                switches: Vec::new(),
                code: vec![
                    instruction("push_int_local", wire::Operand::Int(COMPONENT_ARGUMENT))?,
                    instruction("push_int_local", wire::Operand::Int(CHILD_SLOT))?,
                    instruction("cc_find", wire::Operand::Byte(NONBOOLEAN_SELECTOR))?,
                    instruction("branch_if_false", wire::Operand::Int(FALSE_BRANCH))?,
                    instruction("push_int_local", wire::Operand::Int(COLOUR_SLOT))?,
                    instruction("cc_setcolour", wire::Operand::Byte(u8::from(true)))?,
                    instruction("return", wire::Operand::Byte(u8::default()))?,
                ],
            },
            book,
        )?,
    )]);
    let mut plan = Plan::from_root(&scripts, book, pack_root, source, "select_flat_child")?;
    plan.entries[0].int_arguments[0] = IntArgument::Component(FRAME_SYMBOL.into());
    let output = frame.directory.join("flat-child-import");
    let inames = std::fs::read_to_string(pack_root.join("inames.txt"))?;
    let report = import910::build(
        BuildInput {
            frames: None,
            scripts: &scripts,
            book,
            adapter_bytes: include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json"),
            plan: &plan,
            pack_root,
            inames_text: &inames,
            database: None,
            enums: None,
            variables: None,
        },
        &output,
    )?;
    assert!(report
        .child_selection_bindings
        .iter()
        .any(|site| site.component == FRAME_SYMBOL));
    complete_pack(&output, pack_root)?;
    Ok(report.exports["select_flat_child"])
}

pub(super) fn replay_flat_child(
    script: i32,
    child: i32,
    colour: i32,
    game: &mut crate::client_game::ClientGame,
    ui: &mut crate::ui_runtime::Runtime,
    clock: &mut crate::test_support::SimClock,
) -> anyhow::Result<()> {
    use anyhow::Context;
    const SMART_OPCODE_HIGH_MARKER: u16 = 1 << (u16::BITS - 1);
    let mut payload = b"ii\0".to_vec();
    payload.extend(colour.to_be_bytes());
    payload.extend(child.to_be_bytes());
    payload.extend(script.to_be_bytes());
    let length = u16::try_from(payload.len())?;
    let mut bytes = (u16::from(crate::proto::server::RUNCLIENTSCRIPT) | SMART_OPCODE_HIGH_MARKER)
        .to_be_bytes()
        .to_vec();
    bytes.extend(length.to_be_bytes());
    bytes.extend(payload);
    let (packet, consumed) =
        crate::net::decode_frame(&bytes)?.context("flat child packet incomplete")?;
    assert_eq!(consumed, bytes.len());
    let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?
        .context("flat child packet not routed")?;
    clock.with(game, |vars| ui.packet(vars, &event))?;
    clock.tick(game, ui)?;
    Ok(())
}

// An independently authored native installer isolates retained runtime-child
// delivery alongside the complete source callback import above.
fn prepare_retained_hook(frame: &Frame<'_>, callback: i32, player: u16) -> anyhow::Result<i32> {
    use native910::{
        config::ConfigTypes,
        execution::{HostOperation, Role, Specification, Table},
        inames::InterfaceRegistry,
        opcode::OpcodeBook,
        pack::PackArchive,
        script::{CompiledScript, Counts, Instruction, Operand},
        source,
    };
    const INPUTS: u16 = 2;
    const HOOK_INTEGER_INPUTS: u16 = 5;
    const DESCRIPTOR_INPUTS: u16 = 1; // not a content id: descriptor argument count.
    const CHILD_TOKEN_INDEX: usize = 4;
    const CHILD_SLOT: i32 = 0;
    const COLOUR_SLOT: i32 = 1;
    const NEXT_SCRIPT: u32 = 1; // not a content id: allocation increment.
    const CALL_BEFORE_RETURN: usize = 2;
    let base = frame.directory.join("flat-child-import");
    let archive = PackArchive::open(&base.join("client.scripts.js5"))?;
    let installer = i32::try_from(archive.group_ids().max().unwrap_or_default() + NEXT_SCRIPT)?;
    let bridge_id = installer + i32::try_from(NEXT_SCRIPT)?;
    let book = OpcodeBook::embedded()?;
    let instruction = |command: &str, operand| -> anyhow::Result<Instruction> {
        Ok(Instruction {
            opcode: book.opcode_for(command)?,
            command: command.into(),
            operand,
        })
    };
    let policy: cs2::lower910::Adapter = serde_json::from_slice(include_bytes!(
        "../../../../revisions/950/cs2/950-1-to-910.json"
    ))?;
    let child_token = policy.callback_policy.as_ref().unwrap().int_event_tokens[CHILD_TOKEN_INDEX];
    let input_counts = Counts {
        int: INPUTS,
        ..Counts::default()
    };
    let mut entry = CompiledScript {
        name: Some("proc,install_retained_hook".into()),
        args: input_counts,
        locals: input_counts,
        code: vec![
            instruction("push_int_local", Operand::Local(CHILD_SLOT))?,
            instruction("push_int_local", Operand::Local(COLOUR_SLOT))?,
            instruction("gosub_with_params", Operand::Script(callback))?,
            instruction("push_constant_string", Operand::Int(callback))?,
            instruction("push_constant_string", Operand::Int(child_token))?,
            instruction("push_int_local", Operand::Local(COLOUR_SLOT))?,
            instruction("push_constant_string", Operand::Int(i32::from(player)))?,
            instruction("push_constant_string", Operand::Int(i32::from(true)))?,
            instruction("push_constant_string", Operand::Str("iiY".into()))?,
            instruction("gosub_with_params", Operand::Script(bridge_id))?,
            instruction("return", Operand::Byte(u8::default()))?,
        ],
    };
    let args = Counts {
        int: HOOK_INTEGER_INPUTS,
        obj: DESCRIPTOR_INPUTS,
        ..Counts::default()
    };
    let token_policy = &policy.callback_policy.as_ref().unwrap().int_event_tokens;
    let operation = HostOperation::RetainedPlayerTransmit {
        event_tokens: native910::execution::VariableEventTokens::new(
            token_policy[0],
            u8::try_from(token_policy.len())?,
        )?,
        pops: [args.int, args.obj, args.long],
    };
    let mut code = (0..args.int)
        .map(|slot| instruction("push_int_local", Operand::Local(i32::from(slot))))
        .collect::<anyhow::Result<Vec<_>>>()?;
    code.push(instruction(
        "push_string_local",
        Operand::Local(i32::default()),
    )?);
    let setter = code.len();
    code.push(instruction(
        operation.command(),
        Operand::Byte(u8::from(true)),
    )?);
    code.push(instruction("return", Operand::Byte(u8::default()))?);
    let mut bridge = CompiledScript {
        name: Some("proc,retained_hook_use".into()),
        args,
        locals: args,
        code,
    };
    let mut execution = Table::default();
    execution.bind_import(
        bridge_id,
        &mut bridge,
        &book,
        Specification {
            role: Role::Adapter,
            adapter_calls: BTreeMap::new(),
            host_operations: BTreeMap::from([(setter, operation)]),
            resource: None,
        },
    )?;
    let call_pc = entry.code.len() - CALL_BEFORE_RETURN;
    execution.bind_import(
        installer,
        &mut entry,
        &book,
        Specification {
            role: Role::Source,
            adapter_calls: BTreeMap::from([(call_pc, bridge_id)]),
            host_operations: BTreeMap::new(),
            resource: None,
        },
    )?;
    let scripts = BTreeMap::from([(installer, entry), (bridge_id, bridge)]);
    let mut all = native910::xref::load_scripts(&base)?;
    all.extend(scripts.clone());
    let configs = ConfigTypes::load(&base)?;
    let curated = vec![
        (installer, "install_retained_hook".into()),
        (bridge_id, "retained_hook_use".into()),
    ];
    let (returns, _) = native910::dataflow::infer_summaries_with_options(
        &all,
        &configs,
        &execution.analysis_options(),
    );
    let symbols = source::registry_for_decoded_scripts(&all, &curated, &returns)?;
    let names = InterfaceRegistry::empty();
    let directory = frame.directory.join("retained-hook-project");
    std::fs::create_dir_all(&directory)?;
    let mut manifest = format!(
        "base-sha256 {}\nexecution execution.tsv\n",
        project::sha256(&std::fs::read(base.join("client.scripts.js5"))?)
    );
    for (id, script) in &scripts {
        let text = source::format_source(
            &source::lift(script, &symbols, &configs, &names)?,
            &symbols,
            &names,
        )?;
        assert_eq!(
            source::assemble_source(&text, &book, &symbols, &configs, &names)?,
            native910::script::encode_script(script, &book)?
        );
        std::fs::write(directory.join(format!("{id}.rs2")), text)?;
        manifest.push_str(&format!(
            "script {id} {} {id}.rs2\n",
            curated.iter().find(|entry| entry.0 == *id).unwrap().1
        ));
    }
    // Source references resolve through the complete, pinned script registry.
    std::fs::write(
        directory.join("names.tsv"),
        curated
            .iter()
            .map(|(id, name)| format!("script {id} {name}\n"))
            .collect::<String>(),
    )?;
    manifest.push_str("names names.tsv\n");
    std::fs::write(directory.join("execution.tsv"), execution.emit())?;
    std::fs::write(directory.join("project.txt"), manifest)?;
    let output = frame.directory.join("retained-hook-import");
    project::build(&directory.join("project.txt"), &base, &output)?;
    complete_pack(&output, &base)?;
    Ok(installer)
}

pub(super) fn replay_retained_hook(
    imported: &Imported,
    child: &crate::ui_components::Ref,
    game: &mut crate::client_game::ClientGame,
    ui: &mut crate::ui_runtime::Runtime,
    clock: &mut crate::test_support::SimClock,
    directory: &Path,
) -> anyhow::Result<()> {
    use anyhow::Context;
    const COLOUR_CHANNELS: usize = 3;
    const LIVE_COLOUR: i32 = 0x0055_aacc;
    const BEFORE_EVENT_COLOUR: i32 = 0x00aa_5511;
    const SERVER_VALUE: i32 = 1;
    const PENDING_EXPIRY_MARGIN_MS: i64 = 600;
    const NO_CHANGE_WAIT_MS: i64 = 20;
    let child_id = child.borrow().f.id;
    replay_flat_child(
        imported.retained_hook_script,
        child_id,
        LIVE_COLOUR,
        game,
        ui,
        clock,
    )?;
    assert!(child.borrow().retained_player_transmit.is_some());
    assert_eq!(
        child.borrow().transmits["onvartransmitlist"],
        vec![i32::from(imported.palette.player)]
    );
    // Capture a fresh baseline after installation; the owner may already have
    // changes waiting from the earlier palette replay.
    child.borrow_mut().f.colour = BEFORE_EVENT_COLOUR;
    let before = ui.diagnostics.executions.len();
    clock.with(game, |vars| {
        let now = (vars.now)();
        vars.player
            .as_mut()
            .context("live player domain absent")?
            .set_server(i32::from(imported.palette.player), SERVER_VALUE, now)
            .map_err(|error| anyhow::anyhow!("player variable write: {error:?}"))?;
        Ok(())
    })?;
    clock.0 += PENDING_EXPIRY_MARGIN_MS;
    game.game
        .poll_vars(|| clock.0)
        .map_err(|error| anyhow::anyhow!("player variable poll: {error:?}"))?;
    clock.tick(game, ui)?;
    assert_eq!(child.borrow().f.colour, LIVE_COLOUR);
    assert!(ui.diagnostics.executions[before..]
        .iter()
        .any(|row| row["id"] == imported.flat_child_script && row["ok"] == true));
    ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
    let after = ui.diagnostics.executions.len();
    clock.0 += NO_CHANGE_WAIT_MS;
    clock.tick(game, ui)?;
    assert_eq!(ui.diagnostics.executions.len(), after);
    std::fs::write(
        directory.join("retained-hook-observations.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "installer":imported.retained_hook_script,"callback":imported.flat_child_script,
            "variable_symbol":"varp.interface_layout","child":child_id,
            "colour":child.borrow().f.colour,"delivered":true,"repeated_without_change":false,
        }))?,
    )?;
    // Restore the test frame's hook state before the existing pointer replay.
    child.borrow_mut().hooks.remove("onvartransmit");
    child.borrow_mut().transmits.remove("onvartransmitlist");
    child.borrow_mut().retained_player_transmit = None;
    Ok(())
}

struct ColourReplay {
    entry: i32,
    creator: i32,
    style: i32,
    text_style: i32,
    target_font: i32,
    font_reads: Vec<i32>,
    font_seed: i32,
    font_reader: i32,
    font_frame: i32,
    initial_font: i32,
    text_alignment: [i32; 3],
    max_lines: i32,
    template: rs910_config::ui_component_fields::TextChildTemplate,
    created_slot: u16,
    creation_pc: usize,
    callback: i32,
    observations: Vec<PaletteObservation>,
}
struct ColourInput<'a> {
    book: &'a Book,
    frame: &'a Frame<'a>,
    pack_root: &'a Path,
    palette: u32,
    palette_wrapper: u32,
    scripts: &'a BTreeMap<u32, Vec<u8>>,
    enums: &'a cs2::enums::Definitions,
    variables: &'a cs2::variables::Definitions,
    symbols: &'a cs2::variable_bindings::Symbols,
    bindings: &'a [cs2::variable_bindings::Binding],
    observations: &'a [PaletteObservation],
}
fn prepare_colour_helper(input: ColourInput<'_>) -> anyhow::Result<ColourReplay> {
    use anyhow::{ensure, Context};
    use cs2::{semantic, variable_bindings};
    use native910::vars::VarScope;
    const ROOT_OFFSET: u32 = 4;
    const CALLBACK_OFFSET: u32 = 5;
    const ROOT_INPUTS: u16 = 1;
    const CALLBACK_INPUTS: u16 = 4;
    const KEY_INPUT: i32 = 0;
    const CALLBACK_PARENT: i32 = 0;
    const CALLBACK_CHILD: i32 = 1;
    const CALLBACK_KEY: i32 = 2;
    const CALLBACK_OLD: i32 = 3;
    const PARENT_EVENT: usize = 2;
    const CHILD_EVENT: usize = 4;
    const OLD_COMPARE: usize = 2;
    const FIND_FAILURE: usize = 6;
    const CALLBACK_EXIT: usize = 9;
    let ColourInput {
        book,
        frame,
        pack_root,
        palette,
        palette_wrapper,
        scripts,
        enums,
        variables,
        symbols,
        bindings,
        observations,
    } = input;
    let local_case = std::env::var_os("ALTO_MODERN_PALETTE_CASE")
        .map(|path| -> anyhow::Result<PaletteCase> {
            Ok(serde_json::from_slice(&std::fs::read(path)?)?)
        })
        .transpose()?;
    let frame_recording: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../cs2/fixtures/style-fonts.json"))?;
    let source_frame: cs2::frames::FrameRef =
        serde_json::from_value(frame_recording["authored_frame"].clone())?;
    let frame_schema = include_bytes!("../../../../revisions/950/cs2/frames.json");
    let donor_frames = if let Some(case) = &local_case {
        cs2::frames::Definitions::load(&case.cache_root, book, frame_schema)?
    } else {
        cs2::frames::Definitions::authored_fixture(
            book,
            frame_schema,
            &std::collections::BTreeMap::from([(
                source_frame,
                serde_json::from_value(frame_recording["authored_wire"].clone())?,
            )]),
            &serde_json::from_value(frame_recording["authored_styles"].clone())?,
        )?
    };
    let initial_font = donor_frames.font_contract(source_frame)?.font;
    if local_case.is_none() {
        ensure!(
            serde_json::json!(initial_font) == frame_recording["authored_initial_font"],
            "direct style font differs from the original numeric recording"
        );
    }
    let source_frame_address = i32::from(u16::try_from(source_frame.interface)?) << u16::BITS
        | i32::from(u16::try_from(source_frame.file)?);
    let (root, callback, mut sources, database, observations) = if let Some((cache_root, helper)) =
        local_case.and_then(|case| case.colour_helper.map(|helper| (case.cache_root, helper)))
    {
        let database = cs2::database::Definitions::load(
            &cache_root,
            book,
            include_bytes!("../../../../revisions/950/cs2/database.json"),
        )?;
        let sources = import910::load_closure_with_database(
            &cache_root,
            book,
            std::iter::once(helper.procedure),
            Some(&database),
        )?;
        ensure!(
            cs2::profile::digest(&sources[&helper.procedure]) == helper.source_sha256,
            "colour helper donor bytes changed"
        );
        let inspection = import910::inspect_closure_with_database(&sources, book, Some(&database))?;
        let installed: Vec<_> = inspection
            .callbacks
            .iter()
            .filter_map(|site| match site.status {
                cs2::flow::CallbackStatus::Installed { callee }
                    if site.caller == helper.procedure =>
                {
                    Some(callee)
                }
                _ => None,
            })
            .collect();
        ensure!(
            installed.len() == usize::from(true),
            "colour helper callback is ambiguous"
        );
        (
            helper.procedure,
            installed[0],
            sources,
            Some(database),
            helper.observations,
        )
    } else {
        let root = frame.setup_script_id + ROOT_OFFSET;
        let callback = frame.setup_script_id + CALLBACK_OFFSET;
        let adapter: cs2::lower910::Adapter = serde_json::from_slice(include_bytes!(
            "../../../../revisions/950/cs2/950-1-to-910.json"
        ))?;
        let policy = adapter
            .callback_policy
            .as_ref()
            .context("callback policy missing")?;
        let instruction = |command: &str, operand| -> anyhow::Result<wire::Instruction> {
            Ok(wire::Instruction {
                opcode: book
                    .profile
                    .opcodes
                    .iter()
                    .find(|row| row.command.as_deref() == Some(command))
                    .context("colour source command missing")?
                    .id,
                operand,
            })
        };
        let push = |value| instruction("push_constant", wire::Operand::ConstantInt(value));
        let local = |slot| instruction("push_int_local", wire::Operand::Int(slot));
        let use_byte = |command| instruction(command, wire::Operand::Byte(u8::default()));
        let bit = semantic::normalize(&scripts[&palette], book)?
            .instructions
            .iter()
            .filter_map(|instruction| instruction.operation.as_ref())
            .find_map(|operation| match operation.argument {
                semantic::Argument::Varbit { id, .. } => {
                    (variables.bit_contract(id).ok()?.base.domain == u8::from(VarScope::Player))
                        .then_some(id)
                }
                _ => None,
            })
            .context("authored palette has no player bit")?;
        let base = variables.bit_contract(bit)?.base;
        let read_bit = || {
            instruction(
                "push_varbit",
                wire::Operand::Varbit {
                    id: bit,
                    secondary: u8::default(),
                },
            )
        };
        let root_counts = wire::Counts {
            int: ROOT_INPUTS,
            ..Default::default()
        };
        let callback_counts = wire::Counts {
            int: CALLBACK_INPUTS,
            ..Default::default()
        };
        let mut sources = scripts.clone();
        sources.remove(&palette_wrapper);
        ensure!(
            !sources.contains_key(&root) && !sources.contains_key(&callback),
            "authored colour source collision"
        );
        sources.insert(
            root,
            wire::encode(
                &wire::Script {
                    name: Vec::new(),
                    args: root_counts,
                    locals: root_counts,
                    switches: Vec::new(),
                    code: vec![
                        local(KEY_INPUT)?,
                        instruction(
                            "gosub_with_params",
                            wire::Operand::Int(i32::try_from(palette)?),
                        )?,
                        use_byte("cc_setcolour")?,
                        push(i32::try_from(callback)?)?,
                        push(policy.int_event_tokens[PARENT_EVENT])?,
                        push(policy.int_event_tokens[CHILD_EVENT])?,
                        local(KEY_INPUT)?,
                        read_bit()?,
                        push(base.id)?,
                        push(i32::from(true))?,
                        instruction(
                            "push_constant",
                            wire::Operand::ConstantString(b"iiiiY".to_vec()),
                        )?,
                        use_byte("active_transmit_hook")?,
                        use_byte("return")?,
                    ],
                },
                book,
            )?,
        );
        let relative = |from: usize| i32::try_from(CALLBACK_EXIT - from - usize::from(true));
        sources.insert(
            callback,
            wire::encode(
                &wire::Script {
                    name: Vec::new(),
                    args: callback_counts,
                    locals: callback_counts,
                    switches: Vec::new(),
                    code: vec![
                        read_bit()?,
                        local(CALLBACK_OLD)?,
                        instruction("branch_equals", wire::Operand::Int(relative(OLD_COMPARE)?))?,
                        local(CALLBACK_PARENT)?,
                        local(CALLBACK_CHILD)?,
                        use_byte("cc_find")?,
                        instruction(
                            "branch_if_false",
                            wire::Operand::Int(relative(FIND_FAILURE)?),
                        )?,
                        local(CALLBACK_KEY)?,
                        instruction(
                            "gosub_with_params",
                            wire::Operand::Int(i32::try_from(root)?),
                        )?,
                        use_byte("return")?,
                    ],
                },
                book,
            )?,
        );
        let observations = observations
            .iter()
            .filter(|row| row.client == i32::default())
            .cloned()
            .collect();
        (root, callback, sources, None, observations)
    };
    ensure!(
        observations.len() >= usize::from(true) + usize::from(true)
            && observations
                .iter()
                .all(|row| row.key == observations[0].key && row.client == observations[0].client),
        "colour replay needs a single key and client setting across player transitions"
    );
    const CREATOR_OFFSET: u32 = 6;
    const CREATOR_ARGUMENTS: u16 = 2;
    const CREATOR_PARENT: i32 = 0;
    const CREATOR_KEY: i32 = 1;
    const CREATION_PC: usize = 3;
    let revision: cs2::lower910::Adapter = serde_json::from_slice(include_bytes!(
        "../../../../revisions/950/cs2/950-1-to-910.json"
    ))?;
    let constructor = revision
        .rules
        .iter()
        .flat_map(|rule| rule.operations.values())
        .find(|target| target.requirement == cs2::lower910::Requirement::FlatTextChildCreation)
        .and_then(|target| target.child_template.as_ref())
        .context("text constructor missing")?;
    let recording: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../crates/rs910-ui/fixtures/modern-text-child.json"
    ))?;
    let created_slot = u16::try_from(
        recording["cases"][0]["input"]["child_id"]
            .as_u64()
            .context("recorded child identity missing")?,
    )?;
    let creator = frame.setup_script_id + CREATOR_OFFSET;
    ensure!(
        !sources.contains_key(&creator),
        "modern creator source collision"
    );
    let instruction = |command: &str, operand| -> anyhow::Result<wire::Instruction> {
        Ok(wire::Instruction {
            opcode: book
                .profile
                .opcodes
                .iter()
                .find(|row| row.command.as_deref() == Some(command))
                .with_context(|| format!("creator source command missing: {command}"))?
                .id,
            operand,
        })
    };
    let creator_counts = wire::Counts {
        int: CREATOR_ARGUMENTS,
        ..Default::default()
    };
    sources.insert(
        creator,
        wire::encode(
            &wire::Script {
                name: Vec::new(),
                args: creator_counts,
                locals: creator_counts,
                switches: Vec::new(),
                code: vec![
                    instruction("push_int_local", wire::Operand::Int(CREATOR_PARENT))?,
                    instruction(
                        "push_constant",
                        wire::Operand::ConstantInt(i32::from(constructor.source_kind)),
                    )?,
                    instruction(
                        "push_constant",
                        wire::Operand::ConstantInt(i32::from(created_slot)),
                    )?,
                    instruction("cc_create", wire::Operand::Byte(u8::default()))?,
                    instruction("push_int_local", wire::Operand::Int(CREATOR_KEY))?,
                    instruction(
                        "gosub_with_params",
                        wire::Operand::Int(i32::try_from(root)?),
                    )?,
                    instruction("return", wire::Operand::Byte(u8::default()))?,
                ],
            },
            book,
        )?,
    );
    const TEXT_STYLE_OFFSET: u32 = 7;
    const TEXT_STYLE_FONT_PC: usize = 7;
    const TEXT_STYLE_SECOND_FONT_PC: usize = 11;
    const TEXT_STYLE_READ_PCS: [usize; 3] = [4, 8, 12];
    const TEXT_STYLE_LOCALS: u16 = 4;
    const ABSENT_FONT_LOCAL: i32 = 1;
    const FIRST_FONT_LOCAL: i32 = 2;
    const SECOND_FONT_LOCAL: i32 = 3;
    const TEXT_STYLE_PROPERTY_USES: usize = 11;
    const FONT_READER_USE_PCS: [usize; FONT_READER_VALUES] = [0, 2];
    const FONT_SEED_OFFSET: u32 = 8;
    const FONT_READER_OFFSET: u32 = 9;
    const FONT_SEED_PCS: [usize; 2] = [1, 4];
    const TEXT_STYLE_FIND_PC: usize = 2;
    let text_style = frame.setup_script_id + TEXT_STYLE_OFFSET;
    ensure!(
        !sources.contains_key(&text_style),
        "modern text style source collision"
    );
    let native910::interface::ComponentBody::Text {
        font: target_font,
        halign,
        valign,
        line_height,
        maxlines,
        ..
    } = frame.label.body
    else {
        anyhow::bail!("authoring frame has no text style");
    };
    let text_recording: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../crates/rs910-ui/fixtures/modern-text-properties.json"
    ))?;
    let source_font = i32::try_from(
        text_recording["cases"][0]["input"]["values"][0]
            .as_i64()
            .context("recorded font input missing")?,
    )?;
    let font_recording: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../crates/rs910-ui/fixtures/modern-font-identity.json"
    ))?;
    let font_sequence = font_recording["cases"]
        .as_array()
        .context("font reader recording missing")?
        .iter()
        .find(|case| case["input"]["writes"].is_array())
        .context("font reader sequence missing")?;
    let writes: Vec<i32> = serde_json::from_value(font_sequence["input"]["writes"].clone())?;
    ensure!(
        writes[0] == source_font,
        "font setter and reader recordings differ"
    );
    let mut font_reads = vec![constructor.font];
    font_reads.extend(serde_json::from_value::<Vec<i32>>(
        font_sequence["result"]["reads_after_writes"].clone(),
    )?);
    let text_alignment = [i32::from(halign), i32::from(valign), i32::from(line_height)];
    let style_args = wire::Counts {
        int: u16::from(true),
        ..Default::default()
    };
    let scalar = |value| instruction("push_constant", wire::Operand::ConstantInt(value));
    let byte = |command| instruction(command, wire::Operand::Byte(u8::default()));
    sources.insert(
        text_style,
        wire::encode(
            &wire::Script {
                name: Vec::new(),
                args: style_args,
                locals: wire::Counts {
                    int: TEXT_STYLE_LOCALS,
                    ..style_args
                },
                switches: Vec::new(),
                code: vec![
                    scalar(
                        i32::try_from(frame.group)? << u16::BITS | i32::try_from(frame.root_file)?,
                    )?,
                    instruction("push_int_local", wire::Operand::Int(i32::default()))?,
                    byte("cc_find")?,
                    instruction("pop_int_local", wire::Operand::Int(i32::default()))?,
                    byte("cc_getfontgraphic")?,
                    instruction("pop_int_local", wire::Operand::Int(ABSENT_FONT_LOCAL))?,
                    scalar(source_font)?,
                    byte("cc_settextfont")?,
                    byte("cc_getfontgraphic")?,
                    instruction("pop_int_local", wire::Operand::Int(FIRST_FONT_LOCAL))?,
                    scalar(writes[usize::from(true)])?,
                    byte("cc_settextfont")?,
                    byte("cc_getfontgraphic")?,
                    instruction("pop_int_local", wire::Operand::Int(SECOND_FONT_LOCAL))?,
                    scalar(text_alignment[0])?,
                    scalar(text_alignment[1])?,
                    scalar(text_alignment[2])?,
                    byte("cc_settextalign")?,
                    scalar(i32::from(maxlines))?,
                    byte("cc_setmaxlines")?,
                    instruction("push_int_local", wire::Operand::Int(ABSENT_FONT_LOCAL))?,
                    instruction("push_int_local", wire::Operand::Int(FIRST_FONT_LOCAL))?,
                    instruction("push_int_local", wire::Operand::Int(SECOND_FONT_LOCAL))?,
                    byte("return")?,
                ],
            },
            book,
        )?,
    );
    let font_seed = frame.setup_script_id + FONT_SEED_OFFSET;
    let font_reader = frame.setup_script_id + FONT_READER_OFFSET;
    for (id, code) in [
        (
            font_seed,
            vec![
                instruction("push_int_local", wire::Operand::Int(i32::default()))?,
                byte("cc_settextfont")?,
                instruction("push_int_local", wire::Operand::Int(i32::default()))?,
                scalar(DONOR_OPERATION_COMPONENT)?,
                byte("if_settextfont")?,
                instruction("push_int_local", wire::Operand::Int(i32::default()))?,
                byte("return")?,
            ],
        ),
        (
            font_reader,
            vec![
                byte("cc_getfontmetrics")?,
                scalar(source_frame_address)?,
                byte("if_getfontmetrics")?,
                byte("return")?,
            ],
        ),
    ] {
        ensure!(!sources.contains_key(&id), "font interop source collision");
        sources.insert(
            id,
            wire::encode(
                &wire::Script {
                    name: Vec::new(),
                    args: wire::Counts {
                        int: u16::from(id == font_seed),
                        ..Default::default()
                    },
                    locals: wire::Counts {
                        int: u16::from(id == font_seed),
                        ..Default::default()
                    },
                    switches: Vec::new(),
                    code,
                },
                book,
            )?,
        );
    }
    let inspection = import910::inspect_closure_with_database(&sources, book, database.as_ref())?;
    let font_fact = inspection
        .font_uses
        .iter()
        .find(|site| site.caller == text_style && site.instruction == TEXT_STYLE_FONT_PC)
        .context("source font use is absent from inspection")?;
    ensure!(
        matches!(font_fact.font.as_ref().and_then(|value| value.constant.as_ref()), Some(cs2::flow::Literal::Int(value)) if *value == source_font),
        "source inspector lost the original font identity"
    );
    ensure!(
        TEXT_STYLE_READ_PCS
            .iter()
            .all(|pc| inspection
                .font_uses
                .iter()
                .any(|site| site.caller == text_style
                    && site.instruction == *pc
                    && matches!(site.access, cs2::flow::FontAccess::Read)
                    && site.font.is_none())),
        "source inspector does not distinguish font reads"
    );
    ensure!(
        FONT_SEED_PCS
            .iter()
            .all(|pc| inspection
                .font_uses
                .iter()
                .any(|site| site.caller == font_seed
                    && site.instruction == *pc
                    && matches!(site.access, cs2::flow::FontAccess::Write)
                    && site
                        .font
                        .as_ref()
                        .is_some_and(|value| value.constant.is_none()))),
        "dynamic font arguments were assumed constant"
    );
    let mut plan = Plan::from_root(&sources, book, pack_root, creator, "modern_colour")?;
    plan.frames = Some(donor_frames.identity());
    plan.entries.push(Entry {
        procedure: text_style,
        name: "modern_text_style".into(),
        int_arguments: vec![IntArgument::Input],
        string_arguments: Vec::new(),
        long_arguments: Vec::new(),
        active_component: Some(FRAME_SYMBOL.into()),
    });
    for (procedure, name) in [
        (font_seed, "modern_font_seed"),
        (font_reader, "modern_font_reader"),
    ] {
        plan.entries.push(Entry {
            procedure,
            name: name.into(),
            int_arguments: if procedure == font_seed {
                vec![IntArgument::Input]
            } else {
                Vec::new()
            },
            string_arguments: Vec::new(),
            long_arguments: Vec::new(),
            active_component: Some(LABEL_SYMBOL.into()),
        });
    }
    for instruction in FONT_READER_USE_PCS {
        plan.initial_fonts.push(cs2::import910::InitialFontUse {
            procedure: font_reader,
            instruction,
            component: LABEL_SYMBOL.into(),
            source_font: None,
            source_frame: Some(source_frame),
        });
    }
    plan.component_uses.push(ComponentUse {
        procedure: font_reader,
        instruction: FONT_READER_USE_PCS[usize::from(true)],
        component: LABEL_SYMBOL.into(),
    });
    plan.component_uses.push(ComponentUse {
        procedure: text_style,
        instruction: TEXT_STYLE_FIND_PC,
        component: FRAME_SYMBOL.into(),
    });
    plan.font_uses.push(cs2::import910::FontUse {
        procedure: text_style,
        instruction: TEXT_STYLE_FONT_PC,
        source_font,
        from_component: Some(LABEL_SYMBOL.into()),
    });
    plan.font_uses.push(cs2::import910::FontUse {
        procedure: text_style,
        instruction: TEXT_STYLE_SECOND_FONT_PC,
        source_font: writes[usize::from(true)],
        from_component: Some(LABEL_SYMBOL.into()),
    });
    for instruction in FONT_SEED_PCS {
        plan.font_maps.push(cs2::import910::FontMapUse {
            procedure: font_seed,
            instruction,
            fonts: writes
                .iter()
                .copied()
                .chain(std::iter::once(constructor.font))
                .map(|source_font| cs2::import910::FontChoice {
                    source_font,
                    from_component: (source_font != constructor.font).then(|| LABEL_SYMBOL.into()),
                })
                .collect(),
        });
    }
    plan.component_uses.push(ComponentUse {
        procedure: font_seed,
        instruction: FONT_SEED_PCS[usize::from(true)],
        component: LABEL_SYMBOL.into(),
    });
    plan.entries[0].int_arguments[CREATOR_PARENT as usize] =
        IntArgument::Component(FRAME_SYMBOL.into());
    plan.entries[0].active_component = Some(FRAME_SYMBOL.into());
    plan.enums = Some(enums.identity()?);
    plan.database = database
        .as_ref()
        .map(cs2::database::Definitions::identity)
        .transpose()?;
    let live = || variable_bindings::Import {
        definitions: variables,
        symbols,
    };
    let mut variables_plan = variable_bindings::Plan::prepare_with_database(
        live(),
        &sources,
        book,
        pack_root,
        database.as_ref(),
    )?;
    variables_plan.bindings = bindings.to_vec();
    plan.variables = Some(variables_plan);
    let output = frame.directory.join("colour-helper-import");
    let report = import910::build(
        BuildInput {
            frames: Some(&donor_frames),
            scripts: &sources,
            book,
            adapter_bytes: include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json"),
            plan: &plan,
            pack_root,
            inames_text: &std::fs::read_to_string(pack_root.join("inames.txt"))?,
            database: database.as_ref(),
            enums: Some(enums),
            variables: Some(live()),
        },
        &output,
    )?;
    ensure!(
        report.child_creation_bindings.len() == usize::from(true),
        "creator has no named frame binding"
    );
    // The no-op creation path preserves selection, so an entry without the
    // matching inherited frame cannot prove ownership for its active helper.
    let mut unbound = plan.clone();
    unbound.entries[0].active_component = None;
    let rejected = import910::build(
        BuildInput {
            frames: Some(&donor_frames),
            scripts: &sources,
            book,
            adapter_bytes: include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json"),
            plan: &unbound,
            pack_root,
            inames_text: &std::fs::read_to_string(pack_root.join("inames.txt"))?,
            database: database.as_ref(),
            enums: Some(enums),
            variables: Some(live()),
        },
        &frame.directory.join("colour-helper-unbound"),
    );
    ensure!(
        rejected.is_err(),
        "creation assumed a new selection on its no-op path"
    );
    let rejection = rejected
        .err()
        .context("creation assumed a new selection on its no-op path")?;
    ensure!(
        format!("{rejection:#}").contains("named frame"),
        "unexpected creation rejection: {rejection:#}"
    );
    ensure!(
        report.font_bindings.len() == writes.len()
            && report.font_map_bindings.len() == FONT_SEED_PCS.len()
            && report
                .font_map_bindings
                .iter()
                .all(
                    |binding| binding.fonts.len() == writes.len() + usize::from(true)
                        && binding.fonts.iter().all(|font| {
                            if font.source_font == constructor.font {
                                font.target_font == constructor.font
                            } else {
                                font.target_font == target_font
                            }
                        })
                )
            && report.font_map_bindings[0].resource_sha256
                == report.font_map_bindings[usize::from(true)].resource_sha256
            && report.font_bindings[0].source_font == source_font
            && report.font_bindings[0].target_font == target_font
            && report.font_bindings[usize::from(true)].source_font == writes[usize::from(true)]
            && report.font_bindings[usize::from(true)].target_font == target_font
            && report.text_property_bindings.len() == TEXT_STYLE_PROPERTY_USES
            && report.initial_font_bindings.len() == FONT_READER_USE_PCS.len()
            && report.initial_font_bindings[0].source_font == initial_font
            && report.initial_font_bindings[0]
                .source_frame
                .as_ref()
                .is_some_and(|contract| contract.frame == source_frame)
            && report.initial_font_bindings[0].target_font == target_font,
        "modern text style has no exact font/property binding"
    );
    let mut wrong_initial = plan.clone();
    wrong_initial
        .entries
        .iter_mut()
        .find(|entry| entry.procedure == font_reader)
        .context("initial font reader entry missing")?
        .active_component = Some(FRAME_SYMBOL.into());
    let rejection = import910::build(
        BuildInput {
            frames: Some(&donor_frames),
            scripts: &sources,
            book,
            adapter_bytes: include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json"),
            plan: &wrong_initial,
            pack_root,
            inames_text: &std::fs::read_to_string(pack_root.join("inames.txt"))?,
            database: database.as_ref(),
            enums: Some(enums),
            variables: Some(live()),
        },
        &frame.directory.join("colour-helper-wrong-initial-font"),
    )
    .err()
    .context("wrong initial font frame was accepted")?;
    ensure!(
        format!("{rejection:#}").contains("declared frame/font"),
        "unexpected initial font frame rejection: {rejection:#}"
    );
    let mut wrong_font = plan.clone();
    wrong_font.font_uses[0].source_font = source_font.wrapping_add(i32::from(true));
    let rejected = import910::build(
        BuildInput {
            frames: Some(&donor_frames),
            scripts: &sources,
            book,
            adapter_bytes: include_bytes!("../../../../revisions/950/cs2/950-1-to-910.json"),
            plan: &wrong_font,
            pack_root,
            inames_text: &std::fs::read_to_string(pack_root.join("inames.txt"))?,
            database: database.as_ref(),
            enums: Some(enums),
            variables: Some(live()),
        },
        &frame.directory.join("colour-helper-wrong-font"),
    );
    let rejection = rejected
        .err()
        .context("unproven font binding was accepted")?;
    ensure!(
        format!("{rejection:#}").contains("pinned source font"),
        "unexpected font binding rejection: {rejection:#}"
    );
    let mut foreign_adapter: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../../revisions/950/cs2/950-1-to-910.json"
    ))?;
    let foreign_domain: String = book.profile.client_md5.chars().rev().collect();
    for rule in foreign_adapter["rules"]
        .as_array_mut()
        .context("adapter rules missing")?
    {
        for operation in rule["operations"]
            .as_object_mut()
            .context("adapter operations missing")?
            .values_mut()
        {
            if let Some(spelling) = operation["host_operation"]
                .as_str()
                .filter(|spelling| spelling.starts_with("component-text/read-font"))
            {
                operation["host_operation"] = spelling
                    .replace(&book.profile.client_md5, &foreign_domain)
                    .into();
            }
        }
    }
    let rejection = import910::build(
        BuildInput {
            frames: Some(&donor_frames),
            scripts: &sources,
            book,
            adapter_bytes: &serde_json::to_vec(&foreign_adapter)?,
            plan: &plan,
            pack_root,
            inames_text: &std::fs::read_to_string(pack_root.join("inames.txt"))?,
            database: database.as_ref(),
            enums: Some(enums),
            variables: Some(live()),
        },
        &frame.directory.join("colour-helper-foreign-font"),
    )
    .err()
    .context("foreign font identity domain was accepted")?;
    ensure!(
        format!("{rejection:#}").contains("different source client"),
        "unexpected font domain rejection: {rejection:#}"
    );
    complete_pack(&output, pack_root)?;
    let style = prepare_created_child_style(frame, &output)?;
    Ok(ColourReplay {
        entry: report.exports["modern_colour"],
        creator: report
            .procedures
            .iter()
            .find(|row| row.source == creator)
            .context("creator unlinked")?
            .target,
        style,
        text_style: report.exports["modern_text_style"],
        target_font,
        font_reads,
        font_seed: report.exports["modern_font_seed"],
        font_reader: report.exports["modern_font_reader"],
        font_frame: i32::try_from(frame.group)? << u16::BITS | i32::try_from(frame.label_file)?,
        initial_font,
        text_alignment,
        max_lines: i32::from(maxlines),
        template: rs910_config::ui_component_fields::TextChildTemplate::decode_resource(
            &constructor.resource(),
        )?,
        created_slot,
        creation_pc: CREATION_PC,
        callback: report
            .procedures
            .iter()
            .find(|row| row.source == callback)
            .context("colour callback unlinked")?
            .target,
        observations,
    })
}

fn prepare_created_child_style(frame: &Frame<'_>, base: &Path) -> anyhow::Result<i32> {
    use anyhow::Context;
    let native910::interface::ComponentBody::Text { mono, .. } = frame.label.body else {
        anyhow::bail!("authoring frame has no text style");
    };
    let scripts = native910::pack::PackArchive::open(&base.join("client.scripts.js5"))?;
    let id = scripts
        .group_ids()
        .max()
        .context("style base has no scripts")?
        + NEXT_SOURCE_ID;
    let directory = frame.directory.join("colour-helper-style-project");
    std::fs::create_dir_all(&directory)?;
    // The donor constructor has zero bounds and an absent font. Supply our
    // own native geometry, plain text and monochrome style after observing
    // those defaults. Imported source consumers supply font and text scalars.
    std::fs::write(directory.join("style.rs2"), format!(
        "[clientscript,modern_child_style](int $child)\npush_constant_string(workshop/frame);\npush_int_local($child);\ncc_find(0);\npop_int_discard(0);\npush_constant_string({});\npush_constant_string({});\npush_constant_string({});\npush_constant_string({});\ncc_setsize(0);\npush_constant_string({});\npush_constant_string({});\npush_constant_string({});\npush_constant_string({});\ncc_setposition(0);\npush_constant_string({});\ncc_setfontmono(0);\npush_constant_string(\"Modern-created theme\");\ncc_settext(0);\nreturn(0);\n",
        frame.label.width, frame.label.height, frame.label.width_mode, frame.label.height_mode,
        frame.label.x, frame.label.y, frame.label.x_mode, frame.label.y_mode, i32::from(mono),
    ))?;
    std::fs::write(
        directory.join("inames.txt"),
        std::fs::read(base.join("inames.txt"))?,
    )?;
    std::fs::write(
        directory.join("project.txt"),
        format!(
            "base-sha256 {}\ninames inames.txt\nscript {id} modern_child_style style.rs2\n",
            project::sha256(&std::fs::read(base.join("client.scripts.js5"))?),
        ),
    )?;
    let output = frame.directory.join("colour-helper-style");
    project::build(&directory.join("project.txt"), base, &output)?;
    complete_pack(&output, base)?;
    Ok(i32::try_from(id)?)
}

pub(super) fn expected_font_binding_failure(
    palette: &PaletteReplay,
    execution: &serde_json::Value,
) -> bool {
    (execution["id"] == palette.helper.font_reader
        && execution["error"]
            .as_str()
            .is_some_and(|error| error.contains(UNBOUND_FONT_IDENTITY_ERROR)))
        || (execution["id"] == palette.helper.font_seed
            && execution["error"]
                .as_str()
                .is_some_and(|error| error.contains(UNMAPPED_FONT_ERROR)))
}

/// Equal asset writes still replace source identity through the delayed packet owner.
fn replay_font_target_write(
    helper: &ColourReplay,
    game: &mut crate::client_game::ClientGame,
    ui: &mut crate::ui_runtime::Runtime,
    clock: &mut crate::test_support::SimClock,
) -> anyhow::Result<serde_json::Value> {
    use anyhow::Context;
    const SMART_OPCODE_HIGH_MARKER: u16 = 1 << (u16::BITS - 1);
    const PENDING_EXPIRY_MARGIN_MS: i64 = 600;
    const FONT_WIRE_PERMUTATION: [usize; 4] = [1, 0, 3, 2];
    let script_packet = |script: i32,
                         argument: Option<i32>,
                         game: &mut crate::client_game::ClientGame,
                         ui: &mut crate::ui_runtime::Runtime,
                         clock: &mut crate::test_support::SimClock|
     -> anyhow::Result<serde_json::Value> {
        let mut payload = if argument.is_some() {
            b"i\0".to_vec()
        } else {
            vec![u8::default()]
        };
        if let Some(argument) = argument {
            payload.extend(argument.to_be_bytes());
        }
        payload.extend(script.to_be_bytes());
        let mut wire = (u16::from(crate::proto::server::RUNCLIENTSCRIPT)
            | SMART_OPCODE_HIGH_MARKER)
            .to_be_bytes()
            .to_vec();
        wire.extend(u16::try_from(payload.len())?.to_be_bytes());
        wire.extend(payload);
        let (packet, consumed) =
            crate::net::decode_frame(&wire)?.context("font script packet incomplete")?;
        assert_eq!(consumed, wire.len());
        let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?
            .context("font script packet not routed")?;
        clock.with(game, |vars| ui.packet(vars, &event))?;
        clock.tick(game, ui)?;
        Ok(ui
            .diagnostics
            .executions
            .iter()
            .rev()
            .find(|row| row["id"] == script)
            .context("font script did not execute")?
            .clone())
    };
    let initial = script_packet(helper.font_reader, None, game, ui, clock)?;
    assert_eq!(initial["ok"], true);
    assert_eq!(
        initial["ints"],
        serde_json::json!(vec![helper.initial_font; FONT_READER_VALUES])
    );
    assert_eq!(
        ui.store
            .get(helper.font_frame, -i32::from(true))?
            .context("initial font frame missing")?
            .borrow()
            .f
            .textfont,
        helper.target_font
    );
    let mut dynamic_reads = Vec::new();
    for source_font in helper
        .font_reads
        .iter()
        .skip(usize::from(true))
        .chain(std::iter::once(&helper.font_reads[0]))
    {
        let seeded = script_packet(helper.font_seed, Some(*source_font), game, ui, clock)?;
        assert_eq!(seeded["ok"], true);
        assert_eq!(
            seeded["ints"],
            serde_json::json!([source_font]),
            "font argument was rewritten outside its consumers"
        );
        let read = script_packet(helper.font_reader, None, game, ui, clock)?;
        assert_eq!(read["ok"], true);
        assert_eq!(
            read["ints"],
            serde_json::json!(vec![source_font; FONT_READER_VALUES])
        );
        dynamic_reads.push(read["ints"].clone());
    }
    let seeded = script_packet(
        helper.font_seed,
        Some(helper.font_reads[usize::from(true)]),
        game,
        ui,
        clock,
    )?;
    assert_eq!(seeded["ok"], true);
    let before = script_packet(helper.font_reader, None, game, ui, clock)?;
    assert_eq!(before["ok"], true);
    assert_eq!(
        before["ints"],
        serde_json::json!(vec![
            helper.font_reads[usize::from(true)];
            FONT_READER_VALUES
        ])
    );
    let unmapped = helper.font_reads[usize::from(true)].wrapping_add(i32::from(true));
    let rejected = script_packet(helper.font_seed, Some(unmapped), game, ui, clock)?;
    assert_eq!(rejected["ok"], false);
    assert!(rejected["error"]
        .as_str()
        .is_some_and(|error| error.contains(UNMAPPED_FONT_ERROR)));
    let retained = script_packet(helper.font_reader, None, game, ui, clock)?;
    assert_eq!(retained["ok"], true);
    assert_eq!(retained["ints"], before["ints"]);
    assert_eq!(
        ui.store
            .get(helper.font_frame, -i32::from(true))?
            .context("font frame missing")?
            .borrow()
            .f
            .textfont,
        helper.target_font
    );
    let mut payload = helper.font_frame.to_le_bytes().to_vec();
    let font = helper.target_font.to_be_bytes();
    payload.extend(FONT_WIRE_PERMUTATION.map(|index| font[index]));
    let mut wire = (u16::from(crate::proto::server::IF_SETTEXTFONT) | SMART_OPCODE_HIGH_MARKER)
        .to_be_bytes()
        .to_vec();
    wire.extend(payload);
    let (packet, consumed) =
        crate::net::decode_frame(&wire)?.context("font target write packet incomplete")?;
    assert_eq!(consumed, wire.len());
    let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?
        .context("font target write packet not routed")?;
    clock.with(game, |vars| ui.packet(vars, &event))?;
    clock.0 += PENDING_EXPIRY_MARGIN_MS;
    clock.tick(game, ui)?;
    let after = script_packet(helper.font_reader, None, game, ui, clock)?;
    assert_eq!(after["ok"], false);
    assert!(
        after["error"]
            .as_str()
            .is_some_and(|error| error.contains(UNBOUND_FONT_IDENTITY_ERROR)),
        "unexpected font interop failure: {after}"
    );
    assert_eq!(
        ui.store
            .get(helper.font_frame, -i32::from(true))?
            .context("font frame missing")?
            .borrow()
            .f
            .textfont,
        helper.target_font
    );
    assert_eq!(
        script_packet(
            helper.font_seed,
            Some(helper.font_reads[usize::from(true)]),
            game,
            ui,
            clock
        )?["ok"],
        true
    );
    let restored = script_packet(helper.font_reader, None, game, ui, clock)?;
    assert_eq!(restored["ok"], true);
    assert_eq!(restored["ints"], before["ints"]);
    Ok(
        serde_json::json!({"target_font":helper.target_font,"initial":initial["ints"],"dynamic_reads":dynamic_reads,"unmapped_error":rejected["error"],"retained_after_unmapped":retained["ints"],"before":before["ints"],"after_error":after["error"],"restored":restored["ints"]}),
    )
}

pub(super) fn replay_colour_helper(
    palette: &PaletteReplay,
    game: &mut crate::client_game::ClientGame,
    ui: &mut crate::ui_runtime::Runtime,
    clock: &mut crate::test_support::SimClock,
    frame: i32,
    directory: &Path,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use native910::{vars::VarScope, vm::Value};
    const STATIC_COMPONENT: i32 = -1;
    const COLOUR_CHANNELS: usize = 3;
    const PENDING_EXPIRY_MARGIN_MS: i64 = 600;
    const NO_CHANGE_WAIT_MS: i64 = 20;
    const SMART_OPCODE_HIGH_MARKER: u16 = 1 << (u16::BITS - 1);
    let helper = &palette.helper;
    let parent = ui
        .store
        .get(frame, STATIC_COMPONENT)?
        .context("colour frame missing")?;
    assert!(parent
        .borrow()
        .runtime_child(crate::ui_components::RuntimeChildId::new(
            helper.created_slot
        )?)
        .is_none());
    let initial = &helper.observations[0];
    clock.with(game, |variables| {
        variables.set(
            VarScope::Player,
            palette.player,
            false,
            Value::Int(initial.player),
        )?;
        variables.set(
            VarScope::Client,
            palette.client,
            false,
            Value::Int(initial.client),
        )
    })?;
    let mut payload = b"i\0".to_vec();
    payload.extend(initial.key.to_be_bytes());
    payload.extend(helper.entry.to_be_bytes());
    let mut wire = (u16::from(crate::proto::server::RUNCLIENTSCRIPT) | SMART_OPCODE_HIGH_MARKER)
        .to_be_bytes()
        .to_vec();
    wire.extend(u16::try_from(payload.len())?.to_be_bytes());
    wire.extend(payload);
    let (packet, consumed) =
        crate::net::decode_frame(&wire)?.context("colour packet incomplete")?;
    assert_eq!(consumed, wire.len());
    let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?
        .context("colour packet not routed")?;
    clock.with(game, |vars| ui.packet(vars, &event))?;
    clock.tick(game, ui)?;
    assert!(
        ui.diagnostics
            .executions
            .iter()
            .any(|row| row["id"] == helper.entry && row["ok"] == true),
        "colour entry execution: {:?}",
        ui.diagnostics.executions.last()
    );
    let component = ui
        .store
        .get(frame, i32::from(helper.created_slot))?
        .context("modern-created child missing")?;
    {
        let child = component.borrow();
        let origin = child
            .creation_origin
            .as_ref()
            .context("modern child origin missing")?;
        assert_eq!(
            (origin.script, origin.instruction),
            (Some(helper.creator), helper.creation_pc)
        );
        assert!(child.has_runtime_parent());
        assert_eq!(child.f.textfont, helper.template.font);
        assert!(!child.f.fontmono && !child.f.textshadow && !child.f.textantimacro);
        assert!(child.f.text.as_ref().is_some_and(Vec::is_empty));
    }
    assert_eq!(component.borrow().f.colour, initial.colour);
    let mut payload = b"i\0".to_vec();
    payload.extend(i32::from(helper.created_slot).to_be_bytes());
    payload.extend(helper.style.to_be_bytes());
    let mut wire = (u16::from(crate::proto::server::RUNCLIENTSCRIPT) | SMART_OPCODE_HIGH_MARKER)
        .to_be_bytes()
        .to_vec();
    wire.extend(u16::try_from(payload.len())?.to_be_bytes());
    wire.extend(payload);
    let (packet, consumed) =
        crate::net::decode_frame(&wire)?.context("child style packet incomplete")?;
    assert_eq!(consumed, wire.len());
    let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?
        .context("child style packet not routed")?;
    clock.with(game, |vars| ui.packet(vars, &event))?;
    clock.tick(game, ui)?;
    assert!(ui
        .diagnostics
        .executions
        .iter()
        .any(|row| row["id"] == helper.style && row["ok"] == true));
    let mut payload = b"i\0".to_vec();
    payload.extend(i32::from(helper.created_slot).to_be_bytes());
    payload.extend(helper.text_style.to_be_bytes());
    let mut wire = (u16::from(crate::proto::server::RUNCLIENTSCRIPT) | SMART_OPCODE_HIGH_MARKER)
        .to_be_bytes()
        .to_vec();
    wire.extend(u16::try_from(payload.len())?.to_be_bytes());
    wire.extend(payload);
    let (packet, consumed) =
        crate::net::decode_frame(&wire)?.context("modern text style packet incomplete")?;
    assert_eq!(consumed, wire.len());
    let event = crate::session::parse_ui_event(packet.opcode, &packet.payload)?
        .context("modern text style packet not routed")?;
    clock.with(game, |vars| ui.packet(vars, &event))?;
    clock.tick(game, ui)?;
    assert!(
        ui.diagnostics
            .executions
            .iter()
            .any(|row| row["id"] == helper.text_style
                && row["ok"] == true
                && row["ints"] == serde_json::json!(helper.font_reads)),
        "modern text style execution: {:?}",
        ui.diagnostics.executions.last()
    );
    {
        let child = component.borrow();
        assert_eq!(child.f.textfont, helper.target_font);
        assert_eq!(
            [
                child.f.textHAlign,
                child.f.textVAlign,
                child.f.textLineHeight
            ],
            helper.text_alignment
        );
        assert_eq!(child.f.maxlines, helper.max_lines);
    }
    assert!(
        component.borrow().f.width > i32::default() && component.borrow().f.height > i32::default()
    );
    assert!(component.borrow().retained_player_transmit.is_some());
    assert_eq!(
        component.borrow().transmits["onvartransmitlist"],
        vec![i32::from(palette.player)]
    );
    let font_packet_observation = replay_font_target_write(helper, game, ui, clock)?;
    let mut observations = vec![
        serde_json::json!({"input":initial,"actual_colour":component.borrow().f.colour,"entry":helper.entry,"creator":helper.creator,"creation_pc":helper.creation_pc,"child":helper.created_slot,"text_style":helper.text_style,"font":helper.target_font,"source_font_reads":helper.font_reads,"font_target_write":font_packet_observation,"alignment":helper.text_alignment,"max_lines":helper.max_lines}),
    ];
    for expected in &helper.observations[usize::from(true)..] {
        let before = ui.diagnostics.executions.len();
        // Feed the same strictly framed varp packet as the live session.
        let mut wire = vec![crate::proto::server::VARP_LARGE];
        wire.extend(palette.player.to_le_bytes());
        wire.extend(expected.player.to_be_bytes());
        let (packet, consumed) =
            crate::net::decode_frame(&wire)?.context("colour variable packet incomplete")?;
        assert_eq!(consumed, wire.len());
        assert!(game.runtime.feed.enqueue(packet.opcode, &packet.payload));
        assert!(game
            .apply_next(clock.0)
            .map_err(|error| anyhow::anyhow!("colour variable packet: {error:?}"))?
            .is_some());
        clock.0 += PENDING_EXPIRY_MARGIN_MS;
        game.game
            .poll_vars(|| clock.0)
            .map_err(|error| anyhow::anyhow!("colour variable poll: {error:?}"))?;
        clock.tick(game, ui)?;
        assert_eq!(
            component.borrow().f.colour,
            expected.colour,
            "child callback: {:?}",
            &ui.diagnostics.executions[before..]
        );
        assert!(ui.diagnostics.executions[before..]
            .iter()
            .any(|row| row["id"] == helper.callback && row["ok"] == true));
        ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        let painted = ui.paint(game.cycle, false, [f32::default(); COLOUR_CHANNELS])?;
        let [_, red, green, blue] = expected.colour.to_be_bytes();
        let rgb = [red, green, blue];
        assert!(
            painted.paint.quads.iter().any(|quad| matches!(
                quad.image,
                rs910_toolkit::ui_paint::Image::Font(_)
            ) && quad
                .vertices
                .iter()
                .all(|vertex| vertex.colour[..COLOUR_CHANNELS] == rgb)),
            "modern child colour did not reach text glyphs"
        );
        observations.push(serde_json::json!({"input":expected,"actual_colour":component.borrow().f.colour,"callback":helper.callback,"child":helper.created_slot,"painted_rgb":rgb}));
        let after = ui.diagnostics.executions.len();
        clock.0 += NO_CHANGE_WAIT_MS;
        clock.tick(game, ui)?;
        assert_eq!(ui.diagnostics.executions.len(), after);
    }
    std::fs::write(
        directory.join("colour-helper-observations.json"),
        serde_json::to_vec_pretty(&observations)?,
    )?;
    assert!(ui.store.remove_runtime_child(
        &parent,
        crate::ui_components::RuntimeChildId::new(helper.created_slot)?
    ));
    Ok(())
}
