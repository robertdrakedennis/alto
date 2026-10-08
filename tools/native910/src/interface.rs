//! 910 interface component codec: binary ⇄ [`InterfaceComponent`].
//!
//! A faithful port of the 910 client's component decoder: a linear,
//! version-byte-led decoder with per-type bodies. Accepted versions are exactly what the 910
//! corpus carries (`-1`, `3`, `4`, `5`); accepted types are the six primitives
//! (`0` layer, `3` rectangle, `4` text, `5` graphic, `6` model, `9` line).
//! Anything else is invalid data — 910 has no composite widgets (those are a
//! newer-revision shape the port layer downcodes, never native input).
//!
//! Deliberate model choices (all byte-exact, all gate-proven):
//!
//! * `layer` is stored RESOLVED (`raw + (parentlayer & 0xFFFF0000)`, `-1` for
//!   none); encode inverts with the interface id. The gate holds regardless,
//!   since the same parentlayer round-trips.
//! * The op-name section is stored as raw block pairs plus the raw high
//!   nibble — never interpreted — so any nibble value round-trips.
//! * Keybind entries are stored as raw head bytes for the same reason: the
//!   12-bit delay value spans two bytes with a non-unique split.
//! * Hook argument tags other than `0` (int) / `1` (string) are rejected. The
//!   client silently stores null for those; re-encoding null is impossible,
//!   so accepting them would break the round-trip.
//! * `modelkind`, `hasKeybinds`, `hashook`, and `defaultActive` are
//!   decode-only runtime state and are not modeled.
//!
//! NOTE on `contenttype`: newer-revision layouts carry one, but the 910
//! client decoder reads none — there is no such field here.

use crate::error::{NativeError, Result};
use crate::packet::{ByteWriter, Packet};

/// Whether the key mask carries a target section (drag-target cursors): the
/// target bits are `mask >> 11 & 0x7F`.
fn target_mask_present(mask: u32) -> bool {
    (mask >> 11) & 0x7F != 0
}

/// One decoded 910 interface component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceComponent {
    /// Wire version (`-1` when the byte was 255).
    pub version: i32,
    /// Component type id (low 7 bits; one of 0, 3, 4, 5, 6, 9).
    pub type_id: u8,
    /// Author name (only when the `0x80` type bit was set; absent in 910).
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
    /// Size/position modes (signed).
    pub width_mode: i8,
    /// Size/position modes (signed).
    pub height_mode: i8,
    /// Size/position modes (signed).
    pub x_mode: i8,
    /// Size/position modes (signed).
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
    /// Raw keybind mask (`g3`).
    pub keymask: u32,
    /// Raw keybind entries, in decode order.
    pub keybinds: Vec<Keybind>,
    /// Base right-click op label.
    pub opbase: String,
    /// Right-click op labels, in slot order.
    pub ops: Vec<String>,
    /// Raw op-name high nibble (block count as carried, 0..=2 in corpus).
    pub opname_nibble: u8,
    /// First op-name block (`index`, `cursor`), if carried.
    pub opname_first: Option<(u8, u16)>,
    /// Second op-name block, if carried.
    pub opname_second: Option<(u8, u16)>,
    /// Pause text (`None` iff the wire string was empty).
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
    pub target: Option<TargetSection>,
    /// Mouseover cursor (`-1` = none; carried iff version ≥ 0).
    pub mouseovercursor: i32,
    /// Int params (`key → value`), in decode order.
    pub int_params: Vec<(u32, i32)>,
    /// String params (`key → value`), in decode order.
    pub str_params: Vec<(u32, String)>,
    /// Script hooks, in client order.
    pub hooks: Hooks,
    /// Transmit lists, in client order.
    pub transmits: Transmits,
}

