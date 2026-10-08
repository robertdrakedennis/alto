//! Line parser for interface component text into [`ComponentSource`](crate::isource::ComponentSource).
//!
//! Hand-written, one line at a time, so every failure names its 1-based line. `//`
//! comments run to end of line (a `//` inside a string literal is data, not a comment);
//! blank lines are insignificant. The first statement must be the `component <word>;`
//! header (like the script header in `parse.rs`); everything after is order-free, with
//! body keys parsed type-aware against that header.
//!
//! The parser checks syntax, ranges, enums, duplicates, and `~name` resolution with
//! lines. Whole-model agreement (op-name blocks, aspect/modes, transform blocks,
//! target/keymask, version gates) is checked here too, attributed to the relevant
//! field's line, and [`lower_component`](crate::isource::lower_component) re-validates
//! everything as the safety net for programmatic models (its errors surface at the last
//! content line — unreachable for parsed input, which already passed the precise checks).

use crate::error::{NativeError, Result};
use crate::interface::{Hook, HookArg, Hooks, Keybind, TargetSection, Transmits};
use crate::isource::{
    ComponentSource, ComponentType, hook_slot_mut, lower_component, parse_colour,
    transmit_slot_mut, unquote_string,
};
use crate::source::is_valid_name;
use crate::symbols::SymbolRegistry;
use std::collections::BTreeMap;

/// A component-text parse failure at one line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentParseError {
    /// 1-based line number.
    pub line: usize,
    /// What was wrong.
    pub message: String,
}

impl std::fmt::Display for ComponentParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ComponentParseError {}

impl From<ComponentParseError> for NativeError {
    fn from(error: ComponentParseError) -> Self {
        Self::Invalid(error.to_string())
    }
}

/// Parse component text with `~calls` in hooks resolved through `symbols`. An empty
/// registry parses numeric-only hooks; a populated one also accepts `~name` elements
/// (in any element position — the formatter only ever emits them in the head slot).
pub fn parse_component_source(text: &str, symbols: &SymbolRegistry) -> Result<ComponentSource> {
    Parser::new(symbols).parse(text).map_err(NativeError::from)
}

/// Shorthand for parser-internal results.
type PResult<T> = std::result::Result<T, ComponentParseError>;

/// A required field that may explicitly be `none`: unseen vs seen (with its value).
/// A bare `Option<Option<T>>` trips the `option_option` lint, so the states get names.
#[derive(Clone, Debug)]
enum Present<T> {
    /// No line for this field has been seen.
    Missing,
    /// A line was seen; the inner value is the field's (`None` = explicit `none`).
    Set(Option<T>),
}

impl<T> Present<T> {
    fn set(&mut self, value: Option<T>) {
        *self = Self::Set(value);
    }

    fn take(
        &mut self,
        missing: &dyn Fn(&str) -> ComponentParseError,
        field: &str,
    ) -> PResult<Option<T>> {
        match std::mem::replace(self, Self::Missing) {
            Self::Set(value) => Ok(value),
            Self::Missing => Err(missing(field)),
        }
    }
}

/// Per-type body under construction: every field optional until `finish` requires them.
enum BodyBuilder {
    Layer {
        scroll: Option<(u16, u16)>,
    },
    Rectangle {
        colour: Option<i32>,
        fill: Option<bool>,
        trans: Option<u8>,
    },
    Text {
        font: Option<i32>,
        mono: Option<bool>,
        text: Option<String>,
        lineheight: Option<u8>,
        halign: Option<u8>,
        valign: Option<u8>,
        shadow: Option<bool>,
        colour: Option<i32>,
        trans: Option<u8>,
        maxlines: Option<u8>,
    },
    Graphic {
        graphic: Option<i32>,
        angle: Option<u16>,
        tiling: Option<bool>,
        alpha: Option<bool>,
        trans: Option<u8>,
        outline: Option<u8>,
        shadow: Option<i32>,
        vflip: Option<bool>,
        hflip: Option<bool>,
        colour: Option<i32>,
        clickmask: Option<bool>,
    },
    Model {
        model: Option<i32>,
        origin: Option<bool>,
        extended: Option<bool>,
        orthog: Option<bool>,
        nodepth: Option<bool>,
        ox: Option<i16>,
        oy: Option<i16>,
        oz: Option<i16>,
        ax: Option<u16>,
        ay: Option<u16>,
        az: Option<u16>,
        zoom: Option<i32>,
        anim: Option<i32>,
        objwidth: Present<u16>,
        objheight: Present<u16>,
    },
    Line {
        width: Option<u8>,
        colour: Option<i32>,
        direction: Option<bool>,
    },
}

impl BodyBuilder {
    fn for_type(ctype: ComponentType) -> Self {
        match ctype {
            ComponentType::Layer => Self::Layer { scroll: None },
            ComponentType::Rectangle => Self::Rectangle {
                colour: None,
                fill: None,
                trans: None,
            },
            ComponentType::Text => Self::Text {
                font: None,
                mono: None,
                text: None,
                lineheight: None,
                halign: None,
                valign: None,
                shadow: None,
                colour: None,
                trans: None,
                maxlines: None,
            },
            ComponentType::Graphic => Self::Graphic {
                graphic: None,
                angle: None,
                tiling: None,
                alpha: None,
                trans: None,
                outline: None,
                shadow: None,
                vflip: None,
                hflip: None,
                colour: None,
                clickmask: None,
            },
            ComponentType::Model => Self::Model {
                model: None,
                origin: None,
                extended: None,
                orthog: None,
                nodepth: None,
                ox: None,
                oy: None,
                oz: None,
                ax: None,
                ay: None,
                az: None,
                zoom: None,
                anim: None,
                objwidth: Present::Missing,
                objheight: Present::Missing,
            },
            ComponentType::Line => Self::Line {
                width: None,
                colour: None,
                direction: None,
            },
        }
    }
}

