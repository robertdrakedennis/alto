//! Editable source text for 910 interface components: the human-scale form between text and bytes.
//!
//! [`ComponentSource`] mirrors [`InterfaceComponent`](crate::interface::InterfaceComponent)
//! field-for-field with four human-scale substitutions (everything else keeps its wire
//! width, so range errors stay loud at parse and lower time):
//!
//! * the component type is a word (`layer`, `rectangle`, `text`, `graphic`, `model`,
//!   `line`) instead of a `type_id` byte;
//! * colours render as `0x`-hex ints (two's-complement for the rare negative);
//! * hook programs render their head (callee) id as `~name` when the passed
//!   [`SymbolRegistry`](crate::symbols::SymbolRegistry) resolves it, else numeric —
//!   the same rule [`format_source`](crate::source::format_source) uses for gosubs;
//! * strings are double-quoted with the exact `\\ \" \n \r \t` escaping discipline
//!   from [`source`](crate::source).
//!
//! Deliberate non-substitutions, documented so nobody re-litigates them:
//!
//! * Size/position modes stay explicit ints: the 910 decoder carries raw signed bytes
//!   with no client-side vocabulary, so there is no codebase mapping to borrow and
//!   inventing words would fake meaning.
//! * Sub-model types are reused verbatim (`ComponentBody`, `Hooks`, `Transmits`,
//!   `Keybind`, `TargetSection`, `HookArg`): the human scaling lives at the text layer
//!   (words, hex, `~names`), so parallel model types would only add drift surface.
//! * Filenames carry identity: there is no `[com<file>]` header. The formatter emits a
//!   fixed `// interface component` marker line (the model never sees group/file — the
//!   layer is resolved); dump workflows may prepend their own
//!   `// interface <group> component <file>` line, which parses identically because
//!   comments are insignificant.
//!
//! Canonical text (what [`format_component`] emits; the parser accepts the same lines in
//! any order once the `component` header has fixed the type, with free blank lines and
//! `//` comments). A layer component:
//!
//! ```text
//! // interface component
//! component layer;
//! version 4;
//! name none;
//! clientcode 0;
//! pos -1 2;
//! size 100 200;
//! modes 0 0 0 0;
//! aspect none;
//! layer -1;
//! hide true;
//! noclickthrough false;
//! scroll 300 400;
//! keymask 0;
//! keybinds [];
//! opbase "";
//! ops ["Take"];
//! opnames 1;
//! opname1 0 7;
//! pause none;
//! drag 0 0 0;
//! targetverb "";
//! target none;
//! mouseover -1;
//! intparams [(9, -3)];
//! strparams [(10, "hi")];
//! hook onload none;
//! hook onmouseover none;
//! hook onmouseleave none;
//! hook ontargetleave none;
//! hook ontargetenter none;
//! hook onvartransmit none;
//! hook oninvtransmit none;
//! hook onstattransmit none;
//! hook ontimer none;
//! hook onop none;
//! hook onopt none;
//! hook onmouserepeat none;
//! hook onclick [5690, "x"];
//! hook onclickrepeat none;
//! hook onrelease none;
//! hook onhold none;
//! hook ondrag none;
//! hook ondragcomplete none;
//! hook onscrollwheel none;
//! hook onvarctransmit none;
//! hook onvarcstrtransmit none;
//! transmit var [1752];
//! transmit inv none;
//! transmit stat none;
//! transmit varc none;
//! transmit varcstr none;
//! ```
//!
//! Per-type body blocks (emitted between `noclickthrough` and `keymask`):
//!
//! ```text
//! // rectangle
//! colour 0xFF0000;
//! fill true;
//! trans 128;
//! // text
//! font 1;
//! mono false;
//! text "Hello";
//! lineheight 12;
//! halign 0;
//! valign 0;
//! shadow false;
//! colour 0xFFFFFF;
//! trans 0;
//! maxlines 0;
//! // graphic
//! graphic 123;
//! angle 0;
//! tiling false;
//! alpha false;
//! trans 0;
//! outline 0;
//! shadow 0x0;
//! vflip false;
//! hflip false;
//! colour 0xFFFFFF;
//! clickmask false;
//! // model
//! model 100;
//! origin false;
//! extended false;
//! orthog false;
//! nodepth false;
//! ox 0;
//! oy 0;
//! oz 0;
//! ax 0;
//! ay 0;
//! az 0;
//! zoom 64;
//! anim -1;
//! objwidth none;
//! objheight none;
//! // line
//! linewidth 2;
//! colour 0xFF0000;
//! direction false;
//! ```
//!
//! Rules the parser enforces (every failure names its 1-based line):
//!
//! * The first statement must be the `component <word>;` header; everything after is
//!   order-free. Body keys are parsed type-aware (`shadow` is a bool for text, a hex
//!   colour for graphic), and a body key from another type is a loud error.
//! * Every statement ends with `;`; trailing text after it is an error. `//` comments
//!   run to end of line but are data inside string literals; blank lines are
//!   insignificant. Strings, lists, and tuples are parsed quote- and depth-aware, so
//!   game text containing `;`, `,`, `[`, or `//` survives.
//! * Duplicate fields are errors. Every scalar and every body field is required; absent
//!   `hook`/`transmit` lines default to `none` (the overwhelmingly common case —
//!   hand authors should not need 27 `none` lines to say nothing).
//! * Booleans are exactly `true`/`false`. Ints are decimal; colours accept `0x`-hex or
//!   decimal on input and always format hex. Hook elements are ints, `~name` calls
//!   (resolved via the registry — unknown names fail), or quoted strings; `~name` may
//!   appear in any element position but the formatter only folds the head (callee)
//!   slot, mirroring client execution convention.
//! * All counts are re-derived where the binary derives them (op counts, keybind
//!   termination, hook/transmit/param lengths), so the text carries none. The wire
//!   carries three non-derived scalars that stay explicit: `opnames` nibble plus its
//!   `opname1`/`opname2` blocks, validated for agreement like the encoder.
//! * Normalizations the wire forces (also applied by the parser so parsed models always
//!   lower cleanly): empty `pause ""` means absent (the wire cannot distinguish it
//!   from missing), and empty `hook ... []` / `transmit ... []` lists mean `none`
//!   (both encode to a zero count, which re-decodes as absent).
//!
//! [`lift_component`] is total over decoded models: decode output only ever carries the
//! six primitive types with matching bodies, so nothing decoded can fail to lift (an
//! unknown `type_id` on a hand-built model falls back to the body discriminant rather
//! than erroring). [`lower_component`] validates loudly instead: unknown versions,
//! out-of-range numbers, bad enums, smart-range violations, and every cross-field
//! agreement the encoder assumes (aspect/modes, object dims/modes, transform blocks,
//! op-name blocks, target/keymask, version gates) are all errors. [`assemble_component`]
//! parses, lowers, encodes, and verifies: the output must re-decode to the lowered
//! model exactly, the same self-verify discipline as `assemble_source`.

