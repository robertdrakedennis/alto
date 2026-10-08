mod common;
use native910::opcode::OpcodeBook;
use native910::pack::PackArchive;
use native910::project;
use native910::project::sha256;
use native910::runtime::RuntimeHost;
use native910::script::decode_script;
use native910::vm::{Value, Vm};
use std::collections::HashMap;

#[test]
#[ignore = "requires provisioned script and config packs"]
fn project_links_new_value_calls_and_publishes_immutable_builds() {
    const ARGUMENT_SLOTS: u16 = 2;
    let root = common::native_dir();
    let packs = common::pack_root();
    let base = std::fs::read(packs.join("client.scripts.js5")).expect("required corpus");
    let dir = root.join(format!("target/project-test-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let manifest = dir.join("project.txt");
    std::fs::write(
        &manifest,
        format!(
            "base-sha256 {}\nscript 200000 main main.rs2\nscript 200001 compute compute.rs2\n",
            sha256(&base)
        ),
    )
    .unwrap();
    std::fs::write(dir.join("main.rs2"), "[clientscript,main]()\nint $result;\n$result = ~compute(0, 20 + 21);\npush_int_local($result);\nreturn(0);\n").unwrap();
    std::fs::write(
        dir.join("compute.rs2"),
        "[proc,compute](int $selector,int $x)\npush_int_local($selector);\nbranch_if_true(multiple);\npush_int_local($x);\npush_constant_int(1);\nadd(0);\nreturn(0);\nmultiple:\npush_int_local($x);\npush_constant_int(1);\nadd(0);\npush_constant_int(99);\nreturn(0);\n",
    )
    .unwrap();
    assert_eq!(
        project::build(&manifest, &packs, &dir.join("build1")).unwrap(),
        2
    );
    assert_eq!(
        project::build(&manifest, &packs, &dir.join("build2")).unwrap(),
        2
    );
    assert_eq!(
        std::fs::read(dir.join("build1/client.scripts.js5")).unwrap(),
        std::fs::read(dir.join("build2/client.scripts.js5")).unwrap()
    );
    assert!(project::build(&manifest, &packs, &dir.join("build1")).is_err());
    let book = OpcodeBook::embedded().unwrap();
    let archive = PackArchive::open(&dir.join("build1/client.scripts.js5")).unwrap();
    let main = decode_script(&archive.group_files(200_000).unwrap().unwrap()[&0], &book).unwrap();
    let callee = decode_script(&archive.group_files(200_001).unwrap().unwrap()[&0], &book).unwrap();
    assert_eq!(
        callee.locals.int, ARGUMENT_SLOTS,
        "header includes the argument slots"
    );
    let symbols = native910::symbols::SymbolRegistry::parse_symbols_txt(
        &std::fs::read_to_string(dir.join("build1/symbols.txt")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        symbols.returns_of(200_001),
        None,
        "call-specific proof remains out of universal signatures"
    );
    let provider = HashMap::from([(200_001, callee)]);
    assert_eq!(
        Vm::new(&mut RuntimeHost::default(), &provider)
            .execute(&main, &[])
            .unwrap(),
        Some(Value::Int(42))
    );
    // An unedited cached helper must be available before the only authored
    // caller compiles, including a helper with argument-dependent results.
    let cached_packs = dir.join("cached-packs");
    std::fs::create_dir(&cached_packs).unwrap();
    std::fs::copy(
        dir.join("build1/client.scripts.js5"),
        cached_packs.join("client.scripts.js5"),
    )
    .unwrap();
    for name in ["client.config.js5", "client.enum.config.js5"] {
        std::fs::copy(packs.join(name), cached_packs.join(name)).unwrap();
    }
    let cached_base = std::fs::read(cached_packs.join("client.scripts.js5")).unwrap();
    std::fs::write(dir.join("names.txt"), "script 200001 compute\n").unwrap();
    let cached_manifest = dir.join("cached-project.txt");
    std::fs::write(
        &cached_manifest,
        format!(
            "base-sha256 {}\nnames names.txt\nscript 200000 main main.rs2\n",
            sha256(&cached_base)
        ),
    )
    .unwrap();
    let cached_build = dir.join("cached-build");
    assert_eq!(
        project::build(&cached_manifest, &cached_packs, &cached_build).unwrap(),
        1
    );
    let rebuilt = PackArchive::open(&cached_build.join("client.scripts.js5")).unwrap();
    let cached_main =
        decode_script(&rebuilt.group_files(200_000).unwrap().unwrap()[&0], &book).unwrap();
    assert_eq!(
        Vm::new(&mut RuntimeHost::default(), &provider)
            .execute(&cached_main, &[])
            .unwrap(),
        Some(Value::Int(42))
    );
    assert_eq!(
        rebuilt.group_container(200_001),
        archive.group_container(200_001)
    );
    std::fs::write(
        dir.join("main.rs2"),
        "[clientscript,main]()\nint $result;\n$result = ~compute(1,41);\nreturn(0);\n",
    )
    .unwrap();
    let error = project::build(&manifest, &packs, &dir.join("multiple-return")).unwrap_err();
    assert!(
        error.to_string().contains("exactly one value required"),
        "{error}"
    );
    assert!(!dir.join("multiple-return").exists());
    std::fs::write(
        dir.join("main.rs2"),
        "[clientscript,main]()\ngosub_with_params(999999);\nreturn(0);\n",
    )
    .unwrap();
    assert!(project::build(&manifest, &packs, &dir.join("invalid")).is_err());
    assert!(!dir.join("invalid").exists());
    assert_eq!(
        std::fs::read(packs.join("client.scripts.js5")).unwrap(),
        base
    );
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn ui_project_links_symbolic_components_and_checks_hook_signatures() {
    const NEXT_ID: u32 = 1;
    const FIRST_FILE: u32 = 0;
    const SOURCE_COUNT: usize = 2;
    const NO_PARENT: i32 = -1;
    let packs = common::require_pack(&[
        "client.scripts.js5",
        "client.interfaces.js5",
        "client.config.js5",
    ]);
    let scripts = PackArchive::open(&packs.join("client.scripts.js5")).unwrap();
    let interfaces = PackArchive::open(&packs.join("client.interfaces.js5")).unwrap();
    let script_id = scripts.group_ids().max().unwrap() + NEXT_ID;
    let group = interfaces.group_ids().max().unwrap() + NEXT_ID;
    let bytes = interfaces
        .group_ids()
        .find_map(|id| {
            interfaces
                .group_files(id)
                .unwrap()?
                .into_values()
                .find(|bytes| {
                    native910::interface::decode_component(bytes, i32::default()).is_ok_and(
                        |component| {
                            matches!(
                                component.body,
                                native910::interface::ComponentBody::Text { .. }
                            )
                        },
                    )
                })
        })
        .unwrap();
    let mut component = native910::interface::decode_component(&bytes, i32::default()).unwrap();
    component.layer = NO_PARENT;
    component.hooks = native910::interface::Hooks {
        onload: Some(vec![native910::interface::HookArg::Int(
            script_id.try_into().unwrap(),
        )]),
        ..Default::default()
    };
    component.transmits = native910::interface::Transmits::default();
    let source = native910::isource::format_component(
        &native910::isource::lift_component(&component),
        &native910::symbols::SymbolRegistry::default(),
    );
    let directory =
        common::native_dir().join(format!("target/ui-project-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join("project.txt");
    let base = std::fs::read(packs.join("client.scripts.js5")).unwrap();
    let interface_base = std::fs::read(packs.join("client.interfaces.js5")).unwrap();
    std::fs::write(&manifest, format!("base-sha256 {}\ninterfaces-sha256 {}\ninames inames.txt\nscript {script_id} initialize init.rs2\ncomponent {group} {FIRST_FILE} title.ifc\n", sha256(&base), sha256(&interface_base))).unwrap();
    std::fs::write(
        directory.join("inames.txt"),
        format!("interface {group} workshop\ncomponent {group} {FIRST_FILE} title\n"),
    )
    .unwrap();
    std::fs::write(directory.join("title.ifc"), &source).unwrap();
    std::fs::write(directory.join("init.rs2"), "[clientscript,initialize]()\npush_constant_string(\"Authored UI\");\npush_constant_string(workshop/title);\nif_settext(0);\nreturn(0);\n").unwrap();
    let output = directory.join("build");
    assert_eq!(
        project::build(&manifest, &packs, &output).unwrap(),
        SOURCE_COUNT
    );
    let rebuilt = PackArchive::open(&output.join("client.interfaces.js5")).unwrap();
    let published = native910::interface::decode_component(
        &rebuilt.group_files(group).unwrap().unwrap()[&FIRST_FILE],
        i32::default(),
    )
    .unwrap();
    assert_eq!(published, component);
    for id in interfaces.group_ids() {
        assert_eq!(rebuilt.group_container(id), interfaces.group_container(id));
    }
    assert!(
        std::fs::read_to_string(output.join("build.txt"))
            .unwrap()
            .contains("interfaces-output-sha256")
    );
    assert!(
        std::fs::read_to_string(output.join("stack-analysis.tsv"))
            .unwrap()
            .contains("complete")
    );
    // Hook entry accepts short tails and leaves unused typed locals at their
    // defaults. Those banks include argument slots; longs need no wire tail.
    let script_source = std::fs::read_to_string(directory.join("init.rs2")).unwrap();
    let optional_source = script_source.replace(
        "[clientscript,initialize]()",
        "[clientscript,initialize](int $optional,string $optional_text,long $optional_long)",
    );
    std::fs::write(directory.join("init.rs2"), &optional_source).unwrap();
    project::build(&manifest, &packs, &directory.join("short-tail")).unwrap();
    std::fs::write(directory.join("init.rs2"), &script_source).unwrap();
    component
        .hooks
        .onload
        .as_mut()
        .unwrap()
        .push(native910::interface::HookArg::Int(i32::default()));
    std::fs::write(
        directory.join("title.ifc"),
        native910::isource::format_component(
            &native910::isource::lift_component(&component),
            &native910::symbols::SymbolRegistry::default(),
        ),
    )
    .unwrap();
    let invalid = directory.join("invalid");
    assert!(
        project::build(&manifest, &packs, &invalid)
            .unwrap_err()
            .to_string()
            .contains("local capacity")
    );
    assert!(!invalid.exists());
    std::fs::write(directory.join("title.ifc"), &source).unwrap();
    std::fs::write(
        directory.join("init.rs2"),
        "[clientscript,initialize]()\nif_settext(0);\nreturn(0);\n",
    )
    .unwrap();
    let underflow = directory.join("underflow");
    assert!(
        project::build(&manifest, &packs, &underflow)
            .unwrap_err()
            .to_string()
            .contains("StackUnderflow")
    );
    assert!(!underflow.exists());
    std::fs::write(directory.join("init.rs2"), &script_source).unwrap();
    component.hooks.onload = Some(vec![native910::interface::HookArg::Int(
        script_id.try_into().unwrap(),
    )]);
    let own_address = native910::xref::pack_component(native910::xref::ComponentRef {
        iface: group.try_into().unwrap(),
        child: FIRST_FILE.try_into().unwrap(),
    })
    .unwrap();
    for (name, parent, expected) in [
        ("cycle", own_address, "cyclic layer"),
        ("orphan", own_address + NEXT_ID as i32, "missing layer"),
    ] {
        component.layer = parent;
        std::fs::write(
            directory.join("title.ifc"),
            native910::isource::format_component(
                &native910::isource::lift_component(&component),
                &native910::symbols::SymbolRegistry::default(),
            ),
        )
        .unwrap();
        let rejected = directory.join(name);
        assert!(
            project::build(&manifest, &packs, &rejected)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert!(!rejected.exists());
    }
    let cached_callee = interfaces
        .group_ids()
        .find_map(|group| {
            interfaces
                .group_files(group)
                .unwrap()?
                .into_values()
                .find_map(|bytes| {
                    let cached =
                        native910::interface::decode_component(&bytes, i32::default()).unwrap();
                    native910::isource::hook_slots(&cached.hooks)
                        .into_iter()
                        .find_map(|(_, hook)| {
                            let args = hook.as_ref()?;
                            let (native910::interface::HookArg::Int(id), tail) =
                                args.split_first()?
                            else {
                                return None;
                            };
                            (!tail.is_empty()).then_some(*id)
                        })
                })
        })
        .unwrap();
    let incompatible = directory.join("cached-contract.txt");
    std::fs::write(
        &incompatible,
        format!(
            "base-sha256 {}\nscript {cached_callee} broken broken.rs2\n",
            sha256(&base)
        ),
    )
    .unwrap();
    std::fs::write(
        directory.join("broken.rs2"),
        "[clientscript,broken]()\nreturn(0);\n",
    )
    .unwrap();
    let rejected = directory.join("cached-contract");
    let error = project::build(&incompatible, &packs, &rejected)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cached component") && error.contains("local capacity"));
    assert!(!rejected.exists());
    // A supplied interface pin is authoritative even without component edits.
    // The published interface image and cached-hook checks share that input.
    const SCRIPT_ONLY_SOURCE_COUNT: usize = 1;
    let pinned_manifest = directory.join("pinned-script-only.txt");
    let pinned_output = directory.join("pinned-script-only");
    std::fs::write(
        directory.join("init.rs2"),
        "[clientscript,initialize]()\nreturn(0);\n",
    )
    .unwrap();
    let pinned_text = |interface_hash: String| {
        format!(
            "base-sha256 {}\ninterfaces-sha256 {interface_hash}\nscript {script_id} initialize init.rs2\n",
            sha256(&base)
        )
    };
    std::fs::write(&pinned_manifest, pinned_text(sha256(&base))).unwrap();
    assert!(
        project::build(&pinned_manifest, &packs, &pinned_output)
            .unwrap_err()
            .to_string()
            .contains("interfaces-sha256")
    );
    assert!(!pinned_output.exists());
    std::fs::write(&pinned_manifest, pinned_text(sha256(&interface_base))).unwrap();
    assert_eq!(
        project::build(&pinned_manifest, &packs, &pinned_output).unwrap(),
        SCRIPT_ONLY_SOURCE_COUNT
    );
    assert_eq!(
        std::fs::read(pinned_output.join("client.interfaces.js5")).unwrap(),
        interface_base
    );
    assert_eq!(
        std::fs::read(packs.join("client.interfaces.js5")).unwrap(),
        interface_base
    );
}