/// Per-type component body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentBody {
    /// Type 0: layer.
    Layer {
        /// Scroll dimensions.
        scroll_width: u16,
        /// Scroll dimensions.
        scroll_height: u16,
    },
    /// Type 3: rectangle.
    Rectangle {
        /// Fill colour.
        colour: i32,
        /// Filled.
        fill: bool,
        /// Transparency.
        trans: u8,
    },
    /// Type 4: text.
    Text {
        /// Font metrics id.
        font: i32,
        /// Monospaced (carried iff version ≥ 2).
        mono: bool,
        /// Static text.
        text: String,
        /// Line height.
        line_height: u8,
        /// Horizontal alignment.
        halign: u8,
        /// Vertical alignment.
        valign: u8,
        /// Drop shadow.
        shadow: bool,
        /// Text colour.
        colour: i32,
        /// Transparency.
        trans: u8,
        /// Max lines (carried iff version ≥ 0).
        maxlines: u8,
    },
    /// Type 5: graphic.
    Graphic {
        /// Sprite id.
        graphic: i32,
        /// 2D angle.
        angle: u16,
        /// Tiled.
        tiling: bool,
        /// Alpha channel.
        alpha: bool,
        /// Transparency.
        trans: u8,
        /// Outline width.
        outline: u8,
        /// Drop-shadow colour.
        shadow: i32,
        /// Vertical flip.
        vflip: bool,
        /// Horizontal flip.
        hflip: bool,
        /// Tint colour.
        colour: i32,
        /// Click mask (carried iff version ≥ 3).
        clickmask: bool,
    },
    /// Type 6: model.
    Model {
        /// Model id.
        id: i32,
        /// Simple transform block present.
        origin: bool,
        /// Extended transform block present.
        extended: bool,
        /// Orthographic projection.
        orthog: bool,
        /// Depth test disabled.
        nodepth: bool,
        /// Origins (`oz` meaningful only when extended).
        ox: i16,
        /// Origins (`oz` meaningful only when extended).
        oy: i16,
        /// Origins (`oz` meaningful only when extended).
        oz: i16,
        /// Angles.
        ax: u16,
        /// Angles.
        ay: u16,
        /// Angles.
        az: u16,
        /// Zoom (`u16` in the simple block, `i16` in the extended block).
        zoom: i32,
        /// Animation id.
        anim: i32,
        /// Object width (present iff width mode ≠ 0).
        objwidth: Option<u16>,
        /// Object height (present iff height mode ≠ 0).
        objheight: Option<u16>,
    },
    /// Type 9: line.
    Line {
        /// Line width.
        width: u8,
        /// Line colour.
        colour: i32,
        /// Direction.
        direction: bool,
    },
}

/// One raw keybind entry: the head byte (high nibble = slot + 1), the low
/// byte completing the 12-bit delay value, and the key/modifier bytes.
/// Stored raw because the delay value's byte split is non-unique.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Keybind {
    /// Head byte (nonzero; terminates the loop on zero).
    pub head: u8,
    /// Low byte of the delay value.
    pub lo: u8,
    /// Key byte.
    pub key: i8,
    /// Modifier byte.
    pub mods: i8,
}

/// Drag-target cursor section (`-1` = none throughout).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetSection {
    /// Second key-properties constructor argument (role TBD).
    pub param: i32,
    /// Target cursor.
    pub cursor: i32,
    /// Default target cursor.
    pub default_cursor: i32,
}

/// One hook argument: an int or a string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookArg {
    /// Int argument.
    Int(i32),
    /// String argument.
    Str(String),
}

/// An optional hook program (`None` iff the wire count was zero).
pub type Hook = Option<Vec<HookArg>>;
/// An optional transmit list (`None` iff the wire count was zero).
pub type TransmitList = Option<Vec<i32>>;

/// The 21 script hooks in client decode order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Hooks {
    /// On-load program.
    pub onload: Hook,
    /// Mouse-over program.
    pub onmouseover: Hook,
    /// Mouse-leave program.
    pub onmouseleave: Hook,
    /// Target-leave program.
    pub ontargetleave: Hook,
    /// Target-enter program.
    pub ontargetenter: Hook,
    /// Var-transmit program.
    pub onvartransmit: Hook,
    /// Inventory-transmit program.
    pub oninvtransmit: Hook,
    /// Stat-transmit program.
    pub onstattransmit: Hook,
    /// Timer program.
    pub ontimer: Hook,
    /// Op program.
    pub onop: Hook,
    /// Opt program (carried iff version ≥ 0).
    pub onopt: Hook,
    /// Mouse-repeat program.
    pub onmouserepeat: Hook,
    /// Click program.
    pub onclick: Hook,
    /// Click-repeat program.
    pub onclickrepeat: Hook,
    /// Release program.
    pub onrelease: Hook,
    /// Hold program.
    pub onhold: Hook,
    /// Drag program.
    pub ondrag: Hook,
    /// Drag-complete program.
    pub ondragcomplete: Hook,
    /// Scroll-wheel program.
    pub onscrollwheel: Hook,
    /// Client-var-transmit program.
    pub onvarctransmit: Hook,
    /// Client-var-string-transmit program.
    pub onvarcstrtransmit: Hook,
}