use crate::error::{NativeError, Result};
use crate::interface::{
    ComponentBody, Hook, HookArg, Hooks, InterfaceComponent, Transmits, decode_component,
    encode_component,
};
use crate::symbols::SymbolRegistry;
use std::fmt::Write as _;

/// A component type word: the human-scale name for a wire `type_id`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentType {
    /// Type 0: container.
    Layer,
    /// Type 3: filled rectangle.
    Rectangle,
    /// Type 4: static text.
    Text,
    /// Type 5: sprite graphic.
    Graphic,
    /// Type 6: 3D model.
    Model,
    /// Type 9: line.
    Line,
}

impl ComponentType {
    /// Canonical source word for the type.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Layer => "layer",
            Self::Rectangle => "rectangle",
            Self::Text => "text",
            Self::Graphic => "graphic",
            Self::Model => "model",
            Self::Line => "line",
        }
    }

    /// Wire type id for the type.
    #[must_use]
    pub fn type_id(self) -> u8 {
        match self {
            Self::Layer => 0,
            Self::Rectangle => 3,
            Self::Text => 4,
            Self::Graphic => 5,
            Self::Model => 6,
            Self::Line => 9,
        }
    }

    /// Parse a source word; anything else (including composite widget names, which 910
    /// has no binary shape for) is a loud error.
    pub fn parse_word(word: &str) -> Result<Self> {
        match word {
            "layer" => Ok(Self::Layer),
            "rectangle" => Ok(Self::Rectangle),
            "text" => Ok(Self::Text),
            "graphic" => Ok(Self::Graphic),
            "model" => Ok(Self::Model),
            "line" => Ok(Self::Line),
            _ => Err(invalid(&format!(
                "unknown component type '{word}' (expected layer, rectangle, text, graphic, model, or line)"
            ))),
        }
    }

    /// Type word for a wire id; `None` outside {0, 3, 4, 5, 6, 9}.
    #[must_use]
    pub fn from_type_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Layer),
            3 => Some(Self::Rectangle),
            4 => Some(Self::Text),
            5 => Some(Self::Graphic),
            6 => Some(Self::Model),
            9 => Some(Self::Line),
            _ => None,
        }
    }

    /// Type word from a body discriminant. Only used as the fallback for models no
    /// decoder could produce (unknown `type_id`); decoded models always agree, so the
    /// fallback never fires on them.
    fn for_body(body: &ComponentBody) -> Self {
        match body {
            ComponentBody::Layer { .. } => Self::Layer,
            ComponentBody::Rectangle { .. } => Self::Rectangle,
            ComponentBody::Text { .. } => Self::Text,
            ComponentBody::Graphic { .. } => Self::Graphic,
            ComponentBody::Model { .. } => Self::Model,
            ComponentBody::Line { .. } => Self::Line,
        }
    }
}

/// A 910 interface component in editable form: [`InterfaceComponent`](crate::interface::InterfaceComponent)
/// field-for-field, with the type as a [`ComponentType`] word. Sub-model types are
/// reused verbatim (see the module docs for why); only the text layer scales them to
/// words, hex, `~names`, and quoted strings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentSource {
    /// Component type word.
    pub ctype: ComponentType,
    /// Wire version (`-1`, 3, 4, or 5).
    pub version: i32,
    /// Author name (absent in 910; carried for codec completeness).
    pub name: Option<String>,
    /// Client code.
    pub clientcode: u16,
    /// Position and size.
    pub x: i16,
    /// Position and size.
    pub y: i16,
    /// Position and size.
    pub width: u16,
    /// Position and size.
    pub height: u16,
    /// Size/position modes (raw signed bytes — no client vocabulary exists, so ints).
    pub width_mode: i8,
    /// Size/position modes (raw signed bytes — no client vocabulary exists, so ints).
    pub height_mode: i8,
    /// Size/position modes (raw signed bytes — no client vocabulary exists, so ints).
    pub x_mode: i8,
    /// Size/position modes (raw signed bytes — no client vocabulary exists, so ints).
    pub y_mode: i8,
    /// Aspect size, present iff either size mode is 4.
    pub aspect: Option<(u16, u16)>,
    /// Resolved layer (`-1` = none).
    pub layer: i32,
    /// Hidden flag.
    pub hide: bool,
    /// No-click-through flag.
    pub noclickthrough: bool,
    /// Per-type body.
    pub body: ComponentBody,
    /// Raw keybind mask.
    pub keymask: u32,
    /// Raw keybind entries, in decode order.
    pub keybinds: Vec<crate::interface::Keybind>,
    /// Base right-click op label.
    pub opbase: String,
    /// Right-click op labels, in slot order.
    pub ops: Vec<String>,
    /// Raw op-name high nibble.
    pub opname_nibble: u8,
    /// First op-name block (`index`, `cursor`), if carried.
    pub opname_first: Option<(u8, u16)>,
    /// Second op-name block, if carried.
    pub opname_second: Option<(u8, u16)>,
    /// Pause text (`None` iff absent).
    pub pausetext: Option<String>,
    /// Drag behavior bytes.
    pub dragdeadzone: u8,
    /// Drag behavior bytes.
    pub dragdeadtime: u8,
    /// Drag behavior bytes.
    pub dragrenderbehaviour: u8,
    /// Target verb.
    pub targetverb: String,
    /// Drag-target cursor section, present iff the mask selects it.
    pub target: Option<crate::interface::TargetSection>,
    /// Mouseover cursor (`-1` = none).
    pub mouseovercursor: i32,
    /// Int params (`key` → `value`), in decode order.
    pub int_params: Vec<(u32, i32)>,
    /// String params (`key` → `value`), in decode order.
    pub str_params: Vec<(u32, String)>,
    /// Script hooks, in client order.
    pub hooks: Hooks,
    /// Transmit lists, in client order.
    pub transmits: Transmits,
}

