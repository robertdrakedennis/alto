//! Rust decoders against the original client's decoders, field by field.
//!
//! `tests/fixtures/decode-golden/*.golden` name real cache entries and hold
//! what the original client decoded from them: entry kinds interface
//! component, sprite, script, enum, struct, param, var, varbit, dbtable and
//! dbrow. A block is `@ <kind> <id> <bytes>` followed by the recording.
//!
//! * `<bytes>` is `-` for a real cache entry: its raw bytes are read from
//!   the local revision-910 pack at test time (`cache_bytes`) and never
//!   stored here. A synthetic entry (a script assembled by hand) carries its
//!   bytes as hex.
//! * The recording of a cache entry is one `digest=<FNV-1a 64>` line over the
//!   recorded `key=value` lines joined by `\n`; a synthetic entry keeps its
//!   lines verbatim.
//!
//! The test decodes the same bytes with the Rust codec, renders the same
//! keys and compares (on a mismatch it prints the Rust lines). Unlike the
//! corpus encode->decode round trips, this catches misreads made
//! symmetrically in decoder and encoder (swapped fields, byte order). Tests
//! over cache entries need the pack; the recordings were made once and are
//! not regenerated.
mod common;

use native910::config::{
    ConfigValue, EnumValues, decode_enum, decode_param, decode_struct, decode_var, decode_varbit,
};
use native910::dbtable::{DbCellValue, decode_dbrow, decode_dbtable};
use native910::interface::{ComponentBody, HookArg, decode_component};
use native910::opcode::OpcodeBook;
use native910::script::{Operand, decode_script};
use native910::sprite::{SpriteSheet, decode_sprite};
use native910::vars::VarScope;
use std::collections::BTreeMap;

const FIXTURE: &str = "decode-golden";

/// One golden block: kind, id label, raw bytes (empty until located for a
/// cache entry), recorded lines (or a single `digest=` line for a cache entry).
struct Block {
    kind: String,
    id: String,
    from_pack: bool,
    bytes: Vec<u8>,
    recorded: Vec<String>,
}

/// FNV-1a 64 of the recorded lines joined by `\n`, as 16 hex digits.
fn lines_digest(lines: &[String]) -> String {
    format!("digest={}", common::line_digest(&lines.join("\n")))
}

/// Opened cache archives by file name.
#[derive(Default)]
struct Packs(std::collections::HashMap<&'static str, native910::pack::PackArchive>);

impl Packs {
    /// The raw bytes of the cache entry a block names.
    fn cache_bytes(&mut self, kind: &str, id: &str) -> Vec<u8> {
        let num = |text: &str| -> u32 { text.parse().unwrap() };
        let pair = |id: &str| -> (u32, u32) {
            let (a, b) = id.split_once(':').unwrap();
            (num(a), num(b))
        };
        let (archive, group, file) = match kind {
            "interface" => {
                let (g, f) = pair(id);
                ("client.interfaces.js5", g, f)
            }
            "sprite" => ("client.sprites.js5", num(id), 0),
            "script" => ("client.scripts.js5", num(id), 0),
            "enum" => {
                let (g, f) = pair(id);
                ("client.enum.config.js5", g, f)
            }
            "struct" => {
                let (g, f) = pair(id);
                ("client.struct.config.js5", g, f)
            }
            "param" => ("client.config.js5", 11, num(id)),
            "var" => {
                let (d, f) = pair(id);
                let scope = VarScope::from_id(d as u8).unwrap();
                (
                    "client.config.js5",
                    native910::config::var_group_id(scope),
                    f,
                )
            }
            "varbit" => ("client.config.js5", 69, num(id)),
            "dbtable" => ("client.config.js5", 40, num(id)),
            "dbrow" => ("client.config.js5", 41, num(id)),
            other => panic!("kind {other}"),
        };
        let packed = self.0.entry(archive).or_insert_with(|| {
            native910::pack::PackArchive::open(&common::require_pack_file(archive)).unwrap()
        });
        packed
            .group_files(group)
            .unwrap()
            .unwrap_or_else(|| panic!("{archive}: no group {group} for {kind} {id}"))
            .remove(&file)
            .unwrap_or_else(|| panic!("{archive}: no file {file} in group {group} for {kind} {id}"))
    }
}

fn blocks(file: &str) -> Vec<Block> {
    let text = std::fs::read_to_string(common::fixture(FIXTURE).join(file)).unwrap();
    let mut out: Vec<Block> = Vec::new();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("@ ") {
            let mut parts = header.splitn(3, ' ');
            let (kind, id, bytes) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
            out.push(Block {
                kind: kind.into(),
                id: id.into(),
                from_pack: bytes == "-",
                bytes: if bytes == "-" {
                    Vec::new()
                } else {
                    common::unhex(bytes)
                },
                recorded: Vec::new(),
            });
        } else if !line.starts_with('#') && !line.is_empty() {
            out.last_mut()
                .expect("line before header")
                .recorded
                .push(line.into());
        }
    }
    out
}

