//! M2b gate: editable source text for 910 interface components.
//!
//! * `corpus_text_roundtrip_is_byte_exact` — the FULL corpus gate: every component in the
//!   real 910 interfaces pack must survive decode → lift → format → parse → lower →
//!   encode byte-identical under an EMPTY registry, with format a fixpoint throughout.
//! * `named_hook_renders_and_reassembles` — a real hook callee from the corpus under a
//!   synthetic registry: the head must render as `~name` and reassemble byte-identical.
//! * `hook_names_fold_head_only` — the pack-free mechanism check: only head (callee)
//!   ints fold, everything else stays numeric, and the model round-trips.
//! * `parse_errors_carry_line_numbers` — every parse failure names its 1-based line.
//! * `canonical_text_is_a_fixpoint` — one distinctive component per primitive type:
//!   parse ∘ format is the identity and re-format is stable, comments and blank lines
//!   included.
//!
//! Corpus tests fail loudly without server/data/pack; `--features no-pack`
//! reports them ignored.

mod common;

use native910::inames::InterfaceRegistry;
use native910::interface::{ComponentBody, HookArg, Hooks, Keybind, TargetSection, Transmits};
use native910::pack::PackArchive;
use native910::script::Counts;
use native910::symbols::SymbolRegistry;
use native910::{iparse, isource};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn interfaces_pack() -> PathBuf {
    common::pack_root().join("client.interfaces.js5")
}