/// The 21 hooks in client decode order, as (`name`, slot) pairs over a borrowed
/// [`Hooks`]. Drives the formatter's fixed hook order. (The codec docs say 22, but the
/// struct and the decoder carry 21 — this table matches the struct field-for-field, so
/// no hook can be silently dropped.)
#[must_use]
pub fn hook_slots(hooks: &Hooks) -> [(&'static str, &Hook); 21] {
    [
        ("onload", &hooks.onload),
        ("onmouseover", &hooks.onmouseover),
        ("onmouseleave", &hooks.onmouseleave),
        ("ontargetleave", &hooks.ontargetleave),
        ("ontargetenter", &hooks.ontargetenter),
        ("onvartransmit", &hooks.onvartransmit),
        ("oninvtransmit", &hooks.oninvtransmit),
        ("onstattransmit", &hooks.onstattransmit),
        ("ontimer", &hooks.ontimer),
        ("onop", &hooks.onop),
        ("onopt", &hooks.onopt),
        ("onmouserepeat", &hooks.onmouserepeat),
        ("onclick", &hooks.onclick),
        ("onclickrepeat", &hooks.onclickrepeat),
        ("onrelease", &hooks.onrelease),
        ("onhold", &hooks.onhold),
        ("ondrag", &hooks.ondrag),
        ("ondragcomplete", &hooks.ondragcomplete),
        ("onscrollwheel", &hooks.onscrollwheel),
        ("onvarctransmit", &hooks.onvarctransmit),
        ("onvarcstrtransmit", &hooks.onvarcstrtransmit),
    ]
}

/// Mutable hook slot by source name; `None` for unknown names (the parser's loud error).
pub fn hook_slot_mut<'a>(hooks: &'a mut Hooks, name: &str) -> Option<&'a mut Hook> {
    match name {
        "onload" => Some(&mut hooks.onload),
        "onmouseover" => Some(&mut hooks.onmouseover),
        "onmouseleave" => Some(&mut hooks.onmouseleave),
        "ontargetleave" => Some(&mut hooks.ontargetleave),
        "ontargetenter" => Some(&mut hooks.ontargetenter),
        "onvartransmit" => Some(&mut hooks.onvartransmit),
        "oninvtransmit" => Some(&mut hooks.oninvtransmit),
        "onstattransmit" => Some(&mut hooks.onstattransmit),
        "ontimer" => Some(&mut hooks.ontimer),
        "onop" => Some(&mut hooks.onop),
        "onopt" => Some(&mut hooks.onopt),
        "onmouserepeat" => Some(&mut hooks.onmouserepeat),
        "onclick" => Some(&mut hooks.onclick),
        "onclickrepeat" => Some(&mut hooks.onclickrepeat),
        "onrelease" => Some(&mut hooks.onrelease),
        "onhold" => Some(&mut hooks.onhold),
        "ondrag" => Some(&mut hooks.ondrag),
        "ondragcomplete" => Some(&mut hooks.ondragcomplete),
        "onscrollwheel" => Some(&mut hooks.onscrollwheel),
        "onvarctransmit" => Some(&mut hooks.onvarctransmit),
        "onvarcstrtransmit" => Some(&mut hooks.onvarcstrtransmit),
        _ => None,
    }
}

/// The 5 transmit lists in client decode order, as (`name`, slot) pairs over a borrowed
/// [`Transmits`]. Drives the formatter's fixed transmit order.
#[must_use]
pub fn transmit_slots(
    transmits: &Transmits,
) -> [(&'static str, &crate::interface::TransmitList); 5] {
    [
        ("var", &transmits.var),
        ("inv", &transmits.inv),
        ("stat", &transmits.stat),
        ("varc", &transmits.varc),
        ("varcstr", &transmits.varcstr),
    ]
}

/// Mutable transmit slot by source name; `None` for unknown names (the parser's loud error).
pub fn transmit_slot_mut<'a>(
    transmits: &'a mut Transmits,
    name: &str,
) -> Option<&'a mut crate::interface::TransmitList> {
    match name {
        "var" => Some(&mut transmits.var),
        "inv" => Some(&mut transmits.inv),
        "stat" => Some(&mut transmits.stat),
        "varc" => Some(&mut transmits.varc),
        "varcstr" => Some(&mut transmits.varcstr),
        _ => None,
    }
}