/// String rendering of the recording harness (`s` + UTF-16 units).
fn js(value: &str) -> String {
    common::utf16_objects(std::iter::once(Some(value)))
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

fn opt_js(value: Option<&str>) -> String {
    value.map_or_else(|| "null".into(), js)
}

fn ints(values: &[i32]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn hexs(bytes: &[u8]) -> String {
    format!("x{}", common::hex(bytes))
}

/// Line collector matching the harness `kv`/`raw` output.
#[derive(Default)]
struct Lines(Vec<String>);

impl Lines {
    fn raw(&mut self, key: impl std::fmt::Display, value: impl std::fmt::Display) {
        self.0.push(format!("{key}={value}"));
    }
    fn int(&mut self, key: impl std::fmt::Display, value: impl Into<i64>) {
        self.raw(key, format!("i{}", value.into()));
    }
    fn bool(&mut self, key: impl std::fmt::Display, value: bool) {
        self.raw(key, value);
    }
}

fn compare(file: &str, kinds: &[&str], render: impl Fn(&Block) -> Result<Vec<String>, String>) {
    let blocks = blocks(file);
    let mut packs = Packs::default();
    let mut checked = BTreeMap::<String, usize>::new();
    let mut failures = Vec::new();
    for mut block in blocks
        .into_iter()
        .filter(|b| kinds.contains(&b.kind.as_str()))
    {
        *checked.entry(block.kind.clone()).or_default() += 1;
        if block.from_pack {
            block.bytes = packs.cache_bytes(&block.kind, &block.id);
        }
        match render(&block) {
            Err(error) => failures.push(format!(
                "{} {}: Rust decode failed: {error}",
                block.kind, block.id
            )),
            Ok(rust) => {
                if block.from_pack {
                    if block.recorded != [lines_digest(&rust)] {
                        failures.push(format!(
                            "{} {}: differs from the recording (recorded {:?}, Rust {})\n  rust lines: {rust:?}",
                            block.kind,
                            block.id,
                            block.recorded,
                            lines_digest(&rust)
                        ));
                    }
                } else if rust != block.recorded {
                    let at = rust
                        .iter()
                        .zip(&block.recorded)
                        .position(|(r, j)| r != j)
                        .unwrap_or_else(|| rust.len().min(block.recorded.len()));
                    failures.push(format!(
                        "{} {}: first difference at line {at}\n  rust: {:?}\n  recorded: {:?}",
                        block.kind,
                        block.id,
                        rust.get(at),
                        block.recorded.get(at)
                    ));
                }
            }
        }
    }
    for kind in kinds {
        assert!(
            checked.get(*kind).copied().unwrap_or(0) >= 3,
            "{file}: too few {kind} blocks: {checked:?}"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Renderers: the Rust model expressed in the recording's field vocabulary.

fn render_component(block: &Block) -> Result<Vec<String>, String> {
    let (iface, _child) = block.id.split_once(':').unwrap();
    let iface: i32 = iface.parse().unwrap();
    let c = decode_component(&block.bytes, iface << 16).map_err(|e| e.to_string())?;
    let mut l = Lines::default();
    l.int("type", c.type_id);
    l.raw("name", opt_js(c.name.as_deref()));
    l.int("clientcode", c.clientcode);
    l.int("xpos", c.x);
    l.int("ypos", c.y);
    l.int("wsize", c.width);
    l.int("hsize", c.height);
    l.int("widthSizeMode", c.width_mode);
    l.int("heightSizeMode", c.height_mode);
    l.int("xmode", c.x_mode);
    l.int("ymode", c.y_mode);
    let (aw, ah) = c
        .aspect
        .map_or((1, 1), |(w, h)| (i64::from(w), i64::from(h)));
    l.int("aspectwidth", aw);
    l.int("aspectheight", ah);
    l.int("layer", c.layer);
    l.bool("hide", c.hide);
    l.bool("noclickthrough", c.noclickthrough);
    match &c.body {
        ComponentBody::Layer {
            scroll_width,
            scroll_height,
        } => {
            l.int("scrollwidth", *scroll_width);
            l.int("scrollheight", *scroll_height);
        }
        ComponentBody::Rectangle {
            colour,
            fill,
            trans,
        } => {
            l.int("colour", *colour);
            l.bool("fill", *fill);
            l.int("trans", *trans);
        }
        ComponentBody::Text {
            font,
            mono,
            text,
            line_height,
            halign,
            valign,
            shadow,
            colour,
            trans,
            maxlines,
        } => {
            l.int("textfont", *font);
            l.bool("fontmono", *mono);
            l.raw("text", js(text));
            l.int("textLineHeight", *line_height);
            l.int("textHAlign", *halign);
            l.int("textVAlign", *valign);
            l.bool("textshadow", *shadow);
            l.int("colour", *colour);
            l.int("trans", *trans);
            l.int("maxlines", *maxlines);
        }
        ComponentBody::Graphic {
            graphic,
            angle,
            tiling,
            alpha,
            trans,
            outline,
            shadow,
            vflip,
            hflip,
            colour,
            clickmask,
        } => {
            l.int("graphic", *graphic);
            l.int("angle2d", *angle);
            l.bool("tiling", *tiling);
            l.bool("alpha", *alpha);
            l.int("trans", *trans);
            l.int("outline", *outline);
            l.int("graphicshadow", *shadow);
            l.bool("vflip", *vflip);
            l.bool("hflip", *hflip);
            l.int("colour", *colour);
            l.bool("clickmask", *clickmask);
        }
        ComponentBody::Model {
            id,
            origin,
            extended,
            orthog,
            nodepth,
            ox,
            oy,
            oz,
            ax,
            ay,
            az,
            zoom,
            anim,
            objwidth,
            objheight,
        } => {
            // Field defaults when neither transform block is carried:
            // origins/angles 0, `modelzoom = 100`.
            let carried = *origin || *extended;
            l.int("model", *id);
            l.bool("useExtendedModelTransform", *extended);
            l.bool("modelorthog", *orthog);
            l.bool("disableDepthTest", *nodepth);
            l.int("modelorigin_x", if carried { *ox } else { 0 });
            l.int("modelorigin_y", if carried { *oy } else { 0 });
            l.int("modelorigin_z", if *extended && !*origin { *oz } else { 0 });
            l.int("modelangle_x", if carried { *ax } else { 0 });
            l.int("modelangle_y", if carried { *ay } else { 0 });
            l.int("modelangle_z", if carried { *az } else { 0 });
            l.int("modelzoom", if carried { *zoom } else { 100 });
            l.int("modelanim", *anim);
            l.int("modelobjwidth", objwidth.unwrap_or(0));
            l.int("modelobjheight", objheight.unwrap_or(0));
        }
        ComponentBody::Line {
            width,
            colour,
            direction,
        } => {
            l.int("linewid", *width);
            l.int("colour", *colour);
            l.bool("linedirection", *direction);
        }
    }
    // Keybinds: slot = (head >> 4) - 1; delay = 12 low bits of head:lo
    // (4095 = -1); the last entry for a slot wins (Component.decode).
    let mut keys = BTreeMap::new();
    for key in &c.keybinds {
        let slot = i32::from(key.head >> 4) - 1;
        let delay = ((i32::from(key.head) << 8) | i32::from(key.lo)) & 0xFFF;
        let delay = if delay == 4095 { -1 } else { delay };
        keys.insert(slot, format!("{delay},{},{}", key.key, key.mods));
    }
    for (slot, text) in keys {
        l.raw(format!("opkey.{slot}"), text);
    }
    l.raw("opbase", js(&c.opbase));
    if c.ops.is_empty() {
        l.raw("op", "null");
    } else {
        let ops: Vec<String> = c.ops.iter().map(|op| js(op)).collect();
        l.raw("op", format!("[{}]", ops.join(",")));
    }
    match c.opname_first {
        None => l.raw("opname", "null"),
        Some((index, cursor)) => {
            let mut names = vec![-1; usize::from(index) + 1];
            names[usize::from(index)] = i32::from(cursor);
            if let Some((index, cursor)) = c.opname_second {
                names[usize::from(index)] = i32::from(cursor);
            }
            l.raw("opname", ints(&names));
        }
    }
    l.raw("pausetext", opt_js(c.pausetext.as_deref()));
    l.int("dragdeadzone", c.dragdeadzone);
    l.int("dragdeadtime", c.dragdeadtime);
    l.int("dragrenderbehaviour", c.dragrenderbehaviour);
    l.raw("targetverb", js(&c.targetverb));
    l.int("targetCursor", c.target.map_or(-1, |t| t.cursor));
    l.int(
        "targetDefaultCursor",
        c.target.map_or(-1, |t| t.default_cursor),
    );
    l.int("mouseovercursor", c.mouseovercursor);
    l.int("mask", c.keymask);
    l.int("maskparam", c.target.map_or(-1, |t| t.param));
    let mut params = BTreeMap::new();
    for (key, value) in &c.int_params {
        params.insert(*key, format!("i{value}"));
    }
    for (key, value) in &c.str_params {
        params.insert(*key, js(value));
    }
    for (key, value) in params {
        l.raw(format!("param.{key}"), value);
    }
    let h = &c.hooks;
    for (name, hook) in [
        ("onload", &h.onload),
        ("onmouseover", &h.onmouseover),
        ("onmouseleave", &h.onmouseleave),
        ("ontargetleave", &h.ontargetleave),
        ("ontargetenter", &h.ontargetenter),
        ("onvartransmit", &h.onvartransmit),
        ("oninvtransmit", &h.oninvtransmit),
        ("onstattransmit", &h.onstattransmit),
        ("ontimer", &h.ontimer),
        ("onop", &h.onop),
        ("onopt", &h.onopt),
        ("onmouserepeat", &h.onmouserepeat),
        ("onclick", &h.onclick),
        ("onclickrepeat", &h.onclickrepeat),
        ("onrelease", &h.onrelease),
        ("onhold", &h.onhold),
        ("ondrag", &h.ondrag),
        ("ondragcomplete", &h.ondragcomplete),
        ("onscrollwheel", &h.onscrollwheel),
        ("onvarctransmit", &h.onvarctransmit),
        ("onvarcstrtransmit", &h.onvarcstrtransmit),
    ] {
        match hook {
            None => l.raw(name, "null"),
            Some(args) => {
                let args: Vec<String> = args
                    .iter()
                    .map(|arg| match arg {
                        HookArg::Int(v) => format!("i{v}"),
                        HookArg::Str(s) => js(s),
                    })
                    .collect();
                l.raw(name, format!("[{}]", args.join(",")));
            }
        }
    }
    let t = &c.transmits;
    for (name, list) in [
        ("onvartransmitlist", &t.var),
        ("oninvtransmitlist", &t.inv),
        ("onstattransmitlist", &t.stat),
        ("onvarctransmitlist", &t.varc),
        ("onvarcstrtransmitlist", &t.varcstr),
    ] {
        l.raw(
            name,
            list.as_ref().map_or_else(|| "null".into(), |v| ints(v)),
        );
    }
    Ok(l.0)
}

fn render_sprite(block: &Block) -> Result<Vec<String>, String> {
    let sheet = decode_sprite(&block.bytes).map_err(|e| e.to_string())?;
    let mut l = Lines::default();
    l.int("count", sheet.sprite_count() as i64);
    match &sheet {
        SpriteSheet::Paletted(sheet) => {
            // Palette index 0 stays 0, a stored 0 elsewhere
            // becomes 1; an alpha plane of only 0xFF bytes is dropped.
            let palette: Vec<i32> = sheet
                .palette
                .iter()
                .enumerate()
                .map(|(i, v)| if i > 0 && *v == 0 { 1 } else { *v as i32 })
                .collect();
            for (i, s) in sheet.sprites.iter().enumerate() {
                let p = format!("sprite.{i}.");
                l.bool(format!("{p}paletted"), true);
                l.int(format!("{p}width"), s.width);
                l.int(format!("{p}height"), s.height);
                l.raw(
                    format!("{p}padding"),
                    format!(
                        "{},{},{},{}",
                        s.padding_left,
                        s.padding_top,
                        s.padding_right(sheet.canvas_width),
                        s.padding_bottom(sheet.canvas_height)
                    ),
                );
                l.raw(format!("{p}palette"), ints(&palette));
                l.raw(format!("{p}colour"), hexs(&s.colour));
                let alpha = s
                    .alpha
                    .as_ref()
                    .filter(|alpha| alpha.iter().any(|a| *a != 0xFF));
                l.raw(
                    format!("{p}alpha"),
                    alpha.map_or_else(|| "null".into(), |a| hexs(a)),
                );
            }
        }
        SpriteSheet::Full(sheet) => {
            for (i, s) in sheet.sprites.iter().enumerate() {
                let p = format!("sprite.{i}.");
                l.bool(format!("{p}paletted"), false);
                l.int(format!("{p}width"), sheet.width);
                l.int(format!("{p}height"), sheet.height);
                // ARGB with opaque magenta keyed to 0, then the alpha plane.
                let argb: Vec<i32> = s
                    .rgb
                    .chunks(3)
                    .enumerate()
                    .map(|(n, rgb)| {
                        let mut px = (i32::from(rgb[0]) << 16)
                            | (i32::from(rgb[1]) << 8)
                            | i32::from(rgb[2])
                            | (0xFF << 24);
                        if px == -65281 {
                            px = 0;
                        }
                        if let Some(alpha) = &s.alpha {
                            px = (px & 0xFF_FFFF) | (i32::from(alpha[n]) << 24);
                        }
                        px
                    })
                    .collect();
                l.raw(format!("{p}argb"), ints(&argb));
            }
        }
    }
    Ok(l.0)
}

fn render_script(block: &Block, book: &OpcodeBook) -> Result<Vec<String>, String> {
    let s = decode_script(&block.bytes, book).map_err(|e| e.to_string())?;
    let mut l = Lines::default();
    l.raw("name", opt_js(s.name.as_deref()));
    l.raw(
        "locals",
        format!("{},{},{}", s.locals.int, s.locals.obj, s.locals.long),
    );
    l.raw(
        "args",
        format!("{},{},{}", s.args.int, s.args.obj, s.args.long),
    );
    let opcode = |name: &str| book.opcode_for(name).unwrap();
    for (i, ins) in s.code.iter().enumerate() {
        let rel = |target: i32| target - i as i32 - 1;
        let text = match (&*ins.command, &ins.operand) {
            // The original decoder rewrites typed constants at decode.
            ("push_constant_string", Operand::Int(v)) => {
                format!("{} int {v}", opcode("push_constant_int"))
            }
            ("push_constant_string", Operand::Long(v)) => {
                format!("{} long {v}", opcode("push_long_constant"))
            }
            (_, Operand::Str(v)) => format!("{} str {}", opcode(&ins.command), js(v)),
            (_, Operand::Switch(cases)) => {
                let cases: BTreeMap<i32, i32> =
                    cases.iter().map(|c| (c.value, rel(c.target))).collect();
                let cases: Vec<String> = cases.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("{} switch {{{}}}", opcode(&ins.command), cases.join(", "))
            }
            (_, Operand::VarRef(v)) => format!(
                "{} var {}:{} {}",
                opcode(&ins.command),
                u8::from(v.domain),
                v.id,
                i32::from(v.transmog)
            ),
            (_, Operand::VarBitRef(v)) => format!(
                "{} varbit {} {}",
                opcode(&ins.command),
                v.id,
                i32::from(v.transmog)
            ),
            (_, operand) => {
                let value = match operand {
                    Operand::Branch(target) => rel(*target),
                    Operand::Int(v)
                    | Operand::Local(v)
                    | Operand::Script(v)
                    | Operand::Array(v)
                    | Operand::Count(v) => *v,
                    Operand::Byte(v) => i32::from(*v),
                    Operand::Long(_) => return Err(format!("long operand on {}", ins.command)),
                    _ => unreachable!(),
                };
                format!("{} int {value}", opcode(&ins.command))
            }
        };
        l.raw(format!("ins.{i}"), text);
    }
    Ok(l.0)
}

fn config_value(value: &ConfigValue) -> String {
    match value {
        ConfigValue::Int(v) => format!("i{v}"),
        ConfigValue::Str(s) => js(s),
    }
}

/// Db tuples as decoded (one `Vec` per tuple).
type Tuples = Vec<Vec<DbCellValue>>;

fn cell(value: &DbCellValue) -> String {
    match value {
        DbCellValue::Int(v) => format!("i{v}"),
        DbCellValue::Long(v) => format!("l{v}"),
        DbCellValue::Str(s) => js(s),
    }
}

fn render_config(block: &Block) -> Result<Vec<String>, String> {
    let bytes = &block.bytes;
    let e = |e: native910::error::NativeError| e.to_string();
    let mut l = Lines::default();
    let opt = |v: Option<u16>| v.map_or_else(|| "null".to_string(), |v| v.to_string());
    match block.kind.as_str() {
        "enum" => {
            let t = decode_enum(bytes).map_err(e)?;
            if t.input_legacy.is_some() || t.output_legacy.is_some() {
                return Err("legacy char types are not modelled".into());
            }
            l.raw("inputtype", opt(t.input_type));
            l.raw("outputtype", opt(t.output_type));
            l.raw(
                "defaultString",
                js(t.default_string.as_deref().unwrap_or("null")),
            );
            l.int("defaultInt", t.default_int.unwrap_or(0));
            match &t.values {
                None => l.int("valuesCount", 0),
                Some(EnumValues::SparseString(rows) | EnumValues::SparseInt(rows)) => {
                    l.int("valuesCount", rows.len() as i64);
                    let sorted: BTreeMap<i32, String> = rows
                        .iter()
                        .map(|r| (r.key, config_value(&r.value)))
                        .collect();
                    for (key, value) in sorted {
                        l.raw(format!("value.{key}"), value);
                    }
                }
                Some(
                    EnumValues::DenseString { capacity, slots }
                    | EnumValues::DenseInt { capacity, slots },
                ) => {
                    l.int("valuesCount", slots.len() as i64);
                    let mut dense = vec!["null".to_string(); usize::from(*capacity)];
                    for slot in slots {
                        dense[usize::from(slot.index)] = config_value(&slot.value);
                    }
                    l.raw("dense", format!("[{}]", dense.join(",")));
                }
            }
        }
        "struct" => {
            let t = decode_struct(bytes).map_err(e)?;
            let sorted: BTreeMap<u32, String> = t
                .params
                .iter()
                .map(|r| (r.param, config_value(&r.value)))
                .collect();
            for (key, value) in sorted {
                l.raw(format!("param.{key}"), value);
            }
        }
        "param" => {
            let t = decode_param(bytes).map_err(e)?;
            if t.kind_legacy.is_some() {
                return Err("legacy char types are not modelled".into());
            }
            l.raw("type", opt(t.kind));
            l.int("defaultint", t.default_int.unwrap_or(0));
            l.raw("defaultstr", opt_js(t.default_string.as_deref()));
            l.bool("autodisable", t.autodisable);
        }
        "var" => {
            let (domain, _) = block.id.split_once(':').unwrap();
            let domain = VarScope::from_id(domain.parse().unwrap()).unwrap();
            let t = decode_var(bytes, domain).map_err(e)?;
            l.raw("dataType", opt(t.data_type.map(u16::from)));
            // The lifetime defaults to TEMPORARY (serial 0).
            l.int("lifeTime", t.lifetime.unwrap_or(0));
            l.bool("legacyDefaultValue", t.legacy_default_value);
            if domain == VarScope::Player {
                l.int("clientCode", t.client_code.unwrap_or(0));
            }
        }
        "varbit" => {
            let t = decode_varbit(bytes).map_err(e)?;
            l.raw(
                "domain",
                t.base
                    .map_or_else(|| "null".into(), |b| format!("i{}", b.domain)),
            );
            l.int("baseVarId", t.base.map_or(-1, |b| b.var));
            l.int("startBit", t.bits.map_or(0, |b| b.start));
            l.int("endBit", t.bits.map_or(0, |b| b.end));
        }
        "dbtable" => {
            let t = decode_dbtable(bytes).map_err(e)?;
            let columns: BTreeMap<u8, (&Vec<u16>, Option<&Tuples>)> = t
                .columns
                .iter()
                .map(|c| (c.column, (&c.types, c.defaults.as_ref())))
                .collect();
            db_lines(
                &mut l,
                t.column_count,
                columns
                    .into_iter()
                    .map(|(k, (types, rows))| (k, types, rows)),
            );
        }
        "dbrow" => {
            let t = decode_dbrow(bytes).map_err(e)?;
            l.int("tableId", t.table.map_or(0, i64::from));
            let columns: BTreeMap<u8, (&Vec<u16>, &Vec<Vec<DbCellValue>>)> = t
                .columns
                .iter()
                .map(|c| (c.column, (&c.types, &c.rows)))
                .collect();
            db_lines(
                &mut l,
                t.column_count,
                columns
                    .into_iter()
                    .map(|(k, (types, rows))| (k, types, Some(rows))),
            );
        }
        other => panic!("kind {other}"),
    }
    Ok(l.0)
}

/// dbtable/dbrow arrays: one entry per present column, values flattened
/// tuple by tuple.
fn db_lines<'a>(
    l: &mut Lines,
    count: Option<u8>,
    columns: impl Iterator<Item = (u8, &'a Vec<u16>, Option<&'a Vec<Vec<DbCellValue>>>)>,
) {
    let Some(count) = count else {
        l.raw("columns", "null");
        return;
    };
    l.int("columns", count);
    for (column, types, rows) in columns {
        let types: Vec<i32> = types.iter().map(|t| i32::from(*t)).collect();
        l.raw(format!("column.{column}.types"), ints(&types));
        let values = rows.map_or_else(
            || "null".to_string(),
            |rows| {
                format!(
                    "[{}]",
                    rows.iter()
                        .flatten()
                        .map(cell)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            },
        );
        l.raw(format!("column.{column}.values"), values);
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn interfaces_match_recorded_decode() {
    compare("interfaces.golden", &["interface"], render_component);
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn sprites_match_recorded_decode() {
    compare("sprites.golden", &["sprite"], render_sprite);
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn scripts_match_recorded_decode() {
    let book = OpcodeBook::embedded().unwrap();
    compare("scripts.golden", &["script"], |b| render_script(b, &book));
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn configs_match_recorded_decode() {
    compare(
        "configs.golden",
        &[
            "enum", "struct", "param", "var", "varbit", "dbtable", "dbrow",
        ],
        render_config,
    );
}

/// Golden label of [`raw_long_constant_script`].
const RAW_LONG_CONSTANT_ID: i32 = 900_761;

/// A script assembled BY HAND (not by the codec under test) around raw
/// opcode 761 (`push_long_constant`). The original decoder gives it no special
/// case: like every large-operand command it carries a 4-byte int, not 8.
fn raw_long_constant_script() -> Vec<u8> {
    let mut bytes = vec![0x00]; // fastgstr: null name
    bytes.extend([0x02, 0xF9, 0x01, 0x02, 0x03, 0x04]); // 761, g4s 0x01020304
    bytes.extend([0x02, 0xF9, 0xFF, 0xFF, 0xFF, 0xFE]); // 761, g4s -2
    bytes.extend([0x05, 0x45, 0x00]); // return (1349), g1 0
    bytes.extend([0x00, 0x00, 0x00, 0x03]); // g4s instruction count
    bytes.extend([0x00; 12]); // 6 x g2 local/arg counts
    bytes.push(0x00); // g1 switch table count
    bytes.extend([0x00, 0x01]); // g2 trailer length (the switch count byte)
    bytes
}

#[test]
fn raw_push_long_constant_operand_is_four_bytes() {
    let book = OpcodeBook::embedded().unwrap();
    let bytes = raw_long_constant_script();
    let script = decode_script(&bytes, &book).unwrap();
    let ops: Vec<(&str, &Operand)> = script
        .code
        .iter()
        .map(|ins| (ins.command.as_str(), &ins.operand))
        .collect();
    assert_eq!(
        ops,
        [
            ("push_long_constant", &Operand::Int(0x0102_0304)),
            ("push_long_constant", &Operand::Int(-2)),
            ("return", &Operand::Byte(0)),
        ]
    );
    // Encode writes the same 4-byte operand back: byte-identical.
    assert_eq!(
        native910::script::encode_script(&script, &book).unwrap(),
        bytes
    );
    // The original decoder's reading of these bytes is the committed golden block.
    let script = blocks("scripts.golden")
        .into_iter()
        .find(|b| b.id == RAW_LONG_CONSTANT_ID.to_string())
        .expect("raw 761 block missing from scripts.golden");
    assert_eq!(script.bytes, bytes);
    assert!(
        script
            .recorded
            .contains(&"ins.0=761 int 16909060".to_string()),
        "{:?}",
        script.recorded
    );
}