/// The 5 transmit lists in client decode order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Transmits {
    /// Var-transmit list.
    pub var: TransmitList,
    /// Inventory-transmit list.
    pub inv: TransmitList,
    /// Stat-transmit list.
    pub stat: TransmitList,
    /// Client-var-transmit list.
    pub varc: TransmitList,
    /// Client-var-string-transmit list.
    pub varcstr: TransmitList,
}

fn invalid(what: &str) -> NativeError {
    NativeError::Invalid(format!("bad interface component ({what})"))
}

/// Decode one component's bytes. `parentlayer` is the interface id shifted
/// left 16 (only its high half matters); pass the group's interface id.
pub fn decode_component(data: &[u8], parentlayer: i32) -> Result<InterfaceComponent> {
    let mut packet = Packet::new(data);
    let version = match packet.g1()? {
        255 => -1,
        version => i32::from(version),
    };
    if !matches!(version, -1 | 3 | 4 | 5) {
        return Err(invalid(&format!("unsupported version {version}")));
    }
    let mut type_id = packet.g1()?;
    let name = if type_id & 0x80 != 0 {
        type_id &= 0x7F;
        Some(packet.gjstr()?)
    } else {
        None
    };

    let clientcode = packet.g2()?;
    let x = packet.g2s()?;
    let y = packet.g2s()?;
    let width = packet.g2()?;
    let height = packet.g2()?;
    let width_mode = packet.g1b()?;
    let height_mode = packet.g1b()?;
    let x_mode = packet.g1b()?;
    let y_mode = packet.g1b()?;
    let aspect = if width_mode == 4 || height_mode == 4 {
        Some((packet.g2()?, packet.g2()?))
    } else {
        None
    };
    let layer_raw = packet.g2()?;
    let layer = if layer_raw == 65535 {
        -1
    } else {
        i32::from(layer_raw)
            .checked_add(parentlayer & -65_536)
            .ok_or_else(|| invalid("layer overflow"))?
    };
    let flags = packet.g1()?;
    let hide = flags & 0x1 != 0;
    let mut noclickthrough = version >= 0 && flags & 0x2 != 0;

    let body = match type_id {
        0 => {
            let scroll_width = packet.g2()?;
            let scroll_height = packet.g2()?;
            if version < 0 {
                noclickthrough = packet.g1()? == 1;
            }
            ComponentBody::Layer {
                scroll_width,
                scroll_height,
            }
        }
        5 => {
            let graphic = packet.g4s()?;
            let angle = packet.g2()?;
            let bits = packet.g1()?;
            let tiling = bits & 0x1 != 0;
            let alpha = bits & 0x2 != 0;
            let trans = packet.g1()?;
            let outline = packet.g1()?;
            let shadow = packet.g4s()?;
            let vflip = packet.g1()? == 1;
            let hflip = packet.g1()? == 1;
            let colour = packet.g4s()?;
            // Absent below version 3, the field keeps its declared default
            // `clickmask = true`.
            let clickmask = if version >= 3 {
                packet.g1()? == 1
            } else {
                true
            };
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
            }
        }
        6 => {
            let id = packet.gsmart2or4null()?;
            let bits = packet.g1()?;
            let origin = bits & 0x1 != 0;
            let extended = bits & 0x2 != 0;
            let orthog = bits & 0x4 != 0;
            let nodepth = bits & 0x8 != 0;
            let (ox, oy, oz, ax, ay, az, zoom) = if origin {
                (
                    packet.g2s()?,
                    packet.g2s()?,
                    0,
                    packet.g2()?,
                    packet.g2()?,
                    packet.g2()?,
                    i32::from(packet.g2()?),
                )
            } else if extended {
                (
                    packet.g2s()?,
                    packet.g2s()?,
                    packet.g2s()?,
                    packet.g2()?,
                    packet.g2()?,
                    packet.g2()?,
                    i32::from(packet.g2s()?),
                )
            } else {
                (0, 0, 0, 0, 0, 0, 0)
            };
            let anim = packet.gsmart2or4null()?;
            let objwidth = if width_mode != 0 {
                Some(packet.g2()?)
            } else {
                None
            };
            let objheight = if height_mode != 0 {
                Some(packet.g2()?)
            } else {
                None
            };
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
            }
        }
        4 => {
            let font = packet.gsmart2or4null()?;
            // Absent below version 2, the field keeps its declared default
            // `fontmono = true`.
            let mono = if version >= 2 {
                packet.g1()? == 1
            } else {
                true
            };
            let text = packet.gjstr()?;
            let line_height = packet.g1()?;
            let halign = packet.g1()?;
            let valign = packet.g1()?;
            let shadow = packet.g1()? == 1;
            let colour = packet.g4s()?;
            let trans = packet.g1()?;
            let maxlines = if version >= 0 { packet.g1()? } else { 0 };
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
            }
        }
        3 => ComponentBody::Rectangle {
            colour: packet.g4s()?,
            fill: packet.g1()? == 1,
            trans: packet.g1()?,
        },
        9 => ComponentBody::Line {
            width: packet.g1()?,
            colour: packet.g4s()?,
            direction: packet.g1()? == 1,
        },
        other => return Err(invalid(&format!("unsupported type {other}"))),
    };

    let keymask = packet.g3()?;
    // The first head byte doubles as the section gate: zero breaks out
    // immediately, so an empty section costs exactly one byte. (A separate
    // gate read plus a head read over-consumes by one and shifts every
    // component WITH keybinds into garbage that still round-trips — the gate
    // cannot catch that; only content checks like real op text can.)
    let mut keybinds = Vec::new();
    loop {
        let head = packet.g1()?;
        if head == 0 {
            break;
        }
        keybinds.push(Keybind {
            head,
            lo: packet.g1()?,
            key: packet.g1b()?,
            mods: packet.g1b()?,
        });
    }

    let opbase = packet.gjstr()?;
    let counts = packet.g1()?;
    let nops = counts & 0xF;
    let nopnames = counts >> 4;
    let mut ops = Vec::with_capacity(usize::from(nops));
    for _ in 0..nops {
        ops.push(packet.gjstr()?);
    }
    let opname_first = if nopnames > 0 {
        Some((packet.g1()?, packet.g2()?))
    } else {
        None
    };
    let opname_second = if nopnames > 1 {
        Some((packet.g1()?, packet.g2()?))
    } else {
        None
    };

    let pausetext = match packet.gjstr()?.as_str() {
        "" => None,
        text => Some(text.to_string()),
    };
    let dragdeadzone = packet.g1()?;
    let dragdeadtime = packet.g1()?;
    let dragrenderbehaviour = packet.g1()?;
    let targetverb = packet.gjstr()?;
    let target = if target_mask_present(keymask) {
        let param = read_opt_id(packet.g2()?);
        let cursor = read_opt_id(packet.g2()?);
        let default_cursor = read_opt_id(packet.g2()?);
        Some(TargetSection {
            param,
            cursor,
            default_cursor,
        })
    } else {
        None
    };
    let mouseovercursor = if version >= 0 {
        read_opt_id(packet.g2()?)
    } else {
        -1
    };

    let mut int_params = Vec::new();
    let mut str_params = Vec::new();
    if version >= 0 {
        let int_count = packet.g1()?;
        for _ in 0..int_count {
            int_params.push((packet.g3()?, packet.g4s()?));
        }
        let str_count = packet.g1()?;
        for _ in 0..str_count {
            str_params.push((packet.g3()?, packet.gjstr2()?));
        }
    }

    let hooks = Hooks {
        onload: decode_hook(&mut packet)?,
        onmouseover: decode_hook(&mut packet)?,
        onmouseleave: decode_hook(&mut packet)?,
        ontargetleave: decode_hook(&mut packet)?,
        ontargetenter: decode_hook(&mut packet)?,
        onvartransmit: decode_hook(&mut packet)?,
        oninvtransmit: decode_hook(&mut packet)?,
        onstattransmit: decode_hook(&mut packet)?,
        ontimer: decode_hook(&mut packet)?,
        onop: decode_hook(&mut packet)?,
        onopt: if version >= 0 {
            decode_hook(&mut packet)?
        } else {
            None
        },
        onmouserepeat: decode_hook(&mut packet)?,
        onclick: decode_hook(&mut packet)?,
        onclickrepeat: decode_hook(&mut packet)?,
        onrelease: decode_hook(&mut packet)?,
        onhold: decode_hook(&mut packet)?,
        ondrag: decode_hook(&mut packet)?,
        ondragcomplete: decode_hook(&mut packet)?,
        onscrollwheel: decode_hook(&mut packet)?,
        onvarctransmit: decode_hook(&mut packet)?,
        onvarcstrtransmit: decode_hook(&mut packet)?,
    };

    let transmits = Transmits {
        var: decode_transmit_list(&mut packet)?,
        inv: decode_transmit_list(&mut packet)?,
        stat: decode_transmit_list(&mut packet)?,
        varc: decode_transmit_list(&mut packet)?,
        varcstr: decode_transmit_list(&mut packet)?,
    };

    if !packet.is_empty() {
        return Err(invalid(&format!(
            "trailing bytes: decode stopped at {} of {}",
            packet.pos(),
            packet.len()
        )));
    }

    Ok(InterfaceComponent {
        version,
        type_id,
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
        opname_nibble: nopnames,
        opname_first,
        opname_second,
        pausetext,
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
    })
}