/// Quote a string with the `source.rs` escaping discipline: `\\ \" \n \r \t`.
#[must_use]
pub fn quote_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for char in text.chars() {
        match char {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(char),
        }
    }
    out.push('"');
    out
}

/// Inverse of [`quote_string`]: parse one double-quoted literal with `\\ \" \n \r \t`
/// escapes. Anything else (missing quotes, bad or dangling escapes) is an error.
pub fn unquote_string(text: &str) -> Result<String> {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| invalid(&format!("bad string {text:?} (expected double quotes)")))?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => {
                return Err(invalid(&format!("bad escape '\\{other}' in {text:?}")));
            }
            None => return Err(invalid(&format!("dangling escape in {text:?}"))),
        }
    }
    Ok(out)
}

/// Canonical `0x`-hex rendering of a colour (uppercase, minimal digits; negatives are
/// two's-complement, so `0xFFFFFFFF` parses back to `-1` exactly).
#[must_use]
pub fn format_colour(value: i32) -> String {
    format!("0x{value:X}")
}

/// Parse a colour: `0x`-hex (two's-complement) or a decimal int. The formatter only ever
/// emits hex; decimal is accepted so hand authors are not forced through a calculator.
pub fn parse_colour(text: &str) -> Result<i32> {
    if let Some(digits) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        if digits.is_empty() {
            return Err(invalid(&format!("bad colour '{text}' (expected '0x'-hex)")));
        }
        u32::from_str_radix(digits, 16).map_or_else(
            |_| Err(invalid(&format!("bad colour '{text}' (expected '0x'-hex)"))),
            |value| Ok(value as i32),
        )
    } else {
        text.parse::<i32>().map_err(|_| {
            invalid(&format!(
                "bad colour '{text}' (expected '0x'-hex or decimal)"
            ))
        })
    }
}

/// Lift a decoded component to editable form. Total over decoded models: decode output
/// only ever carries the six primitive types with matching bodies, so nothing decoded
/// can fail to lift — an unknown `type_id` on a hand-built model falls back to the body
/// discriminant rather than erroring.
#[must_use]
pub fn lift_component(component: &InterfaceComponent) -> ComponentSource {
    let ctype = match ComponentType::from_type_id(component.type_id) {
        Some(ctype) => ctype,
        None => ComponentType::for_body(&component.body),
    };
    ComponentSource {
        ctype,
        version: component.version,
        name: component.name.clone(),
        clientcode: component.clientcode,
        x: component.x,
        y: component.y,
        width: component.width,
        height: component.height,
        width_mode: component.width_mode,
        height_mode: component.height_mode,
        x_mode: component.x_mode,
        y_mode: component.y_mode,
        aspect: component.aspect,
        layer: component.layer,
        hide: component.hide,
        noclickthrough: component.noclickthrough,
        body: component.body.clone(),
        keymask: component.keymask,
        keybinds: component.keybinds.clone(),
        opbase: component.opbase.clone(),
        ops: component.ops.clone(),
        opname_nibble: component.opname_nibble,
        opname_first: component.opname_first,
        opname_second: component.opname_second,
        pausetext: component.pausetext.clone(),
        dragdeadzone: component.dragdeadzone,
        dragdeadtime: component.dragdeadtime,
        dragrenderbehaviour: component.dragrenderbehaviour,
        targetverb: component.targetverb.clone(),
        target: component.target,
        mouseovercursor: component.mouseovercursor,
        int_params: component.int_params.clone(),
        str_params: component.str_params.clone(),
        hooks: component.hooks.clone(),
        transmits: component.transmits.clone(),
    }
}

/// Lower editable form back to a component: validate everything the encoder assumes
/// (versions, ranges, enums, smart ranges, and every cross-field agreement — aspect
/// modes, object dims, transform blocks, op-name blocks, target/keymask, version gates)
/// and rebuild the model. Counts are re-derived by the encoder, so the text carries none.
pub fn lower_component(source: &ComponentSource) -> Result<InterfaceComponent> {
    check_version(source.version)?;
    check_body(source)?;
    check_geometry(source)?;
    check_ops(source)?;
    check_keys_and_params(source)?;
    check_target_and_cursor(source)?;
    check_hooks_and_transmits(source)?;
    check_version_gates(source)?;
    Ok(InterfaceComponent {
        version: source.version,
        type_id: source.ctype.type_id(),
        name: source.name.clone(),
        clientcode: source.clientcode,
        x: source.x,
        y: source.y,
        width: source.width,
        height: source.height,
        width_mode: source.width_mode,
        height_mode: source.height_mode,
        x_mode: source.x_mode,
        y_mode: source.y_mode,
        aspect: source.aspect,
        layer: source.layer,
        hide: source.hide,
        noclickthrough: source.noclickthrough,
        body: source.body.clone(),
        keymask: source.keymask,
        keybinds: source.keybinds.clone(),
        opbase: source.opbase.clone(),
        ops: source.ops.clone(),
        opname_nibble: source.opname_nibble,
        opname_first: source.opname_first,
        opname_second: source.opname_second,
        pausetext: source.pausetext.clone(),
        dragdeadzone: source.dragdeadzone,
        dragdeadtime: source.dragdeadtime,
        dragrenderbehaviour: source.dragrenderbehaviour,
        targetverb: source.targetverb.clone(),
        target: source.target,
        mouseovercursor: source.mouseovercursor,
        int_params: source.int_params.clone(),
        str_params: source.str_params.clone(),
        hooks: source.hooks.clone(),
        transmits: source.transmits.clone(),
    })
}