struct Parser<'a> {
    symbols: &'a SymbolRegistry,
    line_no: usize,
    last_no: usize,
    ctype: Option<ComponentType>,
    seen: BTreeMap<String, usize>,
    version: Option<i32>,
    name: Present<String>,
    clientcode: Option<u16>,
    pos: Option<(i16, i16)>,
    size: Option<(u16, u16)>,
    modes: Option<(i8, i8, i8, i8)>,
    aspect: Present<(u16, u16)>,
    layer: Option<i32>,
    hide: Option<bool>,
    noclickthrough: Option<bool>,
    keymask: Option<u32>,
    keybinds: Option<Vec<Keybind>>,
    opbase: Option<String>,
    ops: Option<Vec<String>>,
    opnames: Option<u8>,
    opname1: Option<(u8, u16)>,
    opname2: Option<(u8, u16)>,
    pause: Present<String>,
    drag: Option<(u8, u8, u8)>,
    targetverb: Option<String>,
    target: Present<TargetSection>,
    mouseover: Option<i32>,
    intparams: Option<Vec<(u32, i32)>>,
    strparams: Option<Vec<(u32, String)>>,
    hooks: Hooks,
    transmits: Transmits,
    body: Option<BodyBuilder>,
}

impl<'a> Parser<'a> {
    fn new(symbols: &'a SymbolRegistry) -> Self {
        Self {
            symbols,
            line_no: 0,
            last_no: 1,
            ctype: None,
            seen: BTreeMap::new(),
            version: None,
            name: Present::Missing,
            clientcode: None,
            pos: None,
            size: None,
            modes: None,
            aspect: Present::Missing,
            layer: None,
            hide: None,
            noclickthrough: None,
            keymask: None,
            keybinds: None,
            opbase: None,
            ops: None,
            opnames: None,
            opname1: None,
            opname2: None,
            pause: Present::Missing,
            drag: None,
            targetverb: None,
            target: Present::Missing,
            mouseover: None,
            intparams: None,
            strparams: None,
            hooks: Hooks::default(),
            transmits: Transmits::default(),
            body: None,
        }
    }

    fn parse(mut self, text: &str) -> PResult<ComponentSource> {
        for (position, raw) in text.lines().enumerate() {
            self.line_no = position + 1;
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let code = strip_comment(line).trim();
            if code.is_empty() {
                continue;
            }
            self.last_no = self.line_no;
            if self.ctype.is_none() {
                self.parse_header(code)?;
            } else {
                self.parse_line(code)?;
            }
        }
        self.finish()
    }

    fn bad(&self, message: String) -> ComponentParseError {
        ComponentParseError {
            line: self.line_no,
            message,
        }
    }

    /// Re-wrap a registry/model error's message at the current line.
    fn loud(&self, error: NativeError) -> ComponentParseError {
        self.bad(match error {
            NativeError::Invalid(inner) => inner,
            other => other.to_string(),
        })
    }

    /// Claim a field key: duplicates are errors naming the new line.
    fn claim(&mut self, key: &str) -> PResult<()> {
        if self.seen.contains_key(key) {
            return Err(self.bad(format!("duplicate field '{key}'")));
        }
        self.seen.insert(key.to_string(), self.line_no);
        Ok(())
    }

    fn parse_header(&mut self, code: &str) -> PResult<()> {
        let (key, rest) = split_key(code);
        if key != "component" {
            return Err(self.bad(format!("missing component header, got '{code}'")));
        }
        let word = self.strip_semi(rest, "component")?;
        let ctype = ComponentType::parse_word(word).map_err(|error| self.loud(error))?;
        self.ctype = Some(ctype);
        self.body = Some(BodyBuilder::for_type(ctype));
        self.seen.insert("component".to_string(), self.line_no);
        Ok(())
    }

    fn parse_line(&mut self, code: &str) -> PResult<()> {
        let (key, rest) = split_key(code);
        if key == "hook" {
            return self.parse_hook_line(rest);
        }
        if key == "transmit" {
            return self.parse_transmit_line(rest);
        }
        if key == "component" {
            return Err(self.bad("duplicate component header".to_string()));
        }
        if self.parse_body_key(key, rest)? {
            return Ok(());
        }
        self.parse_common_key(key, rest)
    }

    /// Type-aware body dispatch: each per-type helper owns its keys and reports whether
    /// the key was its. A body key from another type falls through every helper and
    /// lands in `parse_common_key`, which rejects it as unknown.
    fn parse_body_key(&mut self, key: &str, rest: &str) -> PResult<bool> {
        match self.ctype {
            Some(ComponentType::Layer) => self.parse_layer_body(key, rest),
            Some(ComponentType::Rectangle) => self.parse_rectangle_body(key, rest),
            Some(ComponentType::Text) => self.parse_text_body(key, rest),
            Some(ComponentType::Graphic) => self.parse_graphic_body(key, rest),
            Some(ComponentType::Model) => self.parse_model_body(key, rest),
            Some(ComponentType::Line) => self.parse_line_body(key, rest),
            None => Err(self.bad("missing component header".to_string())),
        }
    }

    fn parse_common_key(&mut self, key: &str, rest: &str) -> PResult<()> {
        match key {
            "version" | "name" | "clientcode" | "pos" | "size" | "modes" | "aspect" | "layer" => {
                self.parse_identity_key(key, rest)
            }
            "hide" | "noclickthrough" | "keymask" | "keybinds" | "drag" => {
                self.parse_flag_key(key, rest)
            }
            "opbase" | "ops" | "opnames" | "opname1" | "opname2" | "pause" | "targetverb"
            | "target" | "mouseover" => self.parse_detail_key(key, rest),
            "intparams" | "strparams" => self.parse_param_key(key, rest),
            _ => Err(self.bad(format!("unknown field '{key}'"))),
        }
    }