/// Read a `65535`-means-none id.
fn read_opt_id(raw: u16) -> i32 {
    if raw == 65535 { -1 } else { i32::from(raw) }
}

/// Write a `-1`-means-`65535` id.
fn write_opt_id(writer: &mut ByteWriter, value: i32) -> Result<()> {
    if value == -1 {
        writer.p2(65535);
    } else {
        writer.p2(u16::try_from(value)
            .map_err(|_| NativeError::Invalid(format!("id {value} out of range for component")))?);
    }
    Ok(())
}

/// Decode one hook program: a count plus int/string-tagged arguments. Tags
/// other than 0/1 are rejected (the client silently stores null for those,
/// which cannot round-trip).
fn decode_hook(packet: &mut Packet<'_>) -> Result<Hook> {
    let count = packet.g1()?;
    if count == 0 {
        return Ok(None);
    }
    let mut args = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        match packet.g1()? {
            0 => args.push(HookArg::Int(packet.g4s()?)),
            1 => args.push(HookArg::Str(packet.gjstr()?)),
            tag => {
                return Err(invalid(&format!("unhandled hook argument type {tag}")));
            }
        }
    }
    Ok(Some(args))
}

/// Decode one transmit list: a count plus `i32` entries.
fn decode_transmit_list(packet: &mut Packet<'_>) -> Result<TransmitList> {
    let count = packet.g1()?;
    if count == 0 {
        return Ok(None);
    }
    let mut entries = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        entries.push(packet.g4s()?);
    }
    Ok(Some(entries))
}