/// Render canonical source text: the fixed `// interface component` marker, the type
/// header, common fields, the per-type body block, then keybinds, ops, params, all 21
/// hooks and all 5 transmit lists in client order. A hook head (callee) id the registry
/// names renders as `~name`; every other int stays numeric. Infallible: every model
/// value has exactly one canonical spelling.
#[must_use]
pub fn format_component(source: &ComponentSource, symbols: &SymbolRegistry) -> String {
    let mut out = String::new();
    out.push_str("// interface component\n");
    let _ = writeln!(out, "component {};", source.ctype.word());
    let _ = writeln!(out, "version {};", source.version);
    match &source.name {
        None => out.push_str("name none;\n"),
        Some(name) => {
            let _ = writeln!(out, "name {};", quote_string(name));
        }
    }
    let _ = writeln!(out, "clientcode {};", source.clientcode);
    let _ = writeln!(out, "pos {} {};", source.x, source.y);
    let _ = writeln!(out, "size {} {};", source.width, source.height);
    let _ = writeln!(
        out,
        "modes {} {} {} {};",
        source.width_mode, source.height_mode, source.x_mode, source.y_mode
    );
    match source.aspect {
        None => out.push_str("aspect none;\n"),
        Some((width, height)) => {
            let _ = writeln!(out, "aspect {width} {height};");
        }
    }
    let _ = writeln!(out, "layer {};", source.layer);
    let _ = writeln!(out, "hide {};", source.hide);
    let _ = writeln!(out, "noclickthrough {};", source.noclickthrough);
    format_body(&mut out, source);
    let _ = writeln!(out, "keymask {};", source.keymask);
    if source.keybinds.is_empty() {
        out.push_str("keybinds [];\n");
    } else {
        let mut parts = Vec::with_capacity(source.keybinds.len());
        for entry in &source.keybinds {
            parts.push(format!(
                "({}, {}, {}, {})",
                entry.head, entry.lo, entry.key, entry.mods
            ));
        }
        let _ = writeln!(out, "keybinds [{}];", parts.join(", "));
    }
    let _ = writeln!(out, "opbase {};", quote_string(&source.opbase));
    if source.ops.is_empty() {
        out.push_str("ops [];\n");
    } else {
        let quoted: Vec<String> = source.ops.iter().map(|op| quote_string(op)).collect();
        let _ = writeln!(out, "ops [{}];", quoted.join(", "));
    }
    let _ = writeln!(out, "opnames {};", source.opname_nibble);
    if let Some((index, cursor)) = source.opname_first {
        let _ = writeln!(out, "opname1 {index} {cursor};");
    }
    if let Some((index, cursor)) = source.opname_second {
        let _ = writeln!(out, "opname2 {index} {cursor};");
    }
    match &source.pausetext {
        None => out.push_str("pause none;\n"),
        Some(text) => {
            let _ = writeln!(out, "pause {};", quote_string(text));
        }
    }
    let _ = writeln!(
        out,
        "drag {} {} {};",
        source.dragdeadzone, source.dragdeadtime, source.dragrenderbehaviour
    );
    let _ = writeln!(out, "targetverb {};", quote_string(&source.targetverb));
    match source.target {
        None => out.push_str("target none;\n"),
        Some(target) => {
            let _ = writeln!(
                out,
                "target {} {} {};",
                target.param, target.cursor, target.default_cursor
            );
        }
    }
    let _ = writeln!(out, "mouseover {};", source.mouseovercursor);
    if source.int_params.is_empty() {
        out.push_str("intparams [];\n");
    } else {
        let mut parts = Vec::with_capacity(source.int_params.len());
        for (key, value) in &source.int_params {
            parts.push(format!("({key}, {value})"));
        }
        let _ = writeln!(out, "intparams [{}];", parts.join(", "));
    }
    if source.str_params.is_empty() {
        out.push_str("strparams [];\n");
    } else {
        let mut parts = Vec::with_capacity(source.str_params.len());
        for (key, value) in &source.str_params {
            parts.push(format!("({key}, {})", quote_string(value)));
        }
        let _ = writeln!(out, "strparams [{}];", parts.join(", "));
    }
    for (name, hook) in hook_slots(&source.hooks) {
        match hook {
            None => {
                let _ = writeln!(out, "hook {name} none;");
            }
            Some(args) => {
                let mut parts = Vec::with_capacity(args.len());
                for (index, arg) in args.iter().enumerate() {
                    parts.push(format_hook_arg(arg, index == 0, symbols));
                }
                let _ = writeln!(out, "hook {name} [{}];", parts.join(", "));
            }
        }
    }
    for (name, list) in transmit_slots(&source.transmits) {
        match list {
            None => {
                let _ = writeln!(out, "transmit {name} none;");
            }
            Some(entries) => {
                let numbers: Vec<String> = entries
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect();
                let _ = writeln!(out, "transmit {name} [{}];", numbers.join(", "));
            }
        }
    }
    out
}

/// Parse source text, lower, encode, and verify: the output must re-decode to the lowered
/// model exactly (the byte fidelity the corpus gate holds us to). `parentlayer` is the
/// interface id shifted left 16, as in the codec. The dump identity comment is
/// validated back against the assembling group through `names` (see
/// [`crate::inames::check_identity`]) — a moved or stale address fails loudly.
pub fn assemble_component(
    text: &str,
    parentlayer: i32,
    symbols: &SymbolRegistry,
    names: &crate::inames::InterfaceRegistry,
) -> Result<Vec<u8>> {
    let group = parentlayer >> 16;
    crate::inames::check_identity(text, group, names)?;
    let lowered = lower_component(&crate::iparse::parse_component_source(text, symbols)?)?;
    let bytes = encode_component(&lowered, parentlayer)?;
    let decoded = decode_component(&bytes, parentlayer)?;
    if decoded != lowered {
        return Err(invalid(
            "post-assemble verification failed: re-decoded component differs from the lowered model",
        ));
    }
    Ok(bytes)
}