/// One component through the full text pipeline: lift, format, reparse (model equality
/// plus format fixpoint), then assemble (parse → lower → encode → verify) byte-exact.
fn check_component(
    bytes: &[u8],
    parentlayer: i32,
    symbols: &SymbolRegistry,
) -> std::result::Result<(), String> {
    let component = isource::lift_component(
        &native910::interface::decode_component(bytes, parentlayer)
            .map_err(|error| format!("decode: {error}"))?,
    );
    let text = isource::format_component(&component, symbols);
    let parsed = iparse::parse_component_source(&text, symbols)
        .map_err(|error| format!("reparse: {error}"))?;
    if parsed != component {
        return Err("lift/parse model mismatch".to_string());
    }
    if isource::format_component(&parsed, symbols) != text {
        return Err("format not a fixpoint".to_string());
    }
    let out = isource::assemble_component(&text, parentlayer, symbols, &InterfaceRegistry::empty())
        .map_err(|error| format!("assemble: {error}"))?;
    if out != bytes {
        return Err(format!(
            "reassembled {} bytes, original {}",
            out.len(),
            bytes.len()
        ));
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_text_roundtrip_is_byte_exact() {
    let path = interfaces_pack();
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open interfaces pack");
    let symbols = SymbolRegistry::empty();

    let mut groups = 0_usize;
    let mut components = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for group in archive.group_ids() {
        groups += 1;
        let parentlayer = (group << 16) as i32;
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            components += 1;
            if let Err(error) = check_component(bytes, parentlayer, &symbols) {
                failures.push(format!("{group}/{file}: {error}"));
            }
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} interface text failure(s) over {components} components in {groups} groups",
        failures.len()
    );
    eprintln!("interface text corpus: {components} components in {groups} groups, byte-exact");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn named_hook_renders_and_reassembles() {
    let path = interfaces_pack();
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open interfaces pack");

    // First component with a hook headed by an int: a REAL callee id from the corpus.
    let mut found = None;
    for group in archive.group_ids() {
        let parentlayer = (group << 16) as i32;
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            let component = native910::interface::decode_component(bytes, parentlayer)
                .expect("decode corpus component");
            for (hook_name, hook) in isource::hook_slots(&component.hooks) {
                if let Some(args) = hook
                    && let Some(HookArg::Int(head)) = args.first()
                {
                    found = Some((group, *file, parentlayer, bytes.clone(), *head, hook_name));
                }
                if found.is_some() {
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let Some((group, file, parentlayer, bytes, callee, hook_name)) = found else {
        panic!("corpus has no hook-headed component");
    };

    let known = BTreeMap::from([(callee, Counts::default())]);
    let registry = SymbolRegistry::build(vec![(callee, "spot_callee".to_string())], &known)
        .expect("build synthetic registry");

    let component = native910::interface::decode_component(&bytes, parentlayer).expect("decode");
    let source = isource::lift_component(&component);
    let text = isource::format_component(&source, &registry);
    assert!(
        text.contains("~spot_callee"),
        "hook {hook_name} of {group}/{file} did not fold to ~spot_callee"
    );
    // The folded head parses back through the registry to the same model.
    let parsed = iparse::parse_component_source(&text, &registry).expect("parse named-hook text");
    assert_eq!(parsed, source);
    let out =
        isource::assemble_component(&text, parentlayer, &registry, &InterfaceRegistry::empty())
            .expect("assemble");
    assert_eq!(
        out, bytes,
        "named-hook reassembly differs for {group}/{file}"
    );

    // The same component under the empty registry stays numeric and still verifies.
    let plain = isource::format_component(&source, &SymbolRegistry::empty());
    assert!(!plain.contains('~'), "empty registry must render no names");
    let plain_out = isource::assemble_component(
        &plain,
        parentlayer,
        &SymbolRegistry::empty(),
        &InterfaceRegistry::empty(),
    )
    .expect("assemble numeric");
    assert_eq!(plain_out, bytes);
    eprintln!(
        "named-hook spot check: {group}/{file} {hook_name} callee {callee} as ~spot_callee, byte-exact"
    );
}

/// Pack-free mechanism check: only the head (callee) int folds to `~name`; a non-head
/// int with the same value, and heads the registry does not know, stay numeric.
#[test]
fn hook_names_fold_head_only() {
    let callee = 4242;
    let known = BTreeMap::from([(callee, Counts::default())]);
    let registry = SymbolRegistry::build(vec![(callee, "quest_complete".to_string())], &known)
        .expect("build synthetic registry");

    let mut source = base_source(
        isource::ComponentType::Layer,
        ComponentBody::Layer {
            scroll_width: 0,
            scroll_height: 0,
        },
    );
    source.hooks.onclick = Some(vec![
        HookArg::Int(callee),
        HookArg::Int(callee),
        HookArg::Str("x".to_string()),
    ]);
    source.hooks.onload = Some(vec![HookArg::Int(999)]);

    let text = isource::format_component(&source, &registry);
    assert!(
        text.contains("hook onclick [~quest_complete, 4242, \"x\"];"),
        "head-only folding wrong:\n{text}"
    );
    assert!(
        text.contains("hook onload [999];"),
        "unknown callee must stay numeric:\n{text}"
    );
    let parsed = iparse::parse_component_source(&text, &registry).expect("parse");
    assert_eq!(parsed, source);
    let bytes = isource::assemble_component(&text, 0, &registry, &InterfaceRegistry::empty())
        .expect("assemble");
    let expected = native910::interface::encode_component(
        &isource::lower_component(&source).expect("lower"),
        0,
    )
    .expect("encode");
    assert_eq!(bytes, expected);
}

#[test]
fn parse_errors_carry_line_numbers() {
    let symbols = SymbolRegistry::empty();
    let base = minimal_layer();
    // (mutated text, expected line, expected message fragment)
    let rectangle = set_line(&base, 1, "component rectangle;");
    let rectangle = set_line(&rectangle, 12, "colour 0xFF0000;\nfill true;\ntrans 0;");
    let cases: Vec<(String, usize, &str)> = vec![
        ("pos 0 0;\n".to_string(), 1, "missing component header"),
        (
            set_line(&base, 1, "component circle;"),
            1,
            "unknown component type",
        ),
        (set_line(&base, 2, "version 99;"), 2, "unsupported version"),
        (
            set_line(&base, 10, "hide yes;"),
            10,
            "expected true or false",
        ),
        (set_line(&base, 4, "clientcode abc;"), 4, "bad clientcode"),
        (set_line(&base, 5, "pos 0 0"), 5, "misses its ';'"),
        (set_line(&base, 15, "opbase \"abc;"), 15, "bad string"),
        (set_line(&base, 15, "opbase \"a\\qb\";"), 15, "bad escape"),
        (set_line(&base, 17, "opnames 99;"), 17, "out of range"),
        (set_line(&rectangle, 12, "colour red;"), 12, "bad colour"),
        (
            set_line(&base, 12, "colour 0xFF0000;"),
            12,
            "unknown field 'colour'",
        ),
        (
            push_line(&base, "frobnicate 1;"),
            25,
            "unknown field 'frobnicate'",
        ),
        (
            push_line(&base, "version 5;"),
            25,
            "duplicate field 'version'",
        ),
        (
            push_line(&base, "hook onfrobnicate none;"),
            25,
            "unknown hook 'onfrobnicate'",
        ),
        (
            push_line(&base, "transmit foo none;"),
            25,
            "unknown transmit list 'foo'",
        ),
        (
            push_line(&base, "hook onclick [~ghost];"),
            25,
            "unknown script '~ghost'",
        ),
        (
            push_line(
                &push_line(&base, "hook onclick none;"),
                "hook onclick none;",
            ),
            26,
            "duplicate field 'hook onclick'",
        ),
        (
            drop_line(&base, 12),
            drop_line(&base, 12).lines().count(),
            "missing required field 'scroll'",
        ),
    ];
    for (text, line, fragment) in &cases {
        let error = iparse::parse_component_source(text, &symbols)
            .expect_err(&format!("expected failure containing '{fragment}'"));
        let message = error.to_string();
        assert!(
            message.contains(&format!("line {line}:")),
            "wrong line in '{message}' (expected line {line})"
        );
        assert!(
            message.contains(fragment),
            "missing '{fragment}' in '{message}'"
        );
    }
    eprintln!("parse errors: {} cases name their lines", cases.len());

    // Decimal colours are accepted (the formatter canonicalizes them to hex).
    let decimal = format!("colour {};", 0xFF_0000);
    let parsed = iparse::parse_component_source(&set_line(&rectangle, 12, &decimal), &symbols)
        .expect("parse decimal colour");
    let text = isource::format_component(&parsed, &symbols);
    assert!(
        text.contains("colour 0xFF0000;"),
        "decimal colour not canonicalized"
    );
}

#[test]
fn canonical_text_is_a_fixpoint() {
    let symbols = SymbolRegistry::empty();
    let sources = all_type_sources();
    assert_eq!(sources.len(), 7);
    for source in &sources {
        let text = isource::format_component(source, &symbols);
        let parsed = iparse::parse_component_source(&text, &symbols).expect("parse");
        assert_eq!(&parsed, source, "parse o format drift:\n{text}");
        let again = isource::format_component(&parsed, &symbols);
        assert_eq!(again, text, "format not a fixpoint:\n{text}");
        let lowered = isource::lower_component(&parsed).expect("lower");
        assert_eq!(lowered, isource::lower_component(source).expect("lower"));
        // Byte pipeline verifies on the hand models too (all use layer -1, so
        // parentlayer 0 encodes and re-decodes exactly).
        isource::assemble_component(&text, 0, &symbols, &InterfaceRegistry::empty())
            .expect("assemble");
    }

    // Comments, blank lines, and trailing remarks are insignificant; hostile game text
    // (escapes, `//`, `;`, commas) inside strings survives them.
    let source = &sources[2];
    let text = isource::format_component(source, &symbols);
    let decorated = format!(
        "// interface 7 component 42\n\n{}\n// trailing remark\n",
        text.replace("version 4;", "version 4; // the version")
    );
    let parsed = iparse::parse_component_source(&decorated, &symbols).expect("parse decorated");
    assert_eq!(&parsed, source);
    eprintln!(
        "canonical stability: {} component types are fixpoints",
        sources.len()
    );
}

/// One distinctive component per primitive type (plus an extended-block model), pushing
/// every text feature: hex colours, escapes, keybinds, op-names, params, transmits, a
/// keymask-selected target, aspect modes, and version edges (-1, 3, 4, 5).
fn all_type_sources() -> Vec<isource::ComponentSource> {
    let mut layer = base_source(
        isource::ComponentType::Layer,
        ComponentBody::Layer {
            scroll_width: 300,
            scroll_height: 400,
        },
    );
    // Version -1 exercises the legacy noclickthrough byte; params/mouseover stay clear.
    layer.version = -1;
    layer.hide = true;
    layer.noclickthrough = true;
    layer.keybinds = vec![Keybind {
        head: 0x10,
        lo: 5,
        key: 27,
        mods: 1,
    }];
    layer.ops = vec!["Take".to_string()];
    layer.opname_nibble = 1;
    layer.opname_first = Some((0, 7));
    layer.pausetext = Some("wait...".to_string());
    layer.hooks.onclick = Some(vec![HookArg::Int(5690), HookArg::Str("x".to_string())]);
    layer.transmits.var = Some(vec![1752]);

    let mut rectangle = base_source(
        isource::ComponentType::Rectangle,
        ComponentBody::Rectangle {
            colour: 0xFF_00FF,
            fill: true,
            trans: 128,
        },
    );
    rectangle.version = 3;
    rectangle.opbase = "base//x".to_string();

    let mut text = base_source(
        isource::ComponentType::Text,
        ComponentBody::Text {
            font: 5,
            mono: true,
            text: "say \"hi\"\n// bye; ok, \\done\\\r\t".to_string(),
            line_height: 12,
            halign: 1,
            valign: 2,
            shadow: true,
            colour: 0xFF_FFFF,
            trans: 0,
            maxlines: 3,
        },
    );
    text.width_mode = 4;
    text.aspect = Some((256, 256));
    text.keymask = 1 << 11;
    text.target = Some(TargetSection {
        param: 1,
        cursor: 2,
        default_cursor: -1,
    });
    text.mouseovercursor = 5;
    text.ops = vec!["A, B".to_string(), "C".to_string()];
    text.int_params = vec![(9, -3)];
    text.str_params = vec![(10, "hi".to_string())];

    let graphic = base_source(
        isource::ComponentType::Graphic,
        ComponentBody::Graphic {
            graphic: 123,
            angle: 512,
            tiling: true,
            alpha: true,
            trans: 64,
            outline: 2,
            shadow: -1,
            vflip: true,
            hflip: false,
            colour: 0,
            clickmask: true,
        },
    );

    let mut origin_model = base_source(
        isource::ComponentType::Model,
        ComponentBody::Model {
            id: 50,
            origin: true,
            extended: false,
            orthog: true,
            nodepth: false,
            ox: 1,
            oy: -2,
            oz: 0,
            ax: 3,
            ay: 4,
            az: 5,
            zoom: 640,
            anim: -1,
            objwidth: Some(100),
            objheight: None,
        },
    );
    origin_model.width_mode = 1;
    origin_model.hide = true;
    origin_model.noclickthrough = true;

    let extended_model = base_source(
        isource::ComponentType::Model,
        ComponentBody::Model {
            id: -1,
            origin: false,
            extended: true,
            orthog: false,
            nodepth: true,
            ox: 0,
            oy: 0,
            oz: -7,
            ax: 0,
            ay: 0,
            az: 0,
            zoom: -300,
            anim: 90_000,
            objwidth: None,
            objheight: Some(200),
        },
    );
    // Height mode nonzero pairs with the object height above (width mode stays 0).
    let mut extended_model = extended_model;
    extended_model.height_mode = 2;

    let mut line = base_source(
        isource::ComponentType::Line,
        ComponentBody::Line {
            width: 2,
            colour: 0x12_3456,
            direction: true,
        },
    );
    line.version = 5;
    line.name = Some("sig".to_string());
    line.opname_nibble = 2;
    line.opname_first = Some((1, 10));
    line.opname_second = Some((2, 20));
    line.transmits.varcstr = Some(vec![-1, 0, 1]);

    vec![
        layer,
        rectangle,
        text,
        graphic,
        origin_model,
        extended_model,
        line,
    ]
}

fn base_source(ctype: isource::ComponentType, body: ComponentBody) -> isource::ComponentSource {
    isource::ComponentSource {
        ctype,
        version: 4,
        name: None,
        clientcode: 0,
        x: 0,
        y: 0,
        width: 10,
        height: 20,
        width_mode: 0,
        height_mode: 0,
        x_mode: 0,
        y_mode: 0,
        aspect: None,
        layer: -1,
        hide: false,
        noclickthrough: false,
        body,
        keymask: 0,
        keybinds: Vec::new(),
        opbase: String::new(),
        ops: Vec::new(),
        opname_nibble: 0,
        opname_first: None,
        opname_second: None,
        pausetext: None,
        dragdeadzone: 0,
        dragdeadtime: 0,
        dragrenderbehaviour: 0,
        targetverb: String::new(),
        target: None,
        mouseovercursor: -1,
        int_params: Vec::new(),
        str_params: Vec::new(),
        hooks: Hooks::default(),
        transmits: Transmits::default(),
    }
}

/// A complete, valid layer text (24 content lines) for mutation testing.
fn minimal_layer() -> String {
    "\
component layer;
version 4;
name none;
clientcode 0;
pos 0 0;
size 10 20;
modes 0 0 0 0;
aspect none;
layer -1;
hide false;
noclickthrough false;
scroll 1 2;
keymask 0;
keybinds [];
opbase \"\";
ops [];
opnames 0;
pause none;
drag 0 0 0;
targetverb \"\";
target none;
mouseover -1;
intparams [];
strparams [];
"
    .to_string()
}

/// Replace 1-based content line `number` with `replacement` (which may span lines).
fn set_line(text: &str, number: usize, replacement: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines[number - 1] = replacement;
    lines.join("\n") + "\n"
}

/// Drop 1-based content line `number`.
fn drop_line(text: &str, number: usize) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.remove(number - 1);
    lines.join("\n") + "\n"
}

/// Append one content line.
fn push_line(text: &str, line: &str) -> String {
    format!("{text}{line}\n")
}