/// Encode a component back to 910 binary. `parentlayer` inverts the decode
/// mapping and must be the same interface id shifted left 16.
pub fn encode_component(component: &InterfaceComponent, parentlayer: i32) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if component.version == -1 {
        writer.p1(255);
    } else {
        writer.p1(u8::try_from(component.version)
            .map_err(|_| invalid(&format!("version {} out of range", component.version)))?);
    }
    let mut type_byte = component.type_id;
    if let Some(name) = &component.name {
        type_byte |= 0x80;
        writer.p1(type_byte);
        writer.pjstr(name)?;
    } else {
        writer.p1(type_byte);
    }
    writer.p2(component.clientcode);
    writer.p2s(component.x);
    writer.p2s(component.y);
    writer.p2(component.width);
    writer.p2(component.height);
    writer.p1b(component.width_mode);
    writer.p1b(component.height_mode);
    writer.p1b(component.x_mode);
    writer.p1b(component.y_mode);
    match component.aspect {
        Some((width, height)) if component.width_mode == 4 || component.height_mode == 4 => {
            writer.p2(width);
            writer.p2(height);
        }
        Some(_) => {
            return Err(invalid("aspect size carried without an aspect size mode"));
        }
        None => {
            if component.width_mode == 4 || component.height_mode == 4 {
                return Err(invalid("aspect size mode without aspect size"));
            }
        }
    }
    if component.layer == -1 {
        writer.p2(65535);
    } else {
        let raw = component
            .layer
            .checked_sub(parentlayer & -65_536)
            .and_then(|raw| u16::try_from(raw).ok())
            .ok_or_else(|| invalid(&format!("layer {} out of range", component.layer)))?;
        writer.p2(raw);
    }
    let mut flags = 0_u8;
    if component.hide {
        flags |= 0x1;
    }
    if component.version >= 0 && component.noclickthrough {
        flags |= 0x2;
    }
    writer.p1(flags);

    match &component.body {
        ComponentBody::Layer {
            scroll_width,
            scroll_height,
        } => {
            require_type(component.type_id, 0)?;
            writer.p2(*scroll_width);
            writer.p2(*scroll_height);
            if component.version < 0 {
                writer.p1(u8::from(component.noclickthrough));
            }
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
            require_type(component.type_id, 5)?;
            writer.p4s(*graphic);
            writer.p2(*angle);
            writer.p1(u8::from(*tiling) | (u8::from(*alpha) << 1));
            writer.p1(*trans);
            writer.p1(*outline);
            writer.p4s(*shadow);
            writer.p1(u8::from(*vflip));
            writer.p1(u8::from(*hflip));
            writer.p4s(*colour);
            if component.version >= 3 {
                writer.p1(u8::from(*clickmask));
            } else if !*clickmask {
                return Err(invalid(
                    "clickmask false below version 3 (the default is true)",
                ));
            }
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
            require_type(component.type_id, 6)?;
            writer.pdata(&smart2or4s(*id)?);
            let mut bits = 0_u8;
            if *origin {
                bits |= 0x1;
            }
            if *extended {
                bits |= 0x2;
            }
            if *orthog {
                bits |= 0x4;
            }
            if *nodepth {
                bits |= 0x8;
            }
            writer.p1(bits);
            if *origin {
                writer.p2s(*ox);
                writer.p2s(*oy);
                writer.p2(*ax);
                writer.p2(*ay);
                writer.p2(*az);
                writer.p2(u16::try_from(*zoom)
                    .map_err(|_| invalid(&format!("model zoom {zoom} out of range")))?);
            } else if *extended {
                writer.p2s(*ox);
                writer.p2s(*oy);
                writer.p2s(*oz);
                writer.p2(*ax);
                writer.p2(*ay);
                writer.p2(*az);
                writer.p2s(
                    i16::try_from(*zoom)
                        .map_err(|_| invalid(&format!("model zoom {zoom} out of range")))?,
                );
            }
            writer.pdata(&smart2or4s(*anim)?);
            match (objwidth, component.width_mode != 0) {
                (Some(value), true) => writer.p2(*value),
                (None, false) => {}
                _ => {
                    return Err(invalid("model object width disagrees with width mode"));
                }
            }
            match (objheight, component.height_mode != 0) {
                (Some(value), true) => writer.p2(*value),
                (None, false) => {}
                _ => {
                    return Err(invalid("model object height disagrees with height mode"));
                }
            }
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
            require_type(component.type_id, 4)?;
            writer.pdata(&smart2or4s(*font)?);
            if component.version >= 2 {
                writer.p1(u8::from(*mono));
            } else if !*mono {
                return Err(invalid(
                    "fontmono false below version 2 (the default is true)",
                ));
            }
            writer.pjstr(text)?;
            writer.p1(*line_height);
            writer.p1(*halign);
            writer.p1(*valign);
            writer.p1(u8::from(*shadow));
            writer.p4s(*colour);
            writer.p1(*trans);
            if component.version >= 0 {
                writer.p1(*maxlines);
            } else if *maxlines != 0 {
                return Err(invalid("maxlines carried below version 0"));
            }
        }
        ComponentBody::Rectangle {
            colour,
            fill,
            trans,
        } => {
            require_type(component.type_id, 3)?;
            writer.p4s(*colour);
            writer.p1(u8::from(*fill));
            writer.p1(*trans);
        }
        ComponentBody::Line {
            width,
            colour,
            direction,
        } => {
            require_type(component.type_id, 9)?;
            writer.p1(*width);
            writer.p4s(*colour);
            writer.p1(u8::from(*direction));
        }
    }

    writer.p3(component.keymask)?;
    for entry in &component.keybinds {
        if entry.head == 0 {
            return Err(invalid("keybind head byte must be nonzero"));
        }
        writer.p1(entry.head);
        writer.p1(entry.lo);
        writer.p1b(entry.key);
        writer.p1b(entry.mods);
    }
    writer.p1(0);

    writer.pjstr(&component.opbase)?;
    if component.ops.len() > 15 || component.opname_nibble > 15 {
        return Err(invalid("op counts out of nibble range"));
    }
    let counts = (component.opname_nibble << 4) | component.ops.len() as u8;
    writer.p1(counts);
    for op in &component.ops {
        writer.pjstr(op)?;
    }
    match (component.opname_nibble, component.opname_first) {
        (0, None) => {}
        (nibble, Some((index, value))) if nibble > 0 => {
            writer.p1(index);
            writer.p2(value);
        }
        _ => {
            return Err(invalid("op-name first block disagrees with nibble"));
        }
    }
    match (
        component.opname_nibble,
        component.opname_first,
        component.opname_second,
    ) {
        (_, _, None) if component.opname_nibble <= 1 => {}
        (nibble, Some(_), Some((index, value))) if nibble > 1 => {
            writer.p1(index);
            writer.p2(value);
        }
        _ => {
            return Err(invalid("op-name second block disagrees with nibble"));
        }
    }

    writer.pjstr(component.pausetext.as_deref().unwrap_or(""))?;
    writer.p1(component.dragdeadzone);
    writer.p1(component.dragdeadtime);
    writer.p1(component.dragrenderbehaviour);
    writer.pjstr(&component.targetverb)?;
    match (&component.target, target_mask_present(component.keymask)) {
        (Some(target), true) => {
            write_opt_id(&mut writer, target.param)?;
            write_opt_id(&mut writer, target.cursor)?;
            write_opt_id(&mut writer, target.default_cursor)?;
        }
        (None, false) => {}
        _ => {
            return Err(invalid("target section disagrees with key mask"));
        }
    }
    if component.version >= 0 {
        write_opt_id(&mut writer, component.mouseovercursor)?;
        writer
            .p1(u8::try_from(component.int_params.len())
                .map_err(|_| invalid("too many int params"))?);
        for (key, value) in &component.int_params {
            writer.p3(*key)?;
            writer.p4s(*value);
        }
        writer.p1(u8::try_from(component.str_params.len())
            .map_err(|_| invalid("too many string params"))?);
        for (key, value) in &component.str_params {
            writer.p3(*key)?;
            writer.pjstr2(value)?;
        }
    } else {
        if !component.int_params.is_empty() || !component.str_params.is_empty() {
            return Err(invalid("params carried below version 0"));
        }
        if component.mouseovercursor != -1 {
            return Err(invalid("mouseover cursor carried below version 0"));
        }
    }

    encode_hook(&mut writer, &component.hooks.onload)?;
    encode_hook(&mut writer, &component.hooks.onmouseover)?;
    encode_hook(&mut writer, &component.hooks.onmouseleave)?;
    encode_hook(&mut writer, &component.hooks.ontargetleave)?;
    encode_hook(&mut writer, &component.hooks.ontargetenter)?;
    encode_hook(&mut writer, &component.hooks.onvartransmit)?;
    encode_hook(&mut writer, &component.hooks.oninvtransmit)?;
    encode_hook(&mut writer, &component.hooks.onstattransmit)?;
    encode_hook(&mut writer, &component.hooks.ontimer)?;
    encode_hook(&mut writer, &component.hooks.onop)?;
    if component.version >= 0 {
        encode_hook(&mut writer, &component.hooks.onopt)?;
    } else if component.hooks.onopt.is_some() {
        return Err(invalid("onopt hook carried below version 0"));
    }
    encode_hook(&mut writer, &component.hooks.onmouserepeat)?;
    encode_hook(&mut writer, &component.hooks.onclick)?;
    encode_hook(&mut writer, &component.hooks.onclickrepeat)?;
    encode_hook(&mut writer, &component.hooks.onrelease)?;
    encode_hook(&mut writer, &component.hooks.onhold)?;
    encode_hook(&mut writer, &component.hooks.ondrag)?;
    encode_hook(&mut writer, &component.hooks.ondragcomplete)?;
    encode_hook(&mut writer, &component.hooks.onscrollwheel)?;
    encode_hook(&mut writer, &component.hooks.onvarctransmit)?;
    encode_hook(&mut writer, &component.hooks.onvarcstrtransmit)?;

    encode_transmit_list(&mut writer, &component.transmits.var)?;
    encode_transmit_list(&mut writer, &component.transmits.inv)?;
    encode_transmit_list(&mut writer, &component.transmits.stat)?;
    encode_transmit_list(&mut writer, &component.transmits.varc)?;
    encode_transmit_list(&mut writer, &component.transmits.varcstr)?;

    Ok(writer.data)
}