/// Render one hook argument: the head (callee) int folds to `~name` when the registry
/// resolves it; every other int, and every head the registry does not know, stays
/// numeric; strings stay quoted.
fn format_hook_arg(arg: &HookArg, is_head: bool, symbols: &SymbolRegistry) -> String {
    match arg {
        HookArg::Int(value) => {
            if is_head && let Some(name) = symbols.name_for_call(*value) {
                return format!("~{name}");
            }
            value.to_string()
        }
        HookArg::Str(text) => quote_string(text),
    }
}

/// Canonical per-type body block between `noclickthrough` and `keymask`.
fn format_body(out: &mut String, source: &ComponentSource) {
    match &source.body {
        ComponentBody::Layer {
            scroll_width,
            scroll_height,
        } => {
            let _ = writeln!(out, "scroll {scroll_width} {scroll_height};");
        }
        ComponentBody::Rectangle {
            colour,
            fill,
            trans,
        } => {
            let _ = writeln!(out, "colour {};", format_colour(*colour));
            let _ = writeln!(out, "fill {fill};");
            let _ = writeln!(out, "trans {trans};");
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
            let _ = writeln!(out, "font {font};");
            let _ = writeln!(out, "mono {mono};");
            let _ = writeln!(out, "text {};", quote_string(text));
            let _ = writeln!(out, "lineheight {line_height};");
            let _ = writeln!(out, "halign {halign};");
            let _ = writeln!(out, "valign {valign};");
            let _ = writeln!(out, "shadow {shadow};");
            let _ = writeln!(out, "colour {};", format_colour(*colour));
            let _ = writeln!(out, "trans {trans};");
            let _ = writeln!(out, "maxlines {maxlines};");
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
            let _ = writeln!(out, "graphic {graphic};");
            let _ = writeln!(out, "angle {angle};");
            let _ = writeln!(out, "tiling {tiling};");
            let _ = writeln!(out, "alpha {alpha};");
            let _ = writeln!(out, "trans {trans};");
            let _ = writeln!(out, "outline {outline};");
            let _ = writeln!(out, "shadow {};", format_colour(*shadow));
            let _ = writeln!(out, "vflip {vflip};");
            let _ = writeln!(out, "hflip {hflip};");
            let _ = writeln!(out, "colour {};", format_colour(*colour));
            let _ = writeln!(out, "clickmask {clickmask};");
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
            let _ = writeln!(out, "model {id};");
            let _ = writeln!(out, "origin {origin};");
            let _ = writeln!(out, "extended {extended};");
            let _ = writeln!(out, "orthog {orthog};");
            let _ = writeln!(out, "nodepth {nodepth};");
            let _ = writeln!(out, "ox {ox};");
            let _ = writeln!(out, "oy {oy};");
            let _ = writeln!(out, "oz {oz};");
            let _ = writeln!(out, "ax {ax};");
            let _ = writeln!(out, "ay {ay};");
            let _ = writeln!(out, "az {az};");
            let _ = writeln!(out, "zoom {zoom};");
            let _ = writeln!(out, "anim {anim};");
            match objwidth {
                None => out.push_str("objwidth none;\n"),
                Some(value) => {
                    let _ = writeln!(out, "objwidth {value};");
                }
            }
            match objheight {
                None => out.push_str("objheight none;\n"),
                Some(value) => {
                    let _ = writeln!(out, "objheight {value};");
                }
            }
        }
        ComponentBody::Line {
            width,
            colour,
            direction,
        } => {
            let _ = writeln!(out, "linewidth {width};");
            let _ = writeln!(out, "colour {};", format_colour(*colour));
            let _ = writeln!(out, "direction {direction};");
        }
    }
}

/// Whether the key mask selects the drag-target cursor section, as carried in
/// the codec (`mask >> 11 & 0x7F != 0`).
fn target_mask_present(mask: u32) -> bool {
    (mask >> 11) & 0x7F != 0
}

fn invalid(message: &str) -> NativeError {
    NativeError::Invalid(format!("bad interface component source ({message})"))
}

/// A signed smart id (`font`, model `id`/`anim`): only `-1` and non-negative values
/// round-trip through `smart2or4s`, so anything else is rejected rather than silently
/// rewritten by the encoder.
fn check_smart(value: i32, what: &str) -> Result<()> {
    if value == -1 || value >= 0 {
        Ok(())
    } else {
        Err(invalid(&format!(
            "negative smart value {value} for {what} cannot round-trip"
        )))
    }
}

/// A `-1`-means-none id: `-1` or a `u16` the decoder would not read back as none.
fn check_opt_id(value: i32, what: &str) -> Result<()> {
    if value == -1 || (0..=65534).contains(&value) {
        Ok(())
    } else {
        Err(invalid(&format!("{what} id {value} out of range")))
    }
}

fn check_version(version: i32) -> Result<()> {
    if matches!(version, -1 | 3 | 4 | 5) {
        Ok(())
    } else {
        Err(invalid(&format!("unsupported version {version}")))
    }
}