    fn parse_identity_key(&mut self, key: &str, rest: &str) -> PResult<()> {
        match key {
            "version" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_i32(text, key)?;
                if !matches!(value, -1 | 3 | 4 | 5) {
                    return Err(self.bad(format!("unsupported version {value}")));
                }
                self.version = Some(value);
            }
            "name" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.name.set(self.parse_opt_string(text)?);
            }
            "clientcode" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.clientcode = Some(self.parse_u16(text, key)?);
            }
            "pos" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.pos = Some(self.parse_i16_pair(text, key)?);
            }
            "size" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.size = Some(self.parse_u16_pair(text, key)?);
            }
            "modes" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.modes = Some(self.parse_i8_quad(text, key)?);
            }
            "aspect" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                if text == "none" {
                    self.aspect.set(None);
                } else {
                    self.aspect.set(Some(self.parse_u16_pair(text, key)?));
                }
            }
            "layer" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.layer = Some(self.parse_i32(text, key)?);
            }
            _ => {
                return Err(self.bad(format!("unknown field '{key}'")));
            }
        }
        Ok(())
    }

    fn parse_flag_key(&mut self, key: &str, rest: &str) -> PResult<()> {
        match key {
            "hide" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.hide = Some(self.parse_bool(text, key)?);
            }
            "noclickthrough" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.noclickthrough = Some(self.parse_bool(text, key)?);
            }
            "keymask" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u32(text, key)?;
                if value > 0xFF_FFFF {
                    return Err(self.bad(format!("key mask '{text}' does not fit in 3 bytes")));
                }
                self.keymask = Some(value);
            }
            "keybinds" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.keybinds = Some(self.parse_keybinds(text)?);
            }
            "drag" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.drag = Some(self.parse_u8_triple(text, key)?);
            }
            _ => {
                return Err(self.bad(format!("unknown field '{key}'")));
            }
        }
        Ok(())
    }

    fn parse_detail_key(&mut self, key: &str, rest: &str) -> PResult<()> {
        match key {
            "opbase" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.opbase = Some(self.parse_string(text)?);
            }
            "ops" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let items = self.parse_list(text, key)?;
                let mut ops = Vec::with_capacity(items.len());
                for item in items {
                    ops.push(self.parse_string(item)?);
                }
                if ops.len() > 15 {
                    return Err(self.bad("too many ops (the nibble holds 15)".to_string()));
                }
                self.ops = Some(ops);
            }
            "opnames" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u8(text, key)?;
                if value > 15 {
                    return Err(self.bad(format!("op-name nibble '{text}' out of range")));
                }
                self.opnames = Some(value);
            }
            "opname1" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.opname1 = Some(self.parse_u8_u16_pair(text, key)?);
            }
            "opname2" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.opname2 = Some(self.parse_u8_u16_pair(text, key)?);
            }
            "pause" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let pause = self.parse_opt_string(text)?;
                // The wire cannot distinguish empty from absent (both re-decode as
                // missing), so normalize early — the formatter then stays a fixpoint.
                self.pause.set(pause.filter(|pause| !pause.is_empty()));
            }
            "targetverb" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.targetverb = Some(self.parse_string(text)?);
            }
            "target" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                if text == "none" {
                    self.target.set(None);
                } else {
                    self.target.set(Some(self.parse_target(text)?));
                }
            }
            "mouseover" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.mouseover = Some(self.parse_opt_id(text, "mouseover cursor")?);
            }
            _ => {
                return Err(self.bad(format!("unknown field '{key}'")));
            }
        }
        Ok(())
    }

    fn parse_param_key(&mut self, key: &str, rest: &str) -> PResult<()> {
        match key {
            "intparams" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.intparams = Some(self.parse_intparams(text)?);
            }
            "strparams" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                self.strparams = Some(self.parse_strparams(text)?);
            }
            _ => {
                return Err(self.bad(format!("unknown field '{key}'")));
            }
        }
        Ok(())
    }

    fn parse_hook_line(&mut self, rest: &str) -> PResult<()> {
        let (name, value) = split_key(rest);
        if name.is_empty() {
            return Err(self.bad("malformed hook line (expected 'hook <name> ...;')".to_string()));
        }
        if hook_slot_mut(&mut self.hooks, name).is_none() {
            return Err(self.bad(format!("unknown hook '{name}'")));
        }
        let seen_key = format!("hook {name}");
        self.claim(&seen_key)?;
        let text = self.strip_semi(value, "hook")?;
        let hook = if text == "none" {
            None
        } else {
            self.parse_hook_list(text)?
        };
        if let Some(slot) = hook_slot_mut(&mut self.hooks, name) {
            *slot = hook;
        }
        Ok(())
    }

    fn parse_transmit_line(&mut self, rest: &str) -> PResult<()> {
        let (name, value) = split_key(rest);
        if name.is_empty() {
            return Err(
                self.bad("malformed transmit line (expected 'transmit <name> ...;')".to_string())
            );
        }
        if transmit_slot_mut(&mut self.transmits, name).is_none() {
            return Err(self.bad(format!("unknown transmit list '{name}'")));
        }
        let seen_key = format!("transmit {name}");
        self.claim(&seen_key)?;
        let text = self.strip_semi(value, "transmit")?;
        let list = if text == "none" {
            Vec::new()
        } else {
            self.parse_int_list(text, "transmit")?
        };
        if let Some(slot) = transmit_slot_mut(&mut self.transmits, name) {
            // An empty list encodes to a zero count, which re-decodes as absent —
            // normalize early so the formatter stays a fixpoint.
            *slot = if list.is_empty() { None } else { Some(list) };
        }
        Ok(())
    }

    fn parse_layer_body(&mut self, key: &str, rest: &str) -> PResult<bool> {
        if key != "scroll" {
            return Ok(false);
        }
        self.claim(key)?;
        let text = self.strip_semi(rest, key)?;
        let value = self.parse_u16_pair(text, key)?;
        match self.body_mut()? {
            BodyBuilder::Layer { scroll } => *scroll = Some(value),
            _ => return Err(self.bad("body disagrees with component type".to_string())),
        }
        Ok(true)
    }

    fn parse_rectangle_body(&mut self, key: &str, rest: &str) -> PResult<bool> {
        match key {
            "colour" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_colour_field(text)?;
                match self.body_mut()? {
                    BodyBuilder::Rectangle { colour, .. } => *colour = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "fill" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_bool(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Rectangle { fill, .. } => *fill = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "trans" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u8(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Rectangle { trans, .. } => *trans = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn parse_text_body(&mut self, key: &str, rest: &str) -> PResult<bool> {
        match key {
            "font" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_smart(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Text { font, .. } => *font = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "mono" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_bool(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Text { mono, .. } => *mono = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "text" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_string(text)?;
                match self.body_mut()? {
                    BodyBuilder::Text { text: slot, .. } => *slot = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "lineheight" | "halign" | "valign" | "trans" | "maxlines" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u8(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Text {
                        lineheight,
                        halign,
                        valign,
                        trans,
                        maxlines,
                        ..
                    } => match key {
                        "lineheight" => *lineheight = Some(value),
                        "halign" => *halign = Some(value),
                        "valign" => *valign = Some(value),
                        "trans" => *trans = Some(value),
                        _ => *maxlines = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "shadow" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_bool(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Text { shadow, .. } => *shadow = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "colour" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_colour_field(text)?;
                match self.body_mut()? {
                    BodyBuilder::Text { colour, .. } => *colour = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn parse_graphic_body(&mut self, key: &str, rest: &str) -> PResult<bool> {
        match key {
            "graphic" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_i32(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Graphic { graphic, .. } => *graphic = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "angle" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u16(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Graphic { angle, .. } => *angle = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "tiling" | "alpha" | "vflip" | "hflip" | "clickmask" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_bool(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Graphic {
                        tiling,
                        alpha,
                        vflip,
                        hflip,
                        clickmask,
                        ..
                    } => match key {
                        "tiling" => *tiling = Some(value),
                        "alpha" => *alpha = Some(value),
                        "vflip" => *vflip = Some(value),
                        "hflip" => *hflip = Some(value),
                        _ => *clickmask = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "trans" | "outline" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u8(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Graphic { trans, outline, .. } => match key {
                        "trans" => *trans = Some(value),
                        _ => *outline = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "shadow" | "colour" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_colour_field(text)?;
                match self.body_mut()? {
                    BodyBuilder::Graphic { shadow, colour, .. } => match key {
                        "shadow" => *shadow = Some(value),
                        _ => *colour = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn parse_model_body(&mut self, key: &str, rest: &str) -> PResult<bool> {
        match key {
            "model" | "anim" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_smart(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Model { model, anim, .. } => match key {
                        "model" => *model = Some(value),
                        _ => *anim = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "origin" | "extended" | "orthog" | "nodepth" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_bool(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Model {
                        origin,
                        extended,
                        orthog,
                        nodepth,
                        ..
                    } => match key {
                        "origin" => *origin = Some(value),
                        "extended" => *extended = Some(value),
                        "orthog" => *orthog = Some(value),
                        _ => *nodepth = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "ox" | "oy" | "oz" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_i16(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Model { ox, oy, oz, .. } => match key {
                        "ox" => *ox = Some(value),
                        "oy" => *oy = Some(value),
                        _ => *oz = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "ax" | "ay" | "az" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u16(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Model { ax, ay, az, .. } => match key {
                        "ax" => *ax = Some(value),
                        "ay" => *ay = Some(value),
                        _ => *az = Some(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "zoom" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_i32(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Model { zoom, .. } => *zoom = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "objwidth" | "objheight" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = if text == "none" {
                    None
                } else {
                    Some(self.parse_u16(text, key)?)
                };
                match self.body_mut()? {
                    BodyBuilder::Model {
                        objwidth,
                        objheight,
                        ..
                    } => match key {
                        "objwidth" => objwidth.set(value),
                        _ => objheight.set(value),
                    },
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn parse_line_body(&mut self, key: &str, rest: &str) -> PResult<bool> {
        match key {
            "linewidth" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_u8(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Line { width, .. } => *width = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "colour" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_colour_field(text)?;
                match self.body_mut()? {
                    BodyBuilder::Line { colour, .. } => *colour = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            "direction" => {
                self.claim(key)?;
                let text = self.strip_semi(rest, key)?;
                let value = self.parse_bool(text, key)?;
                match self.body_mut()? {
                    BodyBuilder::Line { direction, .. } => *direction = Some(value),
                    _ => return Err(self.bad("body disagrees with component type".to_string())),
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn body_mut(&mut self) -> PResult<&mut BodyBuilder> {
        let line = self.line_no;
        match self.body.as_mut() {
            Some(body) => Ok(body),
            None => Err(ComponentParseError {
                line,
                message: "missing component header".to_string(),
            }),
        }
    }

    fn finish(mut self) -> PResult<ComponentSource> {
        let ctype = self.ctype.take().ok_or_else(|| ComponentParseError {
            line: 1,
            message: "missing component header".to_string(),
        })?;
        let last = self.last_no;
        let missing = |field: &str| ComponentParseError {
            line: last,
            message: format!("missing required field '{field}'"),
        };
        let version = self.version.take().ok_or_else(|| missing("version"))?;
        let name = self.name.take(&missing, "name")?;
        let clientcode = self
            .clientcode
            .take()
            .ok_or_else(|| missing("clientcode"))?;
        let (x, y) = self.pos.take().ok_or_else(|| missing("pos"))?;
        let (width, height) = self.size.take().ok_or_else(|| missing("size"))?;
        let (width_mode, height_mode, x_mode, y_mode) =
            self.modes.take().ok_or_else(|| missing("modes"))?;
        let aspect = self.aspect.take(&missing, "aspect")?;
        let layer = self.layer.take().ok_or_else(|| missing("layer"))?;
        let hide = self.hide.take().ok_or_else(|| missing("hide"))?;
        let noclickthrough = self
            .noclickthrough
            .take()
            .ok_or_else(|| missing("noclickthrough"))?;
        let keymask = self.keymask.take().ok_or_else(|| missing("keymask"))?;
        let keybinds = self.keybinds.take().ok_or_else(|| missing("keybinds"))?;
        let opbase = self.opbase.take().ok_or_else(|| missing("opbase"))?;
        let ops = self.ops.take().ok_or_else(|| missing("ops"))?;
        let opname_nibble = self.opnames.take().ok_or_else(|| missing("opnames"))?;
        let pause = self.pause.take(&missing, "pause")?;
        let (dragdeadzone, dragdeadtime, dragrenderbehaviour) =
            self.drag.take().ok_or_else(|| missing("drag"))?;
        let targetverb = self
            .targetverb
            .take()
            .ok_or_else(|| missing("targetverb"))?;
        let target = self.target.take(&missing, "target")?;
        let mouseovercursor = self.mouseover.take().ok_or_else(|| missing("mouseover"))?;
        let int_params = self.intparams.take().ok_or_else(|| missing("intparams"))?;
        let str_params = self.strparams.take().ok_or_else(|| missing("strparams"))?;
        let body = self.finish_body(ctype, &missing)?;
        let opname_first = self.opname1.take();
        let opname_second = self.opname2.take();
        let hooks = std::mem::take(&mut self.hooks);
        let transmits = std::mem::take(&mut self.transmits);
        let field_line = |key: &str| self.seen.get(key).copied().unwrap_or(last);
        let built = ComponentSource {
            ctype,
            version,
            name,
            clientcode,
            x,
            y,
            width,
            height,
            width_mode,
            height_mode,
            x_mode,
            y_mode,
            aspect,
            layer,
            hide,
            noclickthrough,
            body,
            keymask,
            keybinds,
            opbase,
            ops,
            opname_nibble,
            opname_first,
            opname_second,
            pausetext: pause,
            dragdeadzone,
            dragdeadtime,
            dragrenderbehaviour,
            targetverb,
            target,
            mouseovercursor,
            int_params,
            str_params,
            hooks,
            transmits,
        };
        self.finish_agreement(&built, field_line)?;
        // Safety net for programmatic models: parsed input already passed the precise
        // checks above, so a failure here surfaces at the last content line.
        lower_component(&built).map_err(|error| ComponentParseError {
            line: last,
            message: match error {
                NativeError::Invalid(inner) => inner,
                other => other.to_string(),
            },
        })?;
        Ok(built)
    }

    /// Whole-model agreement checks attributed to the relevant field's line.
    fn finish_agreement(
        &self,
        built: &ComponentSource,
        field_line: impl Fn(&str) -> usize,
    ) -> PResult<()> {
        let at = |key: &str, message: &str| ComponentParseError {
            line: field_line(key),
            message: message.to_string(),
        };
        match (built.opname_nibble, built.opname_first) {
            (0, None) => {}
            (nibble, Some(_)) if nibble > 0 => {}
            _ => {
                return Err(at("opnames", "op-name first block disagrees with nibble"));
            }
        }
        match (built.opname_nibble, built.opname_first, built.opname_second) {
            (_, _, None) if built.opname_nibble <= 1 => {}
            (nibble, Some(_), Some(_)) if nibble > 1 => {}
            _ => {
                return Err(at("opnames", "op-name second block disagrees with nibble"));
            }
        }
        match (
            built.aspect,
            built.width_mode == 4 || built.height_mode == 4,
        ) {
            (Some(_), true) | (None, false) => {}
            (Some(_), false) => {
                return Err(at(
                    "aspect",
                    "aspect size carried without an aspect size mode",
                ));
            }
            (None, true) => {
                return Err(at("aspect", "aspect size mode without aspect size"));
            }
        }
        let want_target = (built.keymask >> 11) & 0x7F != 0;
        match (&built.target, want_target) {
            (Some(_), true) | (None, false) => {}
            (Some(_), false) => {
                return Err(at("target", "target section disagrees with key mask"));
            }
            (None, true) => {
                return Err(at("keymask", "target section disagrees with key mask"));
            }
        }
        if built.version < 0 {
            if !built.int_params.is_empty() {
                return Err(at("intparams", "params carried below version 0"));
            }
            if !built.str_params.is_empty() {
                return Err(at("strparams", "params carried below version 0"));
            }
            if built.mouseovercursor != -1 {
                return Err(at("mouseover", "mouseover cursor carried below version 0"));
            }
            if built.hooks.onopt.is_some() {
                return Err(at("hook onopt", "onopt hook carried below version 0"));
            }
        }
        Self::finish_body_agreement(built, &at)
    }

    /// Body-local agreement (transform blocks, object dims, version-gated flags).
    fn finish_body_agreement(
        built: &ComponentSource,
        at: &dyn Fn(&str, &str) -> ComponentParseError,
    ) -> PResult<()> {
        match &built.body {
            crate::interface::ComponentBody::Text { mono, maxlines, .. } => {
                if !*mono && built.version < 2 {
                    return Err(at(
                        "mono",
                        "fontmono false below version 2 (the default is true)",
                    ));
                }
                if built.version < 0 && *maxlines != 0 {
                    return Err(at("maxlines", "maxlines carried below version 0"));
                }
            }
            crate::interface::ComponentBody::Graphic { clickmask, .. } => {
                if !*clickmask && built.version < 3 {
                    return Err(at(
                        "clickmask",
                        "clickmask false below version 3 (the default is true)",
                    ));
                }
            }
            crate::interface::ComponentBody::Model {
                origin,
                extended,
                ox,
                oy,
                oz,
                ax,
                ay,
                az,
                zoom,
                objwidth,
                objheight,
                ..
            } => {
                if *origin {
                    if *oz != 0 {
                        return Err(at("oz", "model oz carried without the extended block"));
                    }
                    if !(0..=65535).contains(zoom) {
                        return Err(at("zoom", "model zoom out of range"));
                    }
                } else if *extended {
                    if !(-32768..=32767).contains(zoom) {
                        return Err(at("zoom", "model zoom out of range"));
                    }
                } else if *ox != 0
                    || *oy != 0
                    || *oz != 0
                    || *ax != 0
                    || *ay != 0
                    || *az != 0
                    || *zoom != 0
                {
                    return Err(at("ox", "model transform carried without an origin block"));
                }
                match (objwidth, built.width_mode != 0) {
                    (Some(_), true) | (None, false) => {}
                    _ => {
                        return Err(at(
                            "objwidth",
                            "model object width disagrees with width mode",
                        ));
                    }
                }
                match (objheight, built.height_mode != 0) {
                    (Some(_), true) | (None, false) => {}
                    _ => {
                        return Err(at(
                            "objheight",
                            "model object height disagrees with height mode",
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_body(
        &mut self,
        ctype: ComponentType,
        missing: &dyn Fn(&str) -> ComponentParseError,
    ) -> PResult<crate::interface::ComponentBody> {
        let body = self.body.take().ok_or_else(|| missing("component"))?;
        match (ctype, body) {
            (ComponentType::Layer, BodyBuilder::Layer { scroll }) => {
                let (scroll_width, scroll_height) = scroll.ok_or_else(|| missing("scroll"))?;
                Ok(crate::interface::ComponentBody::Layer {
                    scroll_width,
                    scroll_height,
                })
            }
            (
                ComponentType::Rectangle,
                BodyBuilder::Rectangle {
                    colour,
                    fill,
                    trans,
                },
            ) => Ok(crate::interface::ComponentBody::Rectangle {
                colour: colour.ok_or_else(|| missing("colour"))?,
                fill: fill.ok_or_else(|| missing("fill"))?,
                trans: trans.ok_or_else(|| missing("trans"))?,
            }),
            (
                ComponentType::Text,
                BodyBuilder::Text {
                    font,
                    mono,
                    text,
                    lineheight,
                    halign,
                    valign,
                    shadow,
                    colour,
                    trans,
                    maxlines,
                },
            ) => Ok(crate::interface::ComponentBody::Text {
                font: font.ok_or_else(|| missing("font"))?,
                mono: mono.ok_or_else(|| missing("mono"))?,
                text: text.ok_or_else(|| missing("text"))?,
                line_height: lineheight.ok_or_else(|| missing("lineheight"))?,
                halign: halign.ok_or_else(|| missing("halign"))?,
                valign: valign.ok_or_else(|| missing("valign"))?,
                shadow: shadow.ok_or_else(|| missing("shadow"))?,
                colour: colour.ok_or_else(|| missing("colour"))?,
                trans: trans.ok_or_else(|| missing("trans"))?,
                maxlines: maxlines.ok_or_else(|| missing("maxlines"))?,
            }),
            (
                ComponentType::Graphic,
                BodyBuilder::Graphic {
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
                },
            ) => Ok(crate::interface::ComponentBody::Graphic {
                graphic: graphic.ok_or_else(|| missing("graphic"))?,
                angle: angle.ok_or_else(|| missing("angle"))?,
                tiling: tiling.ok_or_else(|| missing("tiling"))?,
                alpha: alpha.ok_or_else(|| missing("alpha"))?,
                trans: trans.ok_or_else(|| missing("trans"))?,
                outline: outline.ok_or_else(|| missing("outline"))?,
                shadow: shadow.ok_or_else(|| missing("shadow"))?,
                vflip: vflip.ok_or_else(|| missing("vflip"))?,
                hflip: hflip.ok_or_else(|| missing("hflip"))?,
                colour: colour.ok_or_else(|| missing("colour"))?,
                clickmask: clickmask.ok_or_else(|| missing("clickmask"))?,
            }),
            (
                ComponentType::Model,
                BodyBuilder::Model {
                    model,
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
                },
            ) => Ok(crate::interface::ComponentBody::Model {
                id: model.ok_or_else(|| missing("model"))?,
                origin: origin.ok_or_else(|| missing("origin"))?,
                extended: extended.ok_or_else(|| missing("extended"))?,
                orthog: orthog.ok_or_else(|| missing("orthog"))?,
                nodepth: nodepth.ok_or_else(|| missing("nodepth"))?,
                ox: ox.ok_or_else(|| missing("ox"))?,
                oy: oy.ok_or_else(|| missing("oy"))?,
                oz: oz.ok_or_else(|| missing("oz"))?,
                ax: ax.ok_or_else(|| missing("ax"))?,
                ay: ay.ok_or_else(|| missing("ay"))?,
                az: az.ok_or_else(|| missing("az"))?,
                zoom: zoom.ok_or_else(|| missing("zoom"))?,
                anim: anim.ok_or_else(|| missing("anim"))?,
                objwidth: match objwidth {
                    Present::Set(value) => value,
                    Present::Missing => return Err(missing("objwidth")),
                },
                objheight: match objheight {
                    Present::Set(value) => value,
                    Present::Missing => return Err(missing("objheight")),
                },
            }),
            (
                ComponentType::Line,
                BodyBuilder::Line {
                    width,
                    colour,
                    direction,
                },
            ) => Ok(crate::interface::ComponentBody::Line {
                width: width.ok_or_else(|| missing("linewidth"))?,
                colour: colour.ok_or_else(|| missing("colour"))?,
                direction: direction.ok_or_else(|| missing("direction"))?,
            }),
            _ => Err(missing("component")),
        }
    }

    fn strip_semi<'b>(&self, text: &'b str, what: &str) -> PResult<&'b str> {
        text.strip_suffix(';')
            .map(str::trim)
            .ok_or_else(|| self.bad(format!("{what} misses its ';'")))
    }

    fn parse_i32(&self, text: &str, what: &str) -> PResult<i32> {
        text.parse()
            .map_err(|_| self.bad(format!("bad {what} '{text}' (expected an int)")))
    }

    fn parse_u16(&self, text: &str, what: &str) -> PResult<u16> {
        text.parse()
            .map_err(|_| self.bad(format!("bad {what} '{text}' (expected 0..=65535)")))
    }

    fn parse_u8(&self, text: &str, what: &str) -> PResult<u8> {
        text.parse()
            .map_err(|_| self.bad(format!("bad {what} '{text}' (expected 0..=255)")))
    }

    fn parse_i16(&self, text: &str, what: &str) -> PResult<i16> {
        text.parse()
            .map_err(|_| self.bad(format!("bad {what} '{text}' (expected -32768..=32767)")))
    }

    fn parse_i8(&self, text: &str, what: &str) -> PResult<i8> {
        text.parse()
            .map_err(|_| self.bad(format!("bad {what} '{text}' (expected -128..=127)")))
    }

    fn parse_u32(&self, text: &str, what: &str) -> PResult<u32> {
        text.parse()
            .map_err(|_| self.bad(format!("bad {what} '{text}' (expected an unsigned int)")))
    }

    fn parse_bool(&self, text: &str, what: &str) -> PResult<bool> {
        match text {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(self.bad(format!("bad {what} '{text}' (expected true or false)"))),
        }
    }

    fn parse_string(&self, text: &str) -> PResult<String> {
        unquote_string(text).map_err(|error| self.loud(error))
    }

    fn parse_opt_string(&self, text: &str) -> PResult<Option<String>> {
        if text == "none" {
            return Ok(None);
        }
        self.parse_string(text).map(Some)
    }

    fn parse_colour_field(&self, text: &str) -> PResult<i32> {
        parse_colour(text).map_err(|error| self.loud(error))
    }

    /// A signed smart id (`font`, model `id`/`anim`): only `-1` and non-negative values
    /// round-trip, so anything else fails at its line rather than in the encoder.
    fn parse_smart(&self, text: &str, what: &str) -> PResult<i32> {
        let value = self.parse_i32(text, what)?;
        if value == -1 || value >= 0 {
            Ok(value)
        } else {
            Err(self.bad(format!(
                "negative smart value {value} for {what} cannot round-trip"
            )))
        }
    }

    /// A `-1`-means-none id: `-1` or a `u16` the decoder would not read back as none.
    fn parse_opt_id(&self, text: &str, what: &str) -> PResult<i32> {
        let value = self.parse_i32(text, what)?;
        if value == -1 || (0..=65534).contains(&value) {
            Ok(value)
        } else {
            Err(self.bad(format!("{what} '{text}' out of range")))
        }
    }

    fn parse_i16_pair(&self, text: &str, what: &str) -> PResult<(i16, i16)> {
        let (first, second) = self.parse_word_pair(text, what)?;
        Ok((self.parse_i16(first, what)?, self.parse_i16(second, what)?))
    }

    fn parse_u16_pair(&self, text: &str, what: &str) -> PResult<(u16, u16)> {
        let (first, second) = self.parse_word_pair(text, what)?;
        Ok((self.parse_u16(first, what)?, self.parse_u16(second, what)?))
    }

    fn parse_u8_u16_pair(&self, text: &str, what: &str) -> PResult<(u8, u16)> {
        let (first, second) = self.parse_word_pair(text, what)?;
        Ok((self.parse_u8(first, what)?, self.parse_u16(second, what)?))
    }

    fn parse_u8_triple(&self, text: &str, what: &str) -> PResult<(u8, u8, u8)> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(self.bad(format!("bad {what} '{text}' (expected three ints)")));
        }
        Ok((
            self.parse_u8(parts[0], what)?,
            self.parse_u8(parts[1], what)?,
            self.parse_u8(parts[2], what)?,
        ))
    }

    fn parse_i8_quad(&self, text: &str, what: &str) -> PResult<(i8, i8, i8, i8)> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() != 4 {
            return Err(self.bad(format!("bad {what} '{text}' (expected four ints)")));
        }
        Ok((
            self.parse_i8(parts[0], what)?,
            self.parse_i8(parts[1], what)?,
            self.parse_i8(parts[2], what)?,
            self.parse_i8(parts[3], what)?,
        ))
    }

    fn parse_word_pair<'b>(&self, text: &'b str, what: &str) -> PResult<(&'b str, &'b str)> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(self.bad(format!("bad {what} '{text}' (expected two ints)")));
        }
        Ok((parts[0], parts[1]))
    }

    fn parse_target(&self, text: &str) -> PResult<TargetSection> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(self.bad(format!("bad target '{text}' (expected three ids or none)")));
        }
        Ok(TargetSection {
            param: self.parse_opt_id(parts[0], "target param")?,
            cursor: self.parse_opt_id(parts[1], "target cursor")?,
            default_cursor: self.parse_opt_id(parts[2], "target default cursor")?,
        })
    }

    /// A `[...]` list split on top-level commas (quote- and depth-aware, so game text
    /// containing commas, brackets, or `//` survives).
    fn parse_list<'b>(&self, text: &'b str, what: &str) -> PResult<Vec<&'b str>> {
        let inner = text
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or_else(|| self.bad(format!("bad {what} '{text}' (expected '[...]')")))?;
        let inner = inner.trim();
        if inner.is_empty() {
            return Ok(Vec::new());
        }
        split_top_level(inner)
            .map_err(|message| self.bad(format!("bad {what} '{text}': {message}")))
    }

    /// A `(a, b)` pair split on the single top-level comma.
    fn parse_tuple<'b>(&self, text: &'b str, what: &str) -> PResult<(&'b str, &'b str)> {
        let inner = text
            .strip_prefix('(')
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| self.bad(format!("bad {what} '{text}' (expected '(a, b)')")))?;
        let parts = split_top_level(inner)
            .map_err(|message| self.bad(format!("bad {what} '{text}': {message}")))?;
        if parts.len() != 2 {
            return Err(self.bad(format!("bad {what} '{text}' (expected '(a, b)')")));
        }
        Ok((parts[0], parts[1]))
    }

    fn parse_keybinds(&self, text: &str) -> PResult<Vec<Keybind>> {
        let mut out = Vec::new();
        for item in self.parse_list(text, "keybinds")? {
            let inner = item
                .strip_prefix('(')
                .and_then(|rest| rest.strip_suffix(')'))
                .ok_or_else(|| {
                    self.bad(format!(
                        "bad keybind '{item}' (expected '(head, lo, key, mods)')"
                    ))
                })?;
            let parts = split_top_level(inner)
                .map_err(|message| self.bad(format!("bad keybind '{item}': {message}")))?;
            if parts.len() != 4 {
                return Err(self.bad(format!(
                    "bad keybind '{item}' (expected '(head, lo, key, mods)')"
                )));
            }
            let head = self.parse_u8(parts[0], "keybind head")?;
            if head == 0 {
                return Err(self.bad("keybind head byte must be nonzero".to_string()));
            }
            out.push(Keybind {
                head,
                lo: self.parse_u8(parts[1], "keybind lo")?,
                key: self.parse_i8(parts[2], "keybind key")?,
                mods: self.parse_i8(parts[3], "keybind mods")?,
            });
        }
        Ok(out)
    }

    fn parse_hook_list(&self, text: &str) -> PResult<Hook> {
        let items = self.parse_list(text, "hook")?;
        if items.is_empty() {
            // An empty list encodes to a zero count, which re-decodes as absent —
            // normalize early so the formatter stays a fixpoint.
            return Ok(None);
        }
        let mut args = Vec::with_capacity(items.len());
        for item in items {
            args.push(self.parse_hook_arg(item)?);
        }
        if args.len() > 255 {
            return Err(self.bad("too many hook arguments".to_string()));
        }
        Ok(Some(args))
    }

    fn parse_hook_arg(&self, text: &str) -> PResult<HookArg> {
        if let Some(name) = text.strip_prefix('~') {
            if !is_valid_name(name) {
                return Err(self.bad(format!("bad call target '~{name}'")));
            }
            let id = self
                .symbols
                .resolve_call(name)
                .ok_or_else(|| self.bad(format!("unknown script '~{name}'")))?;
            return Ok(HookArg::Int(id));
        }
        if text.starts_with('"') {
            return Ok(HookArg::Str(self.parse_string(text)?));
        }
        Ok(HookArg::Int(self.parse_i32(text, "hook argument")?))
    }

    fn parse_int_list(&self, text: &str, what: &str) -> PResult<Vec<i32>> {
        let mut out = Vec::new();
        for item in self.parse_list(text, what)? {
            out.push(self.parse_i32(item, what)?);
        }
        if out.len() > 255 {
            return Err(self.bad(format!("too many {what} entries")));
        }
        Ok(out)
    }

    fn parse_intparams(&self, text: &str) -> PResult<Vec<(u32, i32)>> {
        let mut out = Vec::new();
        for item in self.parse_list(text, "intparams")? {
            let (key_text, value_text) = self.parse_tuple(item, "int param")?;
            let key = self.parse_u32(key_text, "int param key")?;
            if key > 0xFF_FFFF {
                return Err(self.bad(format!(
                    "int param key '{key_text}' does not fit in 3 bytes"
                )));
            }
            out.push((key, self.parse_i32(value_text, "int param value")?));
        }
        if out.len() > 255 {
            return Err(self.bad("too many int params".to_string()));
        }
        Ok(out)
    }

    fn parse_strparams(&self, text: &str) -> PResult<Vec<(u32, String)>> {
        let mut out = Vec::new();
        for item in self.parse_list(text, "strparams")? {
            let (key_text, value_text) = self.parse_tuple(item, "string param")?;
            let key = self.parse_u32(key_text, "string param key")?;
            if key > 0xFF_FFFF {
                return Err(self.bad(format!(
                    "string param key '{key_text}' does not fit in 3 bytes"
                )));
            }
            out.push((key, self.parse_string(value_text)?));
        }
        if out.len() > 255 {
            return Err(self.bad("too many string params".to_string()));
        }
        Ok(out)
    }
}

/// Split a statement into its keyword and remainder on the first whitespace run.
fn split_key(code: &str) -> (&str, &str) {
    let mut parts = code.splitn(2, char::is_whitespace);
    let key = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    (key, rest)
}

/// Split on top-level commas, tracking `()`/`[]` depth plus quotes and escapes (the
/// `split_call_args` discipline from `parse.rs`, extended to bracket depth so tuples
/// nest inside lists).
fn split_top_level(text: &str) -> std::result::Result<Vec<&str>, String> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0_usize;
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut position = 0_usize;
    while position < bytes.len() {
        let byte = bytes[position];
        if in_string {
            if byte == b'\\' {
                position += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            position += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' | b'[' => depth += 1,
            b')' | b']' if depth > 0 => depth -= 1,
            b')' | b']' => return Err(format!("unbalanced bracket in '{text}'")),
            b',' if depth == 0 => {
                parts.push(text[start..position].trim());
                start = position + 1;
            }
            _ => {}
        }
        position += 1;
    }
    if in_string {
        return Err(format!("unterminated string in '{text}'"));
    }
    parts.push(text[start..].trim());
    Ok(parts)
}

/// Cut a `//` comment, ignoring `//` inside string literals (the `parse.rs` discipline).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut position = 0;
    while position < bytes.len() {
        let byte = bytes[position];
        if in_string {
            if byte == b'\\' {
                position += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            position += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            position += 1;
            continue;
        }
        if byte == b'/' && bytes.get(position + 1) == Some(&b'/') {
            return &line[..position];
        }
        position += 1;
    }
    line
}