/// Encode one signed smart int (inverse of the client's `gSmart2or4s`).
/// Only `-1` and non-negative values round-trip: the 4-byte path masks to 31
/// bits on decode, so any other negative value is unrepresentable and rejected
/// rather than silently rewritten.
fn smart2or4s(value: i32) -> Result<Vec<u8>> {
    if value == -1 {
        // Decodes via the 2-byte path (`32767` reads as `-1`).
        Ok(vec![0x7F, 0xFF])
    } else if value < 0 {
        Err(invalid(&format!(
            "negative smart value {value} cannot round-trip"
        )))
    } else if value < 32767 {
        Ok((value as u16).to_be_bytes().to_vec())
    } else {
        // High bit set so decode takes the 4-byte path and masks back.
        Ok(((value as u32) | 0x8000_0000).to_be_bytes().to_vec())
    }
}

fn require_type(actual: u8, expected: u8) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(invalid(&format!(
            "body is for type {expected}, component says {actual}"
        )))
    }
}

fn encode_hook(writer: &mut ByteWriter, hook: &Hook) -> Result<()> {
    match hook {
        None => writer.p1(0),
        Some(args) => {
            writer.p1(u8::try_from(args.len()).map_err(|_| invalid("too many hook arguments"))?);
            for arg in args {
                match arg {
                    HookArg::Int(value) => {
                        writer.p1(0);
                        writer.p4s(*value);
                    }
                    HookArg::Str(text) => {
                        writer.p1(1);
                        writer.pjstr(text)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn encode_transmit_list(writer: &mut ByteWriter, list: &TransmitList) -> Result<()> {
    match list {
        None => writer.p1(0),
        Some(entries) => {
            writer.p1(u8::try_from(entries.len()).map_err(|_| invalid("transmit list too long"))?);
            for entry in entries {
                writer.p4s(*entry);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_component() -> InterfaceComponent {
        InterfaceComponent {
            version: 4,
            type_id: 0,
            name: None,
            clientcode: 0,
            x: -1,
            y: 2,
            width: 100,
            height: 200,
            width_mode: 0,
            height_mode: 0,
            x_mode: 0,
            y_mode: 0,
            aspect: None,
            layer: -1,
            hide: true,
            noclickthrough: true,
            body: ComponentBody::Layer {
                scroll_width: 300,
                scroll_height: 400,
            },
            keymask: 0,
            keybinds: Vec::new(),
            opbase: String::new(),
            ops: vec!["Op one".to_string()],
            opname_nibble: 1,
            opname_first: Some((0, 7)),
            opname_second: None,
            pausetext: None,
            dragdeadzone: 0,
            dragdeadtime: 0,
            dragrenderbehaviour: 0,
            targetverb: String::new(),
            target: None,
            mouseovercursor: -1,
            int_params: vec![(9, -3)],
            str_params: vec![(10, "hi".to_string())],
            hooks: Hooks {
                onclick: Some(vec![HookArg::Int(5690), HookArg::Str("x".to_string())]),
                ..Hooks::default()
            },
            transmits: Transmits {
                var: Some(vec![1752]),
                ..Transmits::default()
            },
        }
    }

    #[test]
    fn layer_roundtrip_is_byte_identical() {
        let component = layer_component();
        let bytes = encode_component(&component, 0).unwrap();
        let decoded = decode_component(&bytes, 0).unwrap();
        assert_eq!(decoded, component);
        assert_eq!(encode_component(&decoded, 0).unwrap(), bytes);
    }

    #[test]
    fn layer_inverts_against_parentlayer() {
        // Raw 7 under parentlayer 0x00020000 resolves to 0x20007 and back;
        // the same model is unencodable under parentlayer 0 (raw overflows).
        let mut component = layer_component();
        component.layer = 0x0002_0007;
        let bytes = encode_component(&component, 0x0002_0000).unwrap();
        let decoded = decode_component(&bytes, 0x0002_0000).unwrap();
        assert_eq!(decoded.layer, 0x0002_0007);
        assert_eq!(encode_component(&decoded, 0x0002_0000).unwrap(), bytes);
        assert!(encode_component(&decoded, 0).is_err());
    }

    #[test]
    fn model_validation_rejects_mismatches() {
        let mut component = layer_component();
        component.body = ComponentBody::Rectangle {
            colour: 0,
            fill: false,
            trans: 0,
        };
        assert!(encode_component(&component, 0).is_err());
        component.body = ComponentBody::Layer {
            scroll_width: 0,
            scroll_height: 0,
        };
        component.version = 99;
        assert!(decode_component(&encode_component(&component, 0).unwrap(), 0).is_err());
    }

    #[test]
    fn hook_rejects_unknown_arg_tags() {
        let mut bytes = encode_component(&layer_component(), 0).unwrap();
        // Flip the onclick hook's first arg tag from 0 (int) to 2: the last
        // [count, tag] pair before the trailing all-zero hooks.
        let position = bytes
            .windows(2)
            .rposition(|window| window == [2, 0])
            .expect("hook arg count prefix");
        bytes[position + 1] = 2;
        assert!(decode_component(&bytes, 0).is_err());
    }

    #[test]
    fn smart_roundtrip_covers_edges() {
        for value in [0, 1, 32766, 32767, 100_000, i32::MAX] {
            let bytes = smart2or4s(value).unwrap();
            let mut packet = Packet::new(&bytes);
            assert_eq!(packet.gsmart2or4null().unwrap(), value, "value {value}");
        }
        assert!(smart2or4s(-1).is_ok());
        // Negatives other than -1 cannot round-trip (the 4-byte path masks).
        assert!(smart2or4s(-2).is_err());
        assert!(smart2or4s(i32::MIN).is_err());
    }
}