/// Body/type agreement plus every body-local range the encoder assumes.
fn check_body(source: &ComponentSource) -> Result<()> {
    let actual = match &source.body {
        ComponentBody::Layer { .. } => 0,
        ComponentBody::Rectangle { .. } => 3,
        ComponentBody::Text { .. } => 4,
        ComponentBody::Graphic { .. } => 5,
        ComponentBody::Model { .. } => 6,
        ComponentBody::Line { .. } => 9,
    };
    if actual != source.ctype.type_id() {
        return Err(invalid(&format!(
            "body is for type {actual}, component says {}",
            source.ctype.type_id()
        )));
    }
    match &source.body {
        ComponentBody::Layer { .. }
        | ComponentBody::Rectangle { .. }
        | ComponentBody::Line { .. } => Ok(()),
        ComponentBody::Text { font, .. } => check_smart(*font, "font"),
        ComponentBody::Graphic { .. } => Ok(()),
        ComponentBody::Model {
            id,
            origin,
            extended,
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
            ..
        } => {
            check_smart(*id, "model id")?;
            check_smart(*anim, "model anim")?;
            if *origin {
                if *oz != 0 {
                    return Err(invalid(&format!(
                        "model oz {oz} carried without the extended block"
                    )));
                }
                if !(0..=65535).contains(zoom) {
                    return Err(invalid(&format!("model zoom {zoom} out of range")));
                }
            } else if *extended {
                if !(-32768..=32767).contains(zoom) {
                    return Err(invalid(&format!("model zoom {zoom} out of range")));
                }
            } else if *ox != 0
                || *oy != 0
                || *oz != 0
                || *ax != 0
                || *ay != 0
                || *az != 0
                || *zoom != 0
            {
                return Err(invalid("model transform carried without an origin block"));
            }
            match (objwidth, source.width_mode != 0) {
                (Some(_), true) | (None, false) => {}
                _ => {
                    return Err(invalid("model object width disagrees with width mode"));
                }
            }
            match (objheight, source.height_mode != 0) {
                (Some(_), true) | (None, false) => {}
                _ => {
                    return Err(invalid("model object height disagrees with height mode"));
                }
            }
            Ok(())
        }
    }
}

/// Aspect-size agreement, mirroring the encoder exactly.
fn check_geometry(source: &ComponentSource) -> Result<()> {
    match (
        source.aspect,
        source.width_mode == 4 || source.height_mode == 4,
    ) {
        (Some(_), true) | (None, false) => Ok(()),
        (Some(_), false) => Err(invalid("aspect size carried without an aspect size mode")),
        (None, true) => Err(invalid("aspect size mode without aspect size")),
    }
}

/// Op counts and op-name block agreement, mirroring the encoder exactly.
fn check_ops(source: &ComponentSource) -> Result<()> {
    if source.ops.len() > 15 {
        return Err(invalid("op counts out of nibble range"));
    }
    if source.opname_nibble > 15 {
        return Err(invalid("op counts out of nibble range"));
    }
    match (source.opname_nibble, source.opname_first) {
        (0, None) => {}
        (nibble, Some(_)) if nibble > 0 => {}
        _ => {
            return Err(invalid("op-name first block disagrees with nibble"));
        }
    }
    match (
        source.opname_nibble,
        source.opname_first,
        source.opname_second,
    ) {
        (_, _, None) if source.opname_nibble <= 1 => {}
        (nibble, Some(_), Some(_)) if nibble > 1 => {}
        _ => {
            return Err(invalid("op-name second block disagrees with nibble"));
        }
    }
    Ok(())
}

/// Key mask width, keybind heads, and param keys/counts.
fn check_keys_and_params(source: &ComponentSource) -> Result<()> {
    if source.keymask > 0xFF_FFFF {
        return Err(invalid(&format!(
            "key mask {} does not fit in 3 bytes",
            source.keymask
        )));
    }
    for entry in &source.keybinds {
        if entry.head == 0 {
            return Err(invalid("keybind head byte must be nonzero"));
        }
    }
    for (key, _) in &source.int_params {
        if *key > 0xFF_FFFF {
            return Err(invalid(&format!(
                "int param key {key} does not fit in 3 bytes"
            )));
        }
    }
    for (key, _) in &source.str_params {
        if *key > 0xFF_FFFF {
            return Err(invalid(&format!(
                "string param key {key} does not fit in 3 bytes"
            )));
        }
    }
    if source.int_params.len() > 255 {
        return Err(invalid("too many int params"));
    }
    if source.str_params.len() > 255 {
        return Err(invalid("too many string params"));
    }
    Ok(())
}

/// Target section agreement with the key mask, id ranges, and below-zero version gates
/// for params and the mouseover cursor.
fn check_target_and_cursor(source: &ComponentSource) -> Result<()> {
    match (&source.target, target_mask_present(source.keymask)) {
        (Some(_), true) | (None, false) => {}
        _ => {
            return Err(invalid("target section disagrees with key mask"));
        }
    }
    if let Some(target) = &source.target {
        check_opt_id(target.param, "target param")?;
        check_opt_id(target.cursor, "target cursor")?;
        check_opt_id(target.default_cursor, "target default cursor")?;
    }
    check_opt_id(source.mouseovercursor, "mouseover cursor")?;
    if source.version < 0 {
        if !source.int_params.is_empty() || !source.str_params.is_empty() {
            return Err(invalid("params carried below version 0"));
        }
        if source.mouseovercursor != -1 {
            return Err(invalid("mouseover cursor carried below version 0"));
        }
    }
    Ok(())
}

/// Hook/transmit list shapes plus the below-zero gate on `onopt`.
fn check_hooks_and_transmits(source: &ComponentSource) -> Result<()> {
    for (name, hook) in hook_slots(&source.hooks) {
        if let Some(args) = hook {
            if args.is_empty() {
                return Err(invalid(&format!(
                    "hook '{name}' carries an empty list (use none)"
                )));
            }
            if args.len() > 255 {
                return Err(invalid(&format!("too many hook arguments on '{name}'")));
            }
        }
    }
    if source.version < 0 && source.hooks.onopt.is_some() {
        return Err(invalid("onopt hook carried below version 0"));
    }
    for (name, list) in transmit_slots(&source.transmits) {
        if let Some(entries) = list {
            if entries.is_empty() {
                return Err(invalid(&format!(
                    "transmit '{name}' carries an empty list (use none)"
                )));
            }
            if entries.len() > 255 {
                return Err(invalid(&format!("transmit list '{name}' too long")));
            }
        }
    }
    Ok(())
}

/// Version-gated body flags plus the empty-pause normalization guard: the wire cannot
/// distinguish `pause ""` from absent (both re-decode as `None`), so a model carrying
/// `Some("")` could never verify — reject it loudly instead of silently reshaping it.
fn check_version_gates(source: &ComponentSource) -> Result<()> {
    match &source.body {
        ComponentBody::Text { mono, maxlines, .. } => {
            if !*mono && source.version < 2 {
                return Err(invalid(
                    "fontmono false below version 2 (the default is true)",
                ));
            }
            if source.version < 0 && *maxlines != 0 {
                return Err(invalid("maxlines carried below version 0"));
            }
        }
        ComponentBody::Graphic { clickmask, .. } if !*clickmask && source.version < 3 => {
            return Err(invalid(
                "clickmask false below version 3 (the default is true)",
            ));
        }
        _ => {}
    }
    if source.pausetext.as_deref() == Some("") {
        return Err(invalid("pause text is empty (use none)"));
    }
    Ok(())
}

/// What an interfaces-pack dump produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterfaceDumpReport {
    /// Source files written.
    pub files: usize,
    /// Hook call sites rendered as `~names` (proof the registry engaged).
    pub named_hooks: usize,
}

/// Dump every component in the runtime interfaces pack to
/// `<group>_<file>.ifc` files under `out_dir`, creating it, plus the
/// generated `symbols.txt` the `assemble-interface --symbols` verb reads back
/// (identical content to the scripts dump: same registry inputs). Each file
/// opens with a `// interface <group> component <file>` identity comment —
/// curated names rendering where [`crate::inames::InterfaceRegistry`] knows
/// them — which [`assemble_component`] validates back on input. `curated`
/// names ride along when given. A component that fails to lift, format, or
/// re-verify is a hard error naming its group/file — reported, never skipped.
pub fn dump_interfaces_pack(
    pack_root: &std::path::Path,
    out_dir: &std::path::Path,
    curated: &[(i32, String)],
    names: &crate::inames::InterfaceRegistry,
) -> Result<InterfaceDumpReport> {
    dump_selected_interfaces_pack(pack_root, out_dir, curated, names, None)
}

/// Dump one interface group, or the full corpus when no group is selected.
pub fn dump_selected_interfaces_pack(
    pack_root: &std::path::Path,
    out_dir: &std::path::Path,
    curated: &[(i32, String)],
    names: &crate::inames::InterfaceRegistry,
    selected: Option<u32>,
) -> Result<InterfaceDumpReport> {
    use crate::pack::PackArchive;
    let archive = PackArchive::open(&pack_root.join("client.interfaces.js5"))?;
    if let Some(id) = selected
        && !archive.has_group(id)
    {
        return Err(NativeError::Invalid(format!("unknown interface {id}")));
    }
    std::fs::create_dir_all(out_dir).map_err(NativeError::Io)?;
    let roots = selected
        .map(|group| -> Result<std::collections::BTreeSet<i32>> {
            let mut roots = std::collections::BTreeSet::new();
            for bytes in archive.group_files(group)?.unwrap_or_default().values() {
                let component = decode_component(bytes, (group << u16::BITS) as i32)?;
                for (_, hook) in hook_slots(&component.hooks) {
                    if let Some(crate::interface::HookArg::Int(id)) =
                        hook.as_ref().and_then(|args| args.first())
                    {
                        roots.insert(*id);
                    }
                }
            }
            Ok(roots)
        })
        .transpose()?;
    let symbols =
        crate::source::registry_for_scripts_pack_roots(pack_root, curated, roots.as_ref())?;

    let mut report = InterfaceDumpReport {
        files: 0,
        named_hooks: 0,
    };
    for group in archive.group_ids() {
        if selected.is_some_and(|id| id != group) {
            continue;
        }
        let Some(files) = archive.group_files(group)? else {
            continue;
        };
        let parentlayer = (group << 16) as i32;
        for (file, bytes) in &files {
            let decoded = decode_component(bytes, parentlayer)
                .map_err(|error| NativeError::Invalid(format!("{group}/{file}: {error}")))?;
            let source = lift_component(&decoded);
            let mut text = crate::inames::format_identity(
                i32::try_from(group).unwrap_or(i32::MAX),
                *file,
                names,
            );
            text.push_str(&format_component(&source, &symbols));
            for line in text.lines() {
                if line.trim_start().starts_with('~') {
                    report.named_hooks += 1;
                }
            }
            // The text written must be the text that assembles: verify through
            // the same registry before touching disk.
            let rebuilt = assemble_component(&text, parentlayer, &symbols, names)
                .map_err(|error| NativeError::Invalid(format!("{group}/{file}: {error}")))?;
            if &rebuilt != bytes {
                return Err(NativeError::Invalid(format!(
                    "{group}/{file}: interface source round trip changed bytes"
                )));
            }
            std::fs::write(out_dir.join(format!("{group}_{file}.ifc")), text)
                .map_err(NativeError::Io)?;
            report.files += 1;
        }
    }
    std::fs::write(out_dir.join("symbols.txt"), symbols.emit_symbols_txt())
        .map_err(NativeError::Io)?;
    Ok(report)
}
