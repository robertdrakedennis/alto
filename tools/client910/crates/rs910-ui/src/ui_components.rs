//! Component references and child-array aliases are retained, including detached
//! children still referenced by scripts or a separately sorted drawing array.
use crate::ui_component_fields::Fields;
pub use crate::utf16_text::Text;
use native910::interface::{self as wire, ComponentBody};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
};
pub type Ref = Rc<RefCell<Component>>;
pub type Array = Rc<RefCell<Vec<Option<Ref>>>>;
pub type InterfaceRef = Rc<RefCell<Interface>>;

/// Declares a `Copy` struct of named scalar [`Fields`] members with
/// `of(&Fields)`, which reads them all at one point, as a full
/// `c.borrow().f.clone()` did, without copying the whole struct and its
/// text vectors. Walks that call hooks or the backend between reads keep
/// the values from that point; the `RefCell` borrow ends at once.
macro_rules! field_snapshot {
    ($(#[$meta:meta])* $name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[allow(non_snake_case)]
        #[derive(Clone, Copy)]
        struct $name {
            $($field: $ty,)*
        }
        impl $name {
            fn of(f: &crate::ui_component_fields::Fields) -> Self {
                Self { $($field: f.$field,)* }
            }
        }
    };
}
pub(crate) use field_snapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Int(i32),
    Long(i64),
    String(Text),
    Null,
}
impl From<wire::HookArg> for Arg {
    fn from(a: wire::HookArg) -> Self {
        match a {
            wire::HookArg::Int(v) => Self::Int(v),
            wire::HookArg::Str(v) => Self::String(v.encode_utf16().collect()),
        }
    }
}
#[derive(Clone, Debug)]
pub struct CreationOrigin {
    pub script: Option<i32>,
    pub instruction: usize,
    pub event: Option<String>,
}

/// Encoded identity in a runtime owner's namespace. The all-ones identity is
/// reserved for cache-defined components.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RuntimeChildId(u16);
impl RuntimeChildId {
    pub fn new(encoded: u16) -> anyhow::Result<Self> {
        anyhow::ensure!(encoded != u16::MAX, "reserved static component identity");
        Ok(Self(encoded))
    }
    pub fn encoded(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeLink {
    pub parent: Weak<RefCell<Component>>,
    pub owner: Weak<RefCell<Component>>,
}

#[derive(Clone, Debug)]
pub struct Component {
    /// Authoring identity recorded after a successful runtime child creation.
    /// It observes the owning instruction without changing lookup or hook state.
    pub creation_origin: Option<CreationOrigin>,
    pub(crate) runtime_link: Option<RuntimeLink>,
    pub(crate) runtime_lookup: Option<BTreeMap<RuntimeChildId, Ref>>,
    /// Visibility belongs to the parent's entries, including same-id replacements.
    pub(crate) runtime_hidden: BTreeSet<RuntimeChildId>,
    /// Fractional layout constraints are consumed by ordinary layout traversal.
    pub size_constraint: Option<crate::ui_layout_constraints::SizeConstraint>,
    pub position_constraint: Option<crate::ui_layout_constraints::PositionConstraint>,
    pub viewport_layout: Option<crate::ui_layout_constraints::ViewportInsets>,
    pub layout_padding: Option<crate::ui_layout_constraints::LayoutPadding>,
    pub f: Fields,
    pub(crate) font_projection: FontIdentity,
    /// The model animation playback; owned by the model animation service.
    pub model_animator: Option<crate::animation_assets::Playback>,
    pub default_active: [i32; 2],
    pub ops: Option<Vec<Option<Text>>>,
    pub opname: Option<Vec<i32>>,
    pub keys: Option<Vec<Option<Vec<i8>>>>,
    pub key_mods: Option<Vec<Option<Vec<i8>>>>,
    pub key_delays: Option<Vec<i32>>,
    pub key_rates: Option<Vec<i32>>,
    pub key_chars: Option<Vec<i32>>,
    pub key_next_fire: Option<Vec<i32>>,
    pub hooks: BTreeMap<&'static str, Vec<Arg>>,
    pub transmits: BTreeMap<&'static str, Vec<i32>>,
    /// Imported retained hooks use unsigned cursors and empty-list filtering.
    pub retained_player_transmit: Option<native910::execution::VariableEventTokens>,
    /// HashTable permits duplicate IDs; lookup/update uses the first entry.
    pub params: Option<Vec<(i64, Arg)>>,
    pub children: Option<Array>,
    pub sorted: Option<Array>,
    pub draggable: Option<Ref>,
    pub recolour: Option<([i16; 5], [i16; 5])>,
    pub retexture: Option<([i16; 5], [i16; 5])>,
    /// Group kind: serial id used by linked social actions.
    pub group_kind: Option<u8>,
    /// The NPC model override written by `if_npc_setcustom*`.
    pub npc_customisation: Option<crate::ui_models::NpcCustomisation>,
    /// Identity of this component object for its particle system: a new
    /// component holds no system.
    pub particle_serial: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FontIdentity {
    #[default]
    Initial,
    Source(FontProjection),
    TargetWrite,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FontProjection {
    domain: native910::execution::ContentDomain,
    mapping: native910::execution::FontMapping,
}

/// Construction counter behind [`Component::particle_serial`].
static PARTICLE_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
impl Default for Component {
    fn default() -> Self {
        Self {
            creation_origin: None,
            runtime_link: None,
            runtime_lookup: None,
            runtime_hidden: BTreeSet::new(),
            size_constraint: None,
            position_constraint: None,
            viewport_layout: None,
            layout_padding: None,
            f: Fields::default(),
            font_projection: FontIdentity::Initial,
            model_animator: None,
            default_active: [0, -1],
            ops: None,
            opname: None,
            keys: None,
            key_mods: None,
            key_delays: None,
            key_rates: None,
            key_chars: None,
            key_next_fire: None,
            hooks: BTreeMap::new(),
            transmits: BTreeMap::new(),
            retained_player_transmit: None,
            params: None,
            children: None,
            sorted: None,
            draggable: None,
            recolour: None,
            retexture: None,
            group_kind: None,
            npc_customisation: None,
            particle_serial: PARTICLE_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}
impl Component {
    /// A target write cannot establish a donor identity, including equal writes.
    pub(crate) fn set_target_font(&mut self, font: i32) {
        self.font_projection = FontIdentity::TargetWrite;
        self.f.textfont = font;
    }
    pub(crate) fn set_source_font(
        &mut self,
        domain: native910::execution::ContentDomain,
        mapping: native910::execution::FontMapping,
    ) {
        self.set_target_font(mapping.target);
        self.font_projection = FontIdentity::Source(FontProjection { domain, mapping });
    }
    pub(crate) fn source_font(
        &mut self,
        domain: native910::execution::ContentDomain,
        absent: i32,
        initial: Option<native910::execution::InitialFontMapping>,
    ) -> anyhow::Result<i32> {
        if let FontIdentity::Source(projection) = self.font_projection {
            if projection.domain == domain && projection.mapping.target == self.f.textfont {
                return Ok(projection.mapping.source);
            }
        }
        if self.f.textfont == absent {
            return Ok(absent);
        }
        if let Some(initial) = initial.filter(|initial| {
            matches!(self.font_projection, FontIdentity::Initial)
                && self.f.id == -i32::from(true)
                && self.runtime_link.is_none()
                && self.f.parentlayer == initial.component
                && self.f.textfont == initial.mapping.target
        }) {
            self.font_projection = FontIdentity::Source(FontProjection {
                domain,
                mapping: initial.mapping,
            });
            return Ok(initial.mapping.source);
        }
        anyhow::bail!("font identity is unbound or replaced by a target write; apply a named donor font binding");
    }
    /// Empty retained drawing storage falls back to the ordered children.
    pub(crate) fn child_drawing_order(&self) -> Option<Array> {
        self.sorted
            .as_ref()
            .filter(|order| !order.borrow().is_empty())
            .or(self.children.as_ref())
            .or(self.sorted.as_ref())
            .cloned()
    }

    /// The byte-preserving codec supplies wire values; this installs actual
    /// constructor defaults and all decoded runtime metadata it does not model.
    pub fn decode(packed: i32, bytes: &[u8]) -> anyhow::Result<Self> {
        let v = wire::decode_component(bytes, packed)?;
        // Component.decode dereferences its initially null params
        // table. The retail hf.z bytecode does the same. Every current cache
        // file has zero wire params; runtime setters allocate the table below.
        anyhow::ensure!(
            v.int_params.is_empty() && v.str_params.is_empty(),
            "the component decoder leaves the parameter table null when the file has no parameters"
        );
        let mut c = Self::default();
        let f = &mut c.f;
        f.parentlayer = packed;
        f.r#type = v.type_id as i32;
        f.name = v.name.map(|s| s.encode_utf16().collect());
        f.clientcode = v.clientcode as i32;
        f.xpos = v.x as i32;
        f.ypos = v.y as i32;
        f.wsize = v.width as i32;
        f.hsize = v.height as i32;
        f.widthSizeMode = v.width_mode;
        f.heightSizeMode = v.height_mode;
        f.xmode = v.x_mode;
        f.ymode = v.y_mode;
        if let Some((w, h)) = v.aspect {
            f.aspectwidth = w as i32;
            f.aspectheight = h as i32;
        }
        f.layer = v.layer;
        f.hide = v.hide;
        f.noclickthrough = v.noclickthrough;
        match v.body {
            ComponentBody::Layer {
                scroll_width,
                scroll_height,
            } => {
                f.scrollwidth = scroll_width as i32;
                f.scrollheight = scroll_height as i32;
            }
            ComponentBody::Rectangle {
                colour,
                fill,
                trans,
            } => {
                f.colour = colour;
                f.fill = fill;
                f.trans = trans as i32;
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
                f.textfont = font;
                if v.version >= 2 {
                    f.fontmono = mono;
                }
                f.text = Some(text.encode_utf16().collect());
                f.textLineHeight = line_height as i32;
                f.textHAlign = halign as i32;
                f.textVAlign = valign as i32;
                f.textshadow = shadow;
                f.colour = colour;
                f.trans = trans as i32;
                f.maxlines = maxlines as i32;
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
                f.graphic = graphic;
                f.angle2d = angle as i32;
                f.tiling = tiling;
                f.alpha = alpha;
                f.trans = trans as i32;
                f.outline = outline as i32;
                f.graphicshadow = shadow;
                f.vflip = vflip;
                f.hflip = hflip;
                f.colour = colour;
                if v.version >= 3 {
                    f.clickmask = clickmask;
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
                f.model = id;
                f.useExtendedModelTransform = extended;
                f.modelorthog = orthog;
                f.disableDepthTest = nodepth;
                f.modelanim = anim;
                if origin || extended {
                    f.modelorigin_x = ox as i32;
                    f.modelorigin_y = oy as i32;
                    f.modelorigin_z = oz as i32;
                    f.modelangle_x = ax as i32;
                    f.modelangle_y = ay as i32;
                    f.modelangle_z = az as i32;
                    f.modelzoom = zoom;
                }
                if let Some(v) = objwidth {
                    f.modelobjwidth = v as i32;
                }
                if let Some(v) = objheight {
                    f.modelobjheight = v as i32;
                }
            }
            ComponentBody::Line {
                width,
                colour,
                direction,
            } => {
                f.linewid = width as i32;
                f.colour = colour;
                f.linedirection = direction;
            }
        }
        c.default_active = [v.keymask as i32, v.target.map_or(-1, |t| t.param)];
        if let Some(t) = v.target {
            f.targetCursor = t.cursor;
            f.targetDefaultCursor = t.default_cursor;
        }
        f.mouseovercursor = v.mouseovercursor;
        if !v.keybinds.is_empty() {
            let mut keys = vec![None; 11];
            let mut mods = keys.clone();
            let mut delays = vec![0; 11];
            for k in v.keybinds {
                let slot = (k.head >> 4).wrapping_sub(1) as usize;
                anyhow::ensure!(slot < 11, "keybinding slot");
                let delay = ((k.head as i32) << 8 | k.lo as i32) & 0xfff;
                delays[slot] = if delay == 4095 { -1 } else { delay };
                keys[slot] = Some(vec![k.key]);
                mods[slot] = Some(vec![k.mods]);
                if k.key != 0 {
                    f.hasKeybinds = true;
                }
            }
            c.keys = Some(keys);
            c.key_mods = Some(mods);
            c.key_delays = Some(delays);
            c.key_chars = Some(vec![0; 11]);
        }
        f.opbase = Some(v.opbase.encode_utf16().collect());
        if !v.ops.is_empty() {
            c.ops = Some(
                v.ops
                    .into_iter()
                    .map(|s| Some(s.encode_utf16().collect()))
                    .collect(),
            );
        }
        if let Some((index, cursor)) = v.opname_first {
            let mut names = vec![-1; index as usize + 1];
            names[index as usize] = cursor as i32;
            if let Some((index, cursor)) = v.opname_second {
                let cell = names
                    .get_mut(index as usize)
                    .ok_or_else(|| anyhow::anyhow!("opname index"))?;
                *cell = cursor as i32;
            }
            c.opname = Some(names);
        }
        f.pausetext = v.pausetext.map(|s| s.encode_utf16().collect());
        f.dragdeadzone = v.dragdeadzone as i32;
        f.dragdeadtime = v.dragdeadtime as i32;
        f.dragrenderbehaviour = v.dragrenderbehaviour as i32;
        f.targetverb = Some(v.targetverb.encode_utf16().collect());
        macro_rules! hooks {($($name:ident),*)=>{$(if let Some(h)=v.hooks.$name {f.hashook=true;c.hooks.insert(stringify!($name),h.into_iter().map(Arg::from).collect());})*};}
        hooks!(
            onload,
            onmouseover,
            onmouseleave,
            ontargetleave,
            ontargetenter,
            onvartransmit,
            oninvtransmit,
            onstattransmit,
            ontimer,
            onop,
            onopt,
            onmouserepeat,
            onclick,
            onclickrepeat,
            onrelease,
            onhold,
            ondrag,
            ondragcomplete,
            onscrollwheel,
            onvarctransmit,
            onvarcstrtransmit
        );
        macro_rules! transmits {($($wire:ident=>$name:literal),*)=>{$(if let Some(t)=v.transmits.$wire {c.transmits.insert($name,t);})*};}
        transmits!(var=>"onvartransmitlist",inv=>"oninvtransmitlist",stat=>"onstattransmitlist",varc=>"onvarctransmitlist",varcstr=>"onvarcstrtransmitlist");
        Ok(c)
    }

    /// Parameters are typed: string writes replace the first string entry;
    /// integer writes update an integer entry in place. Cross-type access
    /// fails.
    pub fn param_int(&self, id: i32, default: i32) -> anyhow::Result<i32> {
        match self
            .params
            .as_ref()
            .and_then(|p| p.iter().find(|p| p.0 == id as i64))
        {
            None => Ok(default),
            Some((_, Arg::Int(v))) => Ok(*v),
            _ => anyhow::bail!("parameter is not an integer entry"),
        }
    }
    pub fn param_string(&self, id: i32, default: Option<&[u16]>) -> anyhow::Result<Option<Text>> {
        match self
            .params
            .as_ref()
            .and_then(|p| p.iter().find(|p| p.0 == id as i64))
        {
            None => Ok(default.map(<[u16]>::to_vec)),
            Some((_, Arg::String(v))) => Ok(Some(v.clone())),
            Some((_, Arg::Null)) => Ok(None),
            _ => anyhow::bail!("parameter is not a string entry"),
        }
    }
    pub fn set_param_int(&mut self, id: i32, value: i32) -> anyhow::Result<()> {
        let p = self.params.get_or_insert_with(Vec::new);
        if let Some((_, v)) = p.iter_mut().find(|p| p.0 == id as i64) {
            let Arg::Int(v) = v else {
                anyhow::bail!("parameter is not an integer entry")
            };
            *v = value;
        } else {
            p.push((id as i64, Arg::Int(value)));
        }
        Ok(())
    }
    pub fn set_param_string(&mut self, id: i32, value: Option<Text>) -> anyhow::Result<()> {
        let p = self.params.get_or_insert_with(Vec::new);
        if let Some(at) = p.iter().position(|p| p.0 == id as i64) {
            anyhow::ensure!(
                !matches!(p[at].1, Arg::Int(_)),
                "parameter is not a string entry"
            );
            p.remove(at);
        }
        p.push((id as i64, value.map_or(Arg::Null, Arg::String)));
        Ok(())
    }
    pub fn remove_param(&mut self, id: i32) {
        if let Some(p) = &mut self.params {
            if let Some(at) = p.iter().position(|p| p.0 == id as i64) {
                p.remove(at);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Interface {
    pub components: Array,
    pub sorted: Option<Array>,
    pub transient: bool,
}
impl Interface {
    pub fn new(components: Vec<Option<Ref>>) -> InterfaceRef {
        Rc::new(RefCell::new(Self {
            components: Rc::new(RefCell::new(components)),
            sorted: None,
            transient: false,
        }))
    }
    pub fn get(&self, packed: i32) -> anyhow::Result<Option<Ref>> {
        let components = self.components.borrow();
        let first = components
            .first()
            .and_then(Option::as_ref)
            .ok_or_else(|| anyhow::anyhow!("interface first component missing"))?;
        anyhow::ensure!(
            (first.borrow().f.parentlayer as u32) >> 16 == (packed as u32) >> 16,
            "component interface mismatch"
        );
        components
            .get((packed & 65535) as usize)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("component index"))
    }
    pub fn sorted(&mut self) -> Array {
        self.sorted
            .get_or_insert_with(|| Rc::new(RefCell::new(self.components.borrow().clone())))
            .clone()
    }
}
#[derive(Clone, Debug, Default)]
pub struct Active {
    pub interface: Option<InterfaceRef>,
    pub component: Option<Ref>,
}
#[derive(Default)]
pub struct Store {
    pub resources: Option<crate::ui_resources::Resources>,
    pub interfaces: BTreeMap<i32, InterfaceRef>,
    /// Retained invalidations; the renderer will consume these by identity.
    pub updated: Vec<Ref>,
}

/// VM graph and property dispatcher over retained components. Store lookups
/// load interfaces through their resource source when one is installed.
/// Other engine operations remain the responsibility of the surrounding host.
pub struct ScriptHost<'a, H> {
    pub engine: &'a mut H,
    pub store: &'a mut Store,
    pub active: &'a mut [Active; 2],
    pub properties: Option<crate::ui_properties::Context<'a>>,
}
impl<H> ScriptHost<'_, H> {
    fn graph(
        &mut self,
        c: &native910::vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
    ) -> Option<anyhow::Result<Option<native910::vm::Value>>> {
        let count = match c.command {
            "cc_create" => 3,
            "cc_find" => 2,
            "cc_deleteall" | "if_find" | "if_sendtofront" | "if_sendtoback" => 1,
            "cc_delete" | "cc_sendtofront" | "cc_sendtoback" => 0,
            _ => return None,
        };
        Some((|| {
            if ints.len() < count {
                // Popping past the stack base is the VM's stack-underflow kind,
                // not a host failure.
                return Err(native910::vm::VmError::StackUnderflow { stack: "int" }.into());
            }
            let args = ints.split_off(ints.len() - count);
            let active = &mut self.active[c.secondary as usize];
            match c.command {
                "cc_create" => {
                    self.store.get(args[0], -1)?;
                    self.store.create(active, args[0], args[1], args[2])?;
                    if let Some(component) = &active.component {
                        component.borrow_mut().creation_origin = Some(CreationOrigin {
                            script: c.script_id,
                            instruction: c.pc,
                            event: c.event.map(str::to_owned),
                        });
                    }
                }
                "cc_delete" => self.store.delete(active)?,
                "cc_deleteall" => self.store.delete_all(args[0])?,
                "cc_find" | "if_find" => {
                    let cc = c.command == "cc_find";
                    let child = if cc { args[1] } else { -1 };
                    return self
                        .store
                        .find(active, args[0], child, cc)
                        .map(|v| Some(native910::vm::Value::Int(v as i32)));
                }
                "cc_sendtofront" | "cc_sendtoback" => {
                    if let Some(interface) = &active.interface {
                        Store::reorder(
                            interface,
                            active.component.as_ref(),
                            c.command == "cc_sendtofront",
                        )?;
                    }
                }
                "if_sendtofront" | "if_sendtoback" => {
                    if let Some(component) = self.store.get(args[0], -1)? {
                        let interface = self
                            .store
                            .interfaces
                            .get(&(args[0] >> 16))
                            .ok_or_else(|| anyhow::anyhow!("missing interface"))?;
                        Store::reorder(interface, Some(&component), c.command == "if_sendtofront")?;
                    }
                }
                _ => unreachable!(),
            }
            Ok(None)
        })())
    }
}
/// VM error kind for a failed component-owner command. The original client
/// aborts the script on any failure and reports it; only an unrouted
/// command means the command does not exist. A
/// typed [`native910::vm::VmError`] raised by the owner (stack underflow, an
/// unimplemented setter reported as `UnknownCommand`) keeps its kind; every
/// other failure is a trap that was dispatched and then failed.
pub(crate) fn component_error(command: &str, error: anyhow::Error) -> native910::vm::VmError {
    match error.downcast::<native910::vm::VmError>() {
        Ok(vm) => vm,
        Err(error) => native910::vm::VmError::TrapFailed {
            command: command.into(),
            reason: format!("{error:#}"),
        },
    }
}
pub(crate) fn component_operation(
    store: &mut Store,
    active: &mut [Active; 2],
    operation: native910::execution::HostOperation,
    context: &native910::vm::InstructionContext<'_>,
    ints: &mut Vec<i32>,
) -> native910::vm::VmResult<Option<native910::vm::Value>> {
    match operation {
        native910::execution::HostOperation::CreateFlatTextChild { .. }
        | native910::execution::HostOperation::ComponentText { .. }
        | native910::execution::HostOperation::ComponentPaint { .. }
        | native910::execution::HostOperation::RetainedPlayerTransmit { .. }
        | native910::execution::HostOperation::VariableBit { .. }
        | native910::execution::HostOperation::Enum { .. }
        | native910::execution::HostOperation::DatabaseField { .. }
        | native910::execution::HostOperation::DatabaseFieldCount { .. } => {
            return Err(native910::vm::VmError::UnknownCommand {
                command: operation.spelling(),
            });
        }
        native910::execution::HostOperation::FindComponent => {
            let packed = ints
                .pop()
                .ok_or(native910::vm::VmError::StackUnderflow { stack: "int" })?;
            let found = store
                .find_component(&mut active[usize::from(context.secondary)], packed)
                .map_err(|error| component_error(context.command, error))?;
            return Ok(Some(native910::vm::Value::Int(i32::from(found))));
        }
        native910::execution::HostOperation::FindFlatChild {
            limit,
            slot_argument,
        } => {
            let packed = ints
                .pop()
                .ok_or(native910::vm::VmError::StackUnderflow { stack: "int" })?;
            let child = context
                .int_locals
                .get(usize::from(slot_argument))
                .copied()
                .ok_or_else(|| {
                    component_error(
                        context.command,
                        anyhow::anyhow!("missing flat child argument"),
                    )
                })?;
            let found = store
                .find_flat_child(
                    &mut active[usize::from(context.secondary)],
                    packed,
                    child,
                    limit,
                )
                .map_err(|error| component_error(context.command, error))?;
            return Ok(Some(native910::vm::Value::Int(i32::from(found))));
        }
        native910::execution::HostOperation::FindRuntimeChild {
            banks,
            bank_argument,
            slot_argument,
        } => {
            let packed = ints
                .pop()
                .ok_or(native910::vm::VmError::StackUnderflow { stack: "int" })?;
            let argument = |slot| {
                context
                    .int_locals
                    .get(usize::from(slot))
                    .copied()
                    .ok_or_else(|| {
                        component_error(
                            context.command,
                            anyhow::anyhow!("missing child selection argument"),
                        )
                    })
            };
            let found = store
                .find_runtime_child(
                    &mut active[usize::from(context.secondary)],
                    packed,
                    argument(bank_argument)?,
                    argument(slot_argument)?,
                    banks,
                )
                .map_err(|error| component_error(context.command, error))?;
            return Ok(Some(native910::vm::Value::Int(i32::from(found))));
        }
        native910::execution::HostOperation::NextRuntimeChildSlot {
            banks,
            bank_argument,
        } => {
            let packed = ints
                .pop()
                .ok_or(native910::vm::VmError::StackUnderflow { stack: "int" })?;
            let bank = context
                .int_locals
                .get(usize::from(bank_argument))
                .copied()
                .ok_or_else(|| {
                    component_error(context.command, anyhow::anyhow!("missing bank argument"))
                })?;
            let slot = store
                .packed_next_runtime_child_slot(packed, bank, banks)
                .map_err(|error| component_error(context.command, error))?;
            return Ok(Some(native910::vm::Value::Int(slot)));
        }
        native910::execution::HostOperation::ClearRuntimeChildren => {
            let packed = ints
                .pop()
                .ok_or(native910::vm::VmError::StackUnderflow { stack: "int" })?;
            store
                .clear_runtime_children(packed)
                .map_err(|error| component_error(context.command, error))?;
        }
    }
    Ok(None)
}
struct FlatTextChild {
    pub packed: i32,
    pub kind: i32,
    pub child: i32,
    pub limit: u16,
    pub origin: CreationOrigin,
}

impl Store {
    /// Flat identity validation precedes loading. An absent static parent or
    /// a non-container preserves selection, while interface loading errors trap.
    fn create_flat_text_child(
        &mut self,
        active: &mut Active,
        request: FlatTextChild,
        template: rs910_config::ui_component_fields::TextChildTemplate,
    ) -> anyhow::Result<()> {
        let encoded = u16::try_from(request.child)
            .ok()
            .filter(|id| *id < request.limit)
            .ok_or_else(|| anyhow::anyhow!("invalid flat child identity"))?;
        let parent = self.explicit_component(request.packed)?;
        const GROUP_SHIFT: u32 = 16;
        let group = ((request.packed as u32) >> GROUP_SHIFT) as i32;
        let interface = self
            .interfaces
            .get(&group)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime child interface missing"))?;
        let Some(parent) = parent.filter(|parent| parent.borrow().is_container()) else {
            return Ok(());
        };
        anyhow::ensure!(
            request.kind as u8 == template.source_kind,
            "unmapped child constructor kind"
        );
        let child = Component {
            f: template.fields(),
            creation_origin: Some(request.origin),
            ..Component::default()
        };
        self.attach_runtime_child(
            active,
            &interface,
            &parent,
            child,
            RuntimeChildId::new(encoded)?,
        )
    }
}

pub(crate) fn component_creation_operation(
    store: &mut Store,
    active: &mut [Active; 2],
    operation: native910::execution::HostOperation,
    context: &native910::vm::InstructionContext<'_>,
    resource: Option<&native910::execution::Resource>,
    ints: &mut Vec<i32>,
) -> native910::vm::VmResult<Option<native910::vm::Value>> {
    let native910::execution::HostOperation::CreateFlatTextChild {
        limit,
        kind_argument,
        slot_argument,
        source,
    } = operation
    else {
        return Err(native910::vm::VmError::UnknownCommand {
            command: operation.spelling(),
        });
    };
    let packed = ints
        .pop()
        .ok_or(native910::vm::VmError::StackUnderflow { stack: "int" })?;
    let argument = |slot: u16| {
        context
            .int_locals
            .get(usize::from(slot))
            .copied()
            .ok_or_else(|| {
                component_error(
                    context.command,
                    anyhow::anyhow!("missing creation argument"),
                )
            })
    };
    let (script, instruction) = source.ok_or_else(|| {
        component_error(
            context.command,
            anyhow::anyhow!("creation source identity missing"),
        )
    })?;
    let template = rs910_config::ui_component_fields::TextChildTemplate::decode_resource(
        resource
            .ok_or_else(|| {
                component_error(
                    context.command,
                    anyhow::anyhow!("constructor resource missing"),
                )
            })?
            .bytes(),
    )
    .map_err(|error| component_error(context.command, error))?;
    store
        .create_flat_text_child(
            &mut active[usize::from(context.secondary)],
            FlatTextChild {
                packed,
                kind: argument(kind_argument)?,
                child: argument(slot_argument)?,
                limit,
                origin: CreationOrigin {
                    script: Some(script),
                    instruction,
                    event: context.event.map(str::to_owned),
                },
            },
            template,
        )
        .map(|()| None)
        .map_err(|error| component_error(context.command, error))
}
impl<H: native910::vm::Host> native910::vm::Host for ScriptHost<'_, H> {
    fn trap_resource_context(
        &mut self,
        operation: native910::execution::HostOperation,
        context: &native910::vm::InstructionContext<'_>,
        resource: Option<&native910::execution::Resource>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        if matches!(
            operation,
            native910::execution::HostOperation::ComponentText { .. }
        ) {
            return self
                .properties
                .as_mut()
                .ok_or_else(|| {
                    component_error(
                        context.command,
                        anyhow::anyhow!("component property owner missing"),
                    )
                })?
                .text_component(self.store, self.active, context, operation, resource, ints)
                .map_err(|error| component_error(context.command, error));
        }
        if matches!(
            operation,
            native910::execution::HostOperation::CreateFlatTextChild { .. }
        ) {
            return component_creation_operation(
                self.store,
                self.active,
                operation,
                context,
                resource,
                ints,
            );
        }
        if operation.needs_resource() {
            self.engine
                .trap_resource_context(operation, context, resource, ints, objects, longs)
        } else {
            self.trap_operation_context(operation, context, ints, objects, longs)
        }
    }
    fn trap_operation_context(
        &mut self,
        operation: native910::execution::HostOperation,
        context: &native910::vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        if let native910::execution::HostOperation::RetainedPlayerTransmit {
            event_tokens, ..
        } = operation
        {
            return crate::ui_properties::install_retained_player(
                self.active,
                context,
                event_tokens,
                ints,
                objects,
                longs,
            )
            .map(|()| None)
            .map_err(|error| component_error(context.command, error));
        }
        if matches!(
            operation,
            native910::execution::HostOperation::ComponentText { .. }
        ) {
            return self.trap_resource_context(operation, context, None, ints, objects, longs);
        }
        if let native910::execution::HostOperation::ComponentPaint { property, explicit } =
            operation
        {
            let properties = self.properties.as_mut().ok_or_else(|| {
                component_error(
                    context.command,
                    anyhow::anyhow!("component property owner missing"),
                )
            })?;
            return properties
                .paint_component(self.store, self.active, context, property, explicit, ints)
                .map(|()| None)
                .map_err(|error| component_error(context.command, error));
        }
        component_operation(self.store, self.active, operation, context, ints)
    }
    fn trap_objects_context(
        &mut self,
        c: &native910::vm::InstructionContext<'_>,
        i: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        l: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        if let Some(properties) = &mut self.properties {
            if let Some(result) =
                properties.dispatch_objects(self.store, self.active, c, i, objects, l)
            {
                return result.map_err(|e| component_error(c.command, e));
            }
        }
        native910::vm::with_string_stack(c.command, objects, |strings| {
            self.trap_context(c, i, strings, l)
        })
    }
    fn trap_context(
        &mut self,
        c: &native910::vm::InstructionContext<'_>,
        i: &mut Vec<i32>,
        s: &mut Vec<String>,
        l: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        if let Some(result) = self.graph(c, i) {
            return result.map_err(|e| component_error(c.command, e));
        }
        if let Some(properties) = &mut self.properties {
            if let Some(result) = properties.dispatch(self.store, self.active, c, i, s, l) {
                return result.map_err(|e| component_error(c.command, e));
            }
        }
        self.engine.trap_context(c, i, s, l)
    }
    fn var_type(
        &mut self,
        d: native910::vars::VarScope,
        id: u16,
    ) -> native910::vm::VmResult<native910::vm::VarLane> {
        self.engine.var_type(d, id)
    }
    fn var_get(
        &mut self,
        d: native910::vars::VarScope,
        id: u16,
        s: bool,
    ) -> native910::vm::VmResult<native910::vm::Value> {
        self.engine.var_get(d, id, s)
    }
    fn var_set(
        &mut self,
        d: native910::vars::VarScope,
        id: u16,
        s: bool,
        v: native910::vm::Value,
    ) -> native910::vm::VmResult<()> {
        self.engine.var_set(d, id, s, v)
    }
    fn varbit_get(&mut self, id: u16, s: bool) -> native910::vm::VmResult<i32> {
        self.engine.varbit_get(id, s)
    }
    fn varbit_set(&mut self, id: u16, s: bool, v: i32) -> native910::vm::VmResult<()> {
        self.engine.varbit_set(id, s, v)
    }
    fn array_define(&mut self, id: i32, n: usize) -> native910::vm::VmResult<()> {
        self.engine.array_define(id, n)
    }
    fn array_len(&mut self, id: i32) -> native910::vm::VmResult<usize> {
        self.engine.array_len(id)
    }
    fn array_get(&mut self, id: i32, n: i32) -> native910::vm::VmResult<i32> {
        self.engine.array_get(id, n)
    }
    fn array_set(&mut self, id: i32, n: i32, v: i32) -> native910::vm::VmResult<()> {
        self.engine.array_set(id, n, v)
    }
    fn take_effects(&mut self) -> Vec<native910::vm::HostEffect> {
        self.engine.take_effects()
    }
}
impl Store {
    pub fn get(&mut self, packed: i32, child: i32) -> anyhow::Result<Option<Ref>> {
        let id = packed >> 16;
        if let Some(r) = &self.resources {
            r.valid(id)?;
        }
        let missing = match self.interfaces.get(&id) {
            None => true,
            Some(i) => i.borrow().get(packed)?.is_none(),
        };
        if missing && !self.open(id, None)? {
            return Ok(None);
        }
        self.loaded(packed, child)
    }

    /// Lookup in an already loaded interface, using the same runtime namespace
    /// as script access. Observers never load a missing group as a side effect.
    pub fn loaded(&self, packed: i32, child: i32) -> anyhow::Result<Option<Ref>> {
        let id = packed >> 16;
        let Some(interface) = self.interfaces.get(&id) else {
            return Ok(None);
        };
        let base = interface.borrow().get(packed)?;
        if child == -1 {
            return Ok(base);
        }
        let Some(base) = base else { return Ok(None) };
        let banked = {
            let base = base.borrow();
            base.runtime_lookup.is_some()
                || base.children.as_ref().is_some_and(|children| {
                    children
                        .borrow()
                        .iter()
                        .flatten()
                        .any(|child| child.borrow().has_runtime_parent())
                })
        };
        if banked {
            let id = u16::try_from(child)
                .ok()
                .and_then(|encoded| RuntimeChildId::new(encoded).ok());
            return Ok(id.and_then(|id| base.borrow().runtime_child(id)));
        }
        let Some(children) = &base.borrow().children else {
            return Ok(None);
        };
        let children = children.borrow();
        if child >= children.len() as i32 {
            return Ok(None);
        }
        children
            .get(child as usize)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("negative child index"))
    }
    /// The component lookup behind `cc_find`. `cc_find(-1)` short-circuits
    /// and preserves the prior active reference.
    pub fn find(
        &mut self,
        active: &mut Active,
        packed: i32,
        child: i32,
        cc: bool,
    ) -> anyhow::Result<bool> {
        if cc && child == -1 {
            return Ok(false);
        }
        match self.get(packed, child)? {
            Some(component) => {
                active.interface = self.interfaces.get(&(packed >> 16)).cloned();
                active.component = Some(component);
                Ok(true)
            }
            None => {
                *active = Active::default();
                Ok(false)
            }
        }
    }
    /// cc_create_inner. Array growth precedes the
    /// predecessor check, including its mutation on a failed creation.
    pub fn create(
        &mut self,
        active: &mut Active,
        packed: i32,
        kind: i32,
        child: i32,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(kind != 0, "cannot create layer");
        let interface = self
            .interfaces
            .get(&(packed >> 16))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unloaded interface"))?;
        let base = interface
            .borrow()
            .get(packed)?
            .ok_or_else(|| anyhow::anyhow!("missing parent"))?;
        let mut parent = base.borrow_mut();
        let count = child
            .checked_add(1)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| anyhow::anyhow!("negative array length"))?;
        if parent.children.is_none() {
            let array = Rc::new(RefCell::new(vec![None; count]));
            parent.children = Some(array.clone());
            parent.sorted = Some(array);
        }
        let array = parent.children.as_ref().unwrap().clone();
        if array.borrow().len() <= child as usize && child >= 0 {
            let sorted = parent
                .sorted
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing sorted array"))?
                .clone();
            let mut next = array.borrow().clone();
            next.resize(count, None);
            let next = Rc::new(RefCell::new(next));
            if Rc::ptr_eq(&array, &sorted) {
                parent.sorted = Some(next.clone());
            } else {
                let mut order = sorted.borrow().clone();
                order.resize(count, None);
                parent.sorted = Some(Rc::new(RefCell::new(order)));
            }
            parent.children = Some(next);
        }
        let array = parent.children.as_ref().unwrap().clone();
        anyhow::ensure!(child >= 0, "negative child index");
        anyhow::ensure!(
            child == 0 || array.borrow()[child as usize - 1].is_some(),
            "missing preceding child"
        );
        let mut c = Component::default();
        c.f.r#type = kind;
        c.f.layer = parent.f.parentlayer;
        c.f.parentlayer = parent.f.parentlayer;
        c.f.id = child;
        let c = Rc::new(RefCell::new(c));
        array.borrow_mut()[child as usize] = Some(c.clone());
        let sorted = parent
            .sorted
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing sorted array"))?;
        if !Rc::ptr_eq(&array, sorted) {
            sorted.borrow_mut()[child as usize] = Some(c.clone());
        }
        active.interface = Some(interface);
        active.component = Some(c);
        drop(parent);
        self.updated.push(base);
        Ok(())
    }
    pub fn delete(&mut self, active: &Active) -> anyhow::Result<()> {
        let c = active
            .component
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no active component"))?
            .borrow();
        anyhow::ensure!(c.f.id != -1, "cannot delete static component");
        let i = active
            .interface
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no active interface"))?;
        let parent = i
            .borrow()
            .get(c.f.parentlayer)?
            .ok_or_else(|| anyhow::anyhow!("missing parent"))?;
        let array = parent
            .borrow()
            .children
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing children"))?;
        *array
            .borrow_mut()
            .get_mut(c.f.id as usize)
            .ok_or_else(|| anyhow::anyhow!("child index"))? = None;
        self.updated.push(parent);
        Ok(())
    }
    pub fn delete_all(&mut self, packed: i32) -> anyhow::Result<()> {
        let c = self
            .get(packed, -1)?
            .ok_or_else(|| anyhow::anyhow!("missing component"))?;
        c.borrow_mut().children = None;
        c.borrow_mut().sorted = None;
        self.updated.push(c);
        Ok(())
    }
    /// Resolve a packed cache component with unsigned fields and a missing
    /// result for absent or out-of-range ordinals. Loading precedes operand
    /// checks in the modern host operations that use this lookup.
    pub(crate) fn explicit_component(&mut self, packed: i32) -> anyhow::Result<Option<Ref>> {
        const ABSENT_COMPONENT: i32 = -1;
        const GROUP_SHIFT: u32 = 16;
        const COMPONENT_MASK: i32 = 0xffff;
        Ok(if packed == ABSENT_COMPONENT {
            None
        } else {
            let group = ((packed as u32) >> GROUP_SHIFT) as i32;
            if self.open(group, None)? {
                self.interfaces.get(&group).and_then(|interface| {
                    interface
                        .borrow()
                        .components
                        .borrow()
                        .get((packed & COMPONENT_MASK) as usize)
                        .and_then(Clone::clone)
                })
            } else {
                None
            }
        })
    }

    /// Select a packed cache component with unsigned address fields. Lookup
    /// failure clears both active references; loading failure leaves them
    /// unchanged. A successful lookup changes only the selected active slot.
    pub fn find_component(&mut self, active: &mut Active, packed: i32) -> anyhow::Result<bool> {
        let component = self.explicit_component(packed)?;
        let found = component.is_some();
        active.component = component;
        const GROUP_SHIFT: u32 = 16;
        active.interface = if found {
            self.interfaces
                .get(&(((packed as u32) >> GROUP_SHIFT) as i32))
                .cloned()
        } else {
            None
        };
        Ok(found)
    }

    /// Select a banked child in the packed container's namespace. A missing
    /// child preserves both active references; invalid operands and invalid
    /// containers fail before selection. Successful lookup resolves the owning
    /// interface before assigning either reference.
    pub fn find_runtime_child(
        &mut self,
        active: &mut Active,
        packed: i32,
        bank: i32,
        slot: i32,
        banks: native910::execution::RuntimeChildBanks,
    ) -> anyhow::Result<bool> {
        let target = self.explicit_component(packed)?;
        let id = RuntimeChildId::new(banks.encode(bank, slot)?)?;
        let target = target.ok_or_else(|| anyhow::anyhow!("missing component"))?;
        anyhow::ensure!(
            target.borrow().is_container(),
            "component is not a container"
        );
        let child = target.borrow().runtime_child(id);
        let Some(child) = child else { return Ok(false) };
        const GROUP_SHIFT: u32 = 16;
        let group = ((target.borrow().f.parentlayer as u32) >> GROUP_SHIFT) as i32;
        self.open(group, None)?;
        *active = Active {
            component: Some(child),
            interface: self.interfaces.get(&group).cloned(),
        };
        Ok(true)
    }

    /// Resolve an encoded child, checking its source range before loading the
    /// parent. Invalid identities preserve selection; a missing child or parent
    /// clears both references. Direct children precede the retained shared index.
    pub fn find_flat_child(
        &mut self,
        active: &mut Active,
        packed: i32,
        child: i32,
        limit: u16,
    ) -> anyhow::Result<bool> {
        let Some(encoded) = u16::try_from(child).ok().filter(|encoded| *encoded < limit) else {
            return Ok(false);
        };
        let id = RuntimeChildId::new(encoded)?;
        let parent = self.explicit_component(packed)?;
        let child = parent.and_then(|parent| parent.borrow().runtime_child(id));
        let found = child.is_some();
        const GROUP_SHIFT: u32 = 16;
        active.component = child;
        active.interface = if found {
            self.interfaces
                .get(&(((packed as u32) >> GROUP_SHIFT) as i32))
                .cloned()
        } else {
            None
        };
        Ok(found)
    }

    /// Query an explicit cache component without changing either active target.
    /// Lookup/loading precedes bank validation, including on failing queries.
    pub fn packed_next_runtime_child_slot(
        &mut self,
        packed: i32,
        bank: i32,
        banks: native910::execution::RuntimeChildBanks,
    ) -> anyhow::Result<i32> {
        let component = self.explicit_component(packed)?;
        banks.range(bank)?;
        let component = component.ok_or_else(|| anyhow::anyhow!("missing component"))?;
        self.next_runtime_child_slot(&component, bank, banks)
    }

    /// First free slot in the static owner's bank. Direct arrays reject a full
    /// bank; a materialized shared index returns its capacity. Queries preserve
    /// that distinction and never allocate the shared index themselves.
    pub fn next_runtime_child_slot(
        &self,
        target: &Ref,
        bank: i32,
        banks: native910::execution::RuntimeChildBanks,
    ) -> anyhow::Result<i32> {
        let (start, capacity) = banks.range(bank)?;
        anyhow::ensure!(
            target.borrow().is_container(),
            "component is not a container"
        );
        let owner = {
            let target_state = target.borrow();
            if let Some(link) = &target_state.runtime_link {
                link.owner
                    .upgrade()
                    .ok_or_else(|| anyhow::anyhow!("expired runtime owner"))?
            } else {
                const STATIC_COMPONENT_ID: i32 = -1;
                anyhow::ensure!(
                    target_state.f.id == STATIC_COMPONENT_ID,
                    "runtime container has no owner"
                );
                target.clone()
            }
        };
        let owner_state = owner.borrow();
        let Some(children) = &owner_state.children else {
            return Ok(i32::default());
        };
        if children.borrow().is_empty() {
            return Ok(i32::default());
        }
        let indexed = owner_state.runtime_lookup.is_some();
        let gap = |identities: &mut dyn Iterator<Item = u16>| {
            let mut slot = u16::default();
            for encoded in identities
                .skip_while(|encoded| *encoded < start)
                .take_while(|encoded| *encoded < start + capacity)
            {
                if encoded - start != slot {
                    break;
                }
                slot += 1;
            }
            slot
        };
        let slot = if let Some(index) = &owner_state.runtime_lookup {
            let first = RuntimeChildId::new(start)?;
            gap(&mut index.range(first..).map(|(id, _)| id.encoded()))
        } else {
            gap(&mut children
                .borrow()
                .iter()
                .flatten()
                .filter_map(|child| u16::try_from(child.borrow().f.id).ok()))
        };
        anyhow::ensure!(indexed || slot < capacity, "runtime child bank is full");
        Ok(i32::from(slot))
    }

    /// Recursively empty runtime children and drawing order. Cache-defined
    /// components are a separate owner; missing/non-container targets do nothing.
    pub fn clear_runtime_children(&mut self, packed: i32) -> anyhow::Result<()> {
        const ABSENT_COMPONENT: i32 = -1;
        const GROUP_SHIFT: u32 = 16;
        const COMPONENT_MASK: i32 = 0xffff;
        if packed == ABSENT_COMPONENT {
            return Ok(());
        }
        let group = ((packed as u32) >> GROUP_SHIFT) as i32;
        if !self.open(group, None)? {
            return Ok(());
        }
        let Some(interface) = self.interfaces.get(&group) else {
            return Ok(());
        };
        let array = interface.borrow().components.clone();
        let component = array
            .borrow()
            .get((packed & COMPONENT_MASK) as usize)
            .and_then(Clone::clone);
        if let Some(component) = component.filter(|component| component.borrow().is_container()) {
            clear_runtime_subtree(&component);
            self.updated.push(component);
        }
        Ok(())
    }
    /// cc_if_sendto{front,back}. Cloning an aliased child
    /// array uses the child's index; later moves search by object identity.
    pub fn reorder(
        interface: &InterfaceRef,
        component: Option<&Ref>,
        front: bool,
    ) -> anyhow::Result<()> {
        let Some(c) = component else { return Ok(()) };
        if c.borrow().has_runtime_parent() {
            let parent = c
                .borrow()
                .runtime_parent()
                .ok_or_else(|| anyhow::anyhow!("expired runtime parent"))?;
            let array = {
                let mut parent = parent.borrow_mut();
                let children = parent
                    .children
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("missing runtime children"))?;
                match parent.sorted.clone() {
                    Some(order) if !Rc::ptr_eq(&children, &order) && !order.borrow().is_empty() => {
                        order
                    }
                    existing => {
                        let order = existing
                            .filter(|order| !Rc::ptr_eq(&children, order))
                            .unwrap_or_else(|| Rc::new(RefCell::new(Vec::new())));
                        *order.borrow_mut() = children.borrow().clone();
                        parent.sorted = Some(order.clone());
                        order
                    }
                }
            };
            reorder_identity(&array, c, front);
            return Ok(());
        }
        let (id, layer) = {
            let f = &c.borrow().f;
            (f.id, f.layer)
        };
        let array = if id == -1 {
            interface.borrow_mut().sorted()
        } else {
            let parent = if front {
                interface.borrow().get(layer)?
            } else {
                interface
                    .borrow()
                    .components
                    .borrow()
                    .get((layer & 65535) as usize)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("parent index"))?
            };
            let Some(parent) = parent else { return Ok(()) };
            let mut parent = parent.borrow_mut();
            let children = parent
                .children
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing children"))?;
            let sorted = parent
                .sorted
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing sorted children"))?;
            if Rc::ptr_eq(&children, &sorted) {
                let mut next = children.borrow().clone();
                anyhow::ensure!(id >= 0 && (id as usize) < next.len(), "child index");
                next.remove(id as usize);
                if front {
                    next.push(Some(c.clone()))
                } else {
                    next.insert(0, Some(c.clone()));
                }
                parent.sorted = Some(Rc::new(RefCell::new(next)));
                return Ok(());
            }
            sorted
        };
        reorder_identity(&array, c, front);
        Ok(())
    }
}

fn reorder_identity(array: &Array, component: &Ref, front: bool) {
    let mut array = array.borrow_mut();
    if let Some(position) = array.iter().position(|value| {
        value
            .as_ref()
            .is_some_and(|value| Rc::ptr_eq(value, component))
    }) {
        let value = array.remove(position);
        if front {
            array.push(value);
        } else {
            array.insert(0, value);
        }
    }
}

impl Component {
    pub fn has_runtime_parent(&self) -> bool {
        self.runtime_link.is_some()
    }
    pub fn runtime_parent(&self) -> Option<Ref> {
        self.runtime_link
            .as_ref()
            .and_then(|link| link.parent.upgrade())
    }
    /// Read this parent's current entry by encoded identity. Retained replaced
    /// references address the replacement; absent entries and expired parents
    /// return visible. Legacy components keep their scalar visibility.
    pub fn runtime_entry_hidden(&self) -> Option<bool> {
        self.runtime_link.as_ref()?;
        Some(self.runtime_parent().is_some_and(|parent| {
            u16::try_from(self.f.id)
                .ok()
                .and_then(|encoded| RuntimeChildId::new(encoded).ok())
                .is_some_and(|id| {
                    let parent = parent.borrow();
                    parent.runtime_hidden.contains(&id)
                        && parent.children.as_ref().is_some_and(|children| {
                            children
                                .borrow()
                                .iter()
                                .flatten()
                                .any(|current| current.borrow().f.id == i32::from(id.encoded()))
                        })
                })
        }))
    }
    /// Resolve within this static owner's namespace. Direct children precede
    /// the shared index, which materializes when a nested container inserts.
    pub fn runtime_child(&self, id: RuntimeChildId) -> Option<Ref> {
        if !self.is_container() {
            return None;
        }
        self.children
            .as_ref()
            .and_then(|children| {
                children
                    .borrow()
                    .iter()
                    .flatten()
                    .find(|child| child.borrow().f.id == i32::from(id.0))
                    .cloned()
            })
            .or_else(|| {
                self.runtime_lookup
                    .as_ref()
                    .and_then(|index| index.get(&id).cloned())
            })
    }
}

impl Component {
    pub(crate) fn is_container(&self) -> bool {
        const CONTAINER_COMPONENT_KIND: i32 = 0;
        self.f.r#type == CONTAINER_COMPONENT_KIND
    }
}

pub(crate) fn clear_runtime_subtree(component: &Ref) {
    let (children, sorted) = {
        let owner = component.borrow();
        (owner.children.clone(), owner.sorted.clone())
    };
    if let Some(children) = &children {
        // Recursion precedes reference release. Retained containers lose their
        // descendants too, including when traversal holds an array reference.
        let descendants = children.borrow().clone();
        for child in descendants.iter().flatten() {
            if child.borrow().is_container() {
                clear_runtime_subtree(child);
            }
        }
        remove_lookup_children(component, &descendants);
        children.borrow_mut().clear();
    }
    if let Some(sorted) = sorted {
        sorted.borrow_mut().clear();
    }
    component.borrow_mut().runtime_hidden.clear();
}

pub(crate) fn runtime_child_attached(component: &Ref) -> Option<bool> {
    let parent = {
        let component = component.borrow();
        let link = component.runtime_link.as_ref()?;
        if link.owner.upgrade().is_none() {
            return Some(false);
        }
        component.runtime_parent()
    };
    Some(parent.is_some_and(|parent| {
        parent.borrow().children.as_ref().is_some_and(|children| {
            children
                .borrow()
                .iter()
                .flatten()
                .any(|current| Rc::ptr_eq(current, component))
        })
    }))
}

pub(crate) fn remove_lookup_children(parent: &Ref, children: &[Option<Ref>]) {
    const STATIC_COMPONENT_ID: i32 = -1;
    let owner = {
        let parent_state = parent.borrow();
        match &parent_state.runtime_link {
            Some(link) => link.owner.upgrade(),
            None if parent_state.f.id == STATIC_COMPONENT_ID => Some(parent.clone()),
            None => None,
        }
    };
    let Some(owner) = owner else {
        return;
    };
    let mut state = owner.borrow_mut();
    if let Some(index) = &mut state.runtime_lookup {
        for child in children.iter().flatten() {
            if let Ok(encoded) = u16::try_from(child.borrow().f.id) {
                if let Ok(id) = RuntimeChildId::new(encoded) {
                    index.remove(&id);
                }
            }
        }
    }
}

#[cfg(test)]
mod host_error_tests {
    //! Error kinds the component host reports to the VM. The original client
    //! aborts the script on any failure; only an unrouted command means the
    //! command does not exist. Each case runs a compiled script through
    //! the production `Vm` + `ScriptHost` chain (component graph, property
    //! dispatcher, then the runtime engine).
    use super::*;
    use crate::ui_runtime::Engine;
    use native910::script::{CompiledScript, Counts, Instruction, Operand};
    use native910::vm::{Value, Vm, VmError, VmResult};

    fn script(ints: &[i32], strs: &[&str], command: &str) -> CompiledScript {
        let mut code: Vec<Instruction> = ints
            .iter()
            .map(|&v| Instruction {
                opcode: 0,
                command: "push_constant_int".into(),
                operand: Operand::Int(v),
            })
            .collect();
        code.extend(strs.iter().map(|&s| Instruction {
            opcode: 0,
            command: "push_constant_string".into(),
            operand: Operand::Str(s.into()),
        }));
        code.push(Instruction {
            opcode: 0,
            command: command.into(),
            operand: Operand::Byte(0),
        });
        code.push(Instruction {
            opcode: 0,
            command: "return".into(),
            operand: Operand::Byte(0),
        });
        CompiledScript {
            name: Some("component-host-errors".into()),
            locals: Counts::default(),
            args: Counts::default(),
            code,
        }
    }

    /// Interface 77 with one static root component (`id == -1`), made the
    /// primary active component.
    fn static_active() -> (Store, [Active; 2]) {
        let mut component = Component::default();
        component.f.parentlayer = 77 << 16;
        component.f.id = -1;
        let component = Rc::new(RefCell::new(component));
        let interface = Interface::new(vec![Some(component.clone())]);
        let mut store = Store::default();
        store.interfaces.insert(77, interface.clone());
        let active = [
            Active {
                interface: Some(interface),
                component: Some(component),
            },
            Active::default(),
        ];
        (store, active)
    }

    fn run(ints: &[i32], strs: &[&str], command: &str) -> VmResult<Option<Value>> {
        let (mut store, mut active) = static_active();
        let mut engine = Engine::default();
        let mut state = crate::ui_properties::State::default();
        let mut changes = crate::ui_changes::Changes::default();
        let mut now = || 0i64;
        let mut host = ScriptHost {
            engine: &mut engine,
            store: &mut store,
            active: &mut active,
            properties: Some(crate::ui_properties::Context {
                state: &mut state,
                changes: &mut changes,
                now: &mut now,
                nested_count: 0,
            }),
        };
        Vm::new(&mut host, &()).execute(&script(ints, strs, command), &[])
    }

    fn verify_recursive_child_removal() {
        const GROUP: i32 = 1;
        const GROUP_SHIFT: u32 = 16;
        const ROOT_FILE: i32 = 0;
        const LABEL_FILE: i32 = 1;
        const MISSING_FILE: i32 = 2;
        const OUT_OF_RANGE_FILE: i32 = 3;
        const ABSENT_COMPONENT: i32 = -1;
        const CONTAINER_KIND: i32 = 0;
        const TEXT_KIND: i32 = 4; // not a content id: interface component type tag.
        const FIRST_CHILD: i32 = 0;
        let address = |file| (GROUP << GROUP_SHIFT) | file;
        let component = |kind, file| {
            let mut component = Component::default();
            component.f.r#type = kind;
            component.f.parentlayer = address(file);
            Rc::new(RefCell::new(component))
        };
        let root = component(CONTAINER_KIND, ROOT_FILE);
        let label = component(TEXT_KIND, LABEL_FILE);
        let container = component(CONTAINER_KIND, ROOT_FILE);
        let leaf = component(TEXT_KIND, ROOT_FILE);
        let grandchildren = Rc::new(RefCell::new(vec![Some(leaf.clone())]));
        container.borrow_mut().children = Some(grandchildren.clone());
        container.borrow_mut().sorted = Some(grandchildren.clone());
        let children = Rc::new(RefCell::new(vec![
            Some(container.clone()),
            Some(leaf.clone()),
        ]));
        let sorted = Rc::new(RefCell::new(vec![
            Some(leaf.clone()),
            Some(container.clone()),
        ]));
        root.borrow_mut().children = Some(children.clone());
        root.borrow_mut().sorted = Some(sorted.clone());
        // Non-containers are successful no-ops even when retained graph state exists.
        label.borrow_mut().children = Some(grandchildren.clone());
        let interface = Interface::new(vec![Some(root.clone()), Some(label.clone()), None]);
        let static_children = interface.borrow().components.clone();
        let static_sorted = interface.borrow_mut().sorted();
        let mut store = Store::default();
        store.interfaces.insert(GROUP, interface.clone());
        for packed in [
            ABSENT_COMPONENT,
            address(LABEL_FILE),
            address(MISSING_FILE),
            address(OUT_OF_RANGE_FILE),
        ] {
            store.clear_runtime_children(packed).unwrap();
            assert!(!children.borrow().is_empty());
            assert!(!grandchildren.borrow().is_empty());
        }
        store.clear_runtime_children(address(ROOT_FILE)).unwrap();
        assert!(children.borrow().is_empty());
        assert!(sorted.borrow().is_empty());
        assert!(grandchildren.borrow().is_empty());
        assert!(Rc::ptr_eq(
            root.borrow().children.as_ref().unwrap(),
            &children
        ));
        assert!(Rc::ptr_eq(
            container.borrow().children.as_ref().unwrap(),
            &grandchildren
        ));
        assert!(store
            .get(address(ROOT_FILE), FIRST_CHILD)
            .unwrap()
            .is_none());
        assert!(Rc::ptr_eq(
            &store
                .get(address(LABEL_FILE), ABSENT_COMPONENT)
                .unwrap()
                .unwrap(),
            &label
        ));
        assert!(Rc::ptr_eq(&interface.borrow().components, &static_children));
        assert!(Rc::ptr_eq(
            interface.borrow().sorted.as_ref().unwrap(),
            &static_sorted
        ));
        assert_eq!(static_children.borrow().len(), static_sorted.borrow().len());
        assert!(Rc::ptr_eq(
            static_sorted.borrow()[LABEL_FILE as usize]
                .as_ref()
                .unwrap(),
            &label
        ));
        // The native flat operation releases only the parent arrays. Retained
        // arrays and descendant containers preserve their existing contents.
        grandchildren.borrow_mut().push(Some(leaf));
        children.borrow_mut().push(Some(container));
        store.delete_all(address(ROOT_FILE)).unwrap();
        assert!(root.borrow().children.is_none());
        assert!(root.borrow().sorted.is_none());
        assert!(!children.borrow().is_empty());
        assert!(!grandchildren.borrow().is_empty());
        // A packed address can be negative while its unsigned group is valid.
        const GROUP_WITH_HIGH_BIT: i32 = 0x8000;
        let high_address = (GROUP_WITH_HIGH_BIT << GROUP_SHIFT) | ROOT_FILE;
        let mut high_root = Component::default();
        high_root.f.parentlayer = high_address;
        let high_children = Rc::new(RefCell::new(vec![Some(root)]));
        high_root.children = Some(high_children.clone());
        high_root.sorted = Some(high_children.clone());
        let high_root = Rc::new(RefCell::new(high_root));
        store
            .interfaces
            .insert(GROUP_WITH_HIGH_BIT, Interface::new(vec![Some(high_root)]));
        store.clear_runtime_children(high_address).unwrap();
        assert!(high_children.borrow().is_empty());
    }

    fn verify_banked_child_selection(
        store: &mut Store,
        active: &mut [Active; 2],
        root: &Ref,
        banks: native910::execution::RuntimeChildBanks,
    ) {
        use native910::{
            execution::{decode_accounting, HostOperation, Role, Specification, Table},
            opcode::OpcodeBook,
            vm::{Programs, Session},
        };
        use std::collections::BTreeMap;
        const ROOT_BANK: i32 = 0;
        const NESTED_BANK: i32 = 2;
        const PRESENT_SLOT: i32 = 0;
        const MISSING_SLOT: i32 = 1;
        const INVALID_ARGUMENT: i32 = -1;
        const PRIMARY: usize = 0;
        const SECONDARY: usize = 1;
        const COMPONENT_ARGUMENT: i32 = 0;
        const BANK_ARGUMENT: u16 = 1;
        const SLOT_ARGUMENT: u16 = 2;
        const ARGUMENT_COUNT: u16 = 3;
        const HOST_PC: usize = 1;
        const SYNTHETIC_ADAPTER: i32 = 1; // not a content id: isolated VM provider.
        let saved = active.clone();
        let packed = root.borrow().f.parentlayer;
        let dirty_before = store.updated.len();
        assert!(store
            .find_runtime_child(
                &mut active[SECONDARY],
                packed,
                ROOT_BANK,
                PRESENT_SLOT,
                banks
            )
            .unwrap());
        let expected_nested = root
            .borrow()
            .runtime_child(
                super::RuntimeChildId::new(banks.encode(NESTED_BANK, PRESENT_SLOT).unwrap())
                    .unwrap(),
            )
            .unwrap();
        let operation = HostOperation::FindRuntimeChild {
            banks,
            bank_argument: BANK_ARGUMENT,
            slot_argument: SLOT_ARGUMENT,
        };
        assert_eq!(
            HostOperation::parse(&operation.spelling()).unwrap(),
            operation
        );
        let book = OpcodeBook::embedded().unwrap();
        let instruction = |command: &str, operand| Instruction {
            opcode: book.opcode_for(command).unwrap(),
            command: command.into(),
            operand,
        };
        let mut adapter = CompiledScript {
            name: Some("proc,select_banked_child".into()),
            args: Counts {
                int: ARGUMENT_COUNT,
                ..Counts::default()
            },
            locals: Counts {
                int: ARGUMENT_COUNT,
                ..Counts::default()
            },
            code: vec![
                instruction("push_int_local", Operand::Local(COMPONENT_ARGUMENT)),
                instruction(operation.command(), Operand::Byte(u8::from(true))),
                instruction("return", Operand::Byte(u8::default())),
            ],
        };
        let specification = |operation| Specification {
            resource: None,
            role: Role::Adapter,
            adapter_calls: BTreeMap::new(),
            host_operations: BTreeMap::from([(HOST_PC, operation)]),
        };
        let mut table = Table::default();
        let bytes = table
            .bind_import(
                SYNTHETIC_ADAPTER,
                &mut adapter,
                &book,
                specification(operation),
            )
            .unwrap();
        let metadata = table.group_bytes(SYNTHETIC_ADAPTER).unwrap();
        let accounting =
            decode_accounting(SYNTHETIC_ADAPTER, &bytes, Some(&metadata), &adapter).unwrap();
        let mut provider = Programs::default();
        provider.scripts.insert(SYNTHETIC_ADAPTER, adapter.clone());
        provider.accounting.insert(SYNTHETIC_ADAPTER, accounting);
        let mut engine = Engine::default();
        for (bank, slot, expected) in [
            (NESTED_BANK, PRESENT_SLOT, true),
            (NESTED_BANK, MISSING_SLOT, false),
        ] {
            let mut session = Session::for_script(
                SYNTHETIC_ADAPTER,
                &adapter,
                &[Value::Int(packed), Value::Int(bank), Value::Int(slot)],
                None,
            )
            .unwrap();
            {
                let mut host = ScriptHost {
                    engine: &mut engine,
                    store,
                    active,
                    properties: None,
                };
                while !Vm::new(&mut host, &provider).step(&mut session).unwrap() {}
            }
            assert_eq!(session.snapshot().ints, [i32::from(expected)]);
            assert!(Rc::ptr_eq(
                active[SECONDARY].component.as_ref().unwrap(),
                &expected_nested
            ));
            assert!(Rc::ptr_eq(
                active[SECONDARY].interface.as_ref().unwrap(),
                saved[PRIMARY].interface.as_ref().unwrap()
            ));
            assert!(Rc::ptr_eq(
                active[PRIMARY].component.as_ref().unwrap(),
                saved[PRIMARY].component.as_ref().unwrap()
            ));
        }
        let (_, capacity) = banks.range(NESTED_BANK).unwrap();
        for (bank, slot) in [
            (INVALID_ARGUMENT, PRESENT_SLOT),
            (NESTED_BANK, INVALID_ARGUMENT),
            (NESTED_BANK, i32::from(capacity)),
        ] {
            assert!(store
                .find_runtime_child(&mut active[SECONDARY], packed, bank, slot, banks)
                .is_err());
            assert!(Rc::ptr_eq(
                active[SECONDARY].component.as_ref().unwrap(),
                &expected_nested
            ));
            assert!(Rc::ptr_eq(
                active[SECONDARY].interface.as_ref().unwrap(),
                saved[PRIMARY].interface.as_ref().unwrap()
            ));
        }
        assert!(store
            .find_runtime_child(
                &mut active[SECONDARY],
                INVALID_ARGUMENT,
                ROOT_BANK,
                PRESENT_SLOT,
                banks
            )
            .is_err());
        assert_eq!(store.updated.len(), dirty_before);
        assert!(Rc::ptr_eq(
            active[SECONDARY].component.as_ref().unwrap(),
            &expected_nested
        ));
        let invalid = HostOperation::FindRuntimeChild {
            banks,
            bank_argument: BANK_ARGUMENT,
            slot_argument: ARGUMENT_COUNT,
        };
        let mut invalid_adapter = adapter.clone();
        assert!(Table::default()
            .bind_import(
                SYNTHETIC_ADAPTER,
                &mut invalid_adapter,
                &book,
                specification(invalid)
            )
            .is_err());
        let kind = root.borrow().f.r#type;
        const TEXT_KIND: i32 = 4; // not a content id: component type tag.
        root.borrow_mut().f.r#type = TEXT_KIND;
        assert!(store
            .find_runtime_child(
                &mut active[SECONDARY],
                packed,
                ROOT_BANK,
                PRESENT_SLOT,
                banks
            )
            .is_err());
        root.borrow_mut().f.r#type = kind;
        assert!(Rc::ptr_eq(
            active[SECONDARY].component.as_ref().unwrap(),
            &expected_nested
        ));
        // Packed selection shares the carrier command but has a different
        // failure contract: a missing component clears both selected refs.
        const COMPONENT_ARGUMENT_COUNT: u16 = 1;
        const GROUP_SHIFT: u32 = 16;
        const GROUP_WITH_HIGH_BIT: i32 = 0x8000; // synthetic address namespace.
        const ROOT_FILE: i32 = 0;
        const MISSING_FILE: i32 = u16::MAX as i32;
        const ABSENT_COMPONENT: i32 = -1;
        let operation = HostOperation::FindComponent;
        assert_eq!(
            HostOperation::parse(&operation.spelling()).unwrap(),
            operation
        );
        let mut adapter = CompiledScript {
            name: Some("proc,select_packed_component".into()),
            args: Counts {
                int: COMPONENT_ARGUMENT_COUNT,
                ..Counts::default()
            },
            locals: Counts {
                int: COMPONENT_ARGUMENT_COUNT,
                ..Counts::default()
            },
            code: adapter.code.clone(),
        };
        let mut table = Table::default();
        let bytes = table
            .bind_import(
                SYNTHETIC_ADAPTER,
                &mut adapter,
                &book,
                specification(operation),
            )
            .unwrap();
        let metadata = table.group_bytes(SYNTHETIC_ADAPTER).unwrap();
        let accounting =
            decode_accounting(SYNTHETIC_ADAPTER, &bytes, Some(&metadata), &adapter).unwrap();
        provider.scripts.insert(SYNTHETIC_ADAPTER, adapter.clone());
        provider.accounting.insert(SYNTHETIC_ADAPTER, accounting);
        let high_address = (GROUP_WITH_HIGH_BIT << GROUP_SHIFT) | ROOT_FILE;
        let mut high_root = Component::default();
        high_root.f.parentlayer = high_address;
        let high_root = Rc::new(RefCell::new(high_root));
        let high_interface = Interface::new(vec![Some(high_root.clone())]);
        store
            .interfaces
            .insert(GROUP_WITH_HIGH_BIT, high_interface.clone());
        let missing_address = (packed & !MISSING_FILE) | MISSING_FILE;
        for (packed, expected) in [
            (
                packed,
                Some((root, saved[PRIMARY].interface.as_ref().unwrap())),
            ),
            (missing_address, None),
            (
                packed,
                Some((root, saved[PRIMARY].interface.as_ref().unwrap())),
            ),
            (ABSENT_COMPONENT, None),
            (high_address, Some((&high_root, &high_interface))),
        ] {
            let mut session =
                Session::for_script(SYNTHETIC_ADAPTER, &adapter, &[Value::Int(packed)], None)
                    .unwrap();
            {
                let mut host = ScriptHost {
                    engine: &mut engine,
                    store,
                    active,
                    properties: None,
                };
                while !Vm::new(&mut host, &provider).step(&mut session).unwrap() {}
            }
            assert_eq!(session.snapshot().ints, [i32::from(expected.is_some())]);
            if let Some((component, interface)) = expected {
                assert!(Rc::ptr_eq(
                    active[SECONDARY].component.as_ref().unwrap(),
                    component
                ));
                assert!(Rc::ptr_eq(
                    active[SECONDARY].interface.as_ref().unwrap(),
                    interface
                ));
            } else {
                assert!(active[SECONDARY].component.is_none());
                assert!(active[SECONDARY].interface.is_none());
            }
            assert!(Rc::ptr_eq(
                active[PRIMARY].component.as_ref().unwrap(),
                saved[PRIMARY].component.as_ref().unwrap()
            ));
            assert!(Rc::ptr_eq(
                active[PRIMARY].interface.as_ref().unwrap(),
                saved[PRIMARY].interface.as_ref().unwrap()
            ));
        }
        store.interfaces.remove(&GROUP_WITH_HIGH_BIT);
        assert_eq!(store.updated.len(), dirty_before);
        *active = saved;
    }

    fn verify_recorded_flat_child_selection() {
        use native910::{
            execution::{decode_accounting, HostOperation, Role, Specification, Table},
            opcode::OpcodeBook,
            vm::{Programs, Session},
        };
        const PRIMARY: usize = 0;
        const SECONDARY: usize = 1;
        const PARENT_ARGUMENT: i32 = 0;
        const CHILD_ARGUMENT: i32 = 1;
        const ARGUMENTS: u16 = 2;
        const HOST_PC: usize = 1;
        const ADAPTER_CALL: usize = 3;
        const SYNTHETIC_SOURCE: i32 = 0; // isolated provider identities, not content ids.
        const SYNTHETIC_ADAPTER: i32 = 1; // not a content id: isolated VM provider.
        const NONCONTAINER_KIND: i32 = 4;
        const GROUP_SHIFT: u32 = 16;
        const FILE_MASK: i32 = 0xffff;
        const STATIC_CHILD: i32 = -1;
        const FIRST_NESTED_BANK: u64 = 1;
        let recording: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/flat-child.json")).unwrap();
        let revision: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../revisions/950/cs2/950-1-to-910.json"
        ))
        .unwrap();
        let contract = revision["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|rule| rule["operations"]["cc_find"]["host_operation"].as_str())
            .unwrap();
        let operation = HostOperation::parse(contract).unwrap();
        let HostOperation::FindFlatChild { limit, .. } = operation else {
            unreachable!()
        };
        assert_eq!(u64::from(limit), recording["child_limit"].as_u64().unwrap());
        let native = OpcodeBook::embedded().unwrap();
        let instruction = |command: &str, operand| Instruction {
            opcode: native.opcode_for(command).unwrap(),
            command: command.into(),
            operand,
        };
        let args = Counts {
            int: ARGUMENTS,
            ..Counts::default()
        };
        for case in recording["cases"].as_array().unwrap() {
            let input = &case["input"];
            let result = &case["result"];
            let default_parent =
                i32::try_from(recording["parent_component"].as_i64().unwrap()).unwrap();
            let packed = input["packed"]
                .as_i64()
                .unwrap_or(i64::from(default_parent));
            let packed = i32::try_from(packed).unwrap();
            // The provider's cache address is data from the recording. The
            // sentinel query still supplies the ordinary parent in its cache.
            let address = if packed == STATIC_CHILD {
                default_parent
            } else {
                packed
            };
            let group = ((address as u32) >> GROUP_SHIFT) as i32;
            let file = (address & FILE_MASK) as usize;
            let mut root = Component::default();
            root.f.parentlayer = address;
            if input["container"] == false {
                root.f.r#type = NONCONTAINER_KIND;
            }
            let mut labels = BTreeMap::new();
            let child = |id: i32| {
                let mut value = Component::default();
                value.f.parentlayer = address;
                value.f.id = id;
                Rc::new(RefCell::new(value))
            };
            let direct = input["static"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    let id = row[0].as_i64().unwrap();
                    let component = child(if id == i64::from(u16::MAX) {
                        STATIC_CHILD
                    } else {
                        i32::try_from(id).unwrap()
                    });
                    labels.insert(row[1].as_str().unwrap().to_owned(), component.clone());
                    Some(component)
                })
                .collect();
            root.children = Some(Rc::new(RefCell::new(direct)));
            let root_slots = recording["root_slots"].as_u64().unwrap();
            let bank_slots = recording["bank_slots"].as_u64().unwrap();
            let mut index = BTreeMap::new();
            for (bank, entries) in input["banks"].as_object().unwrap() {
                let bank: u64 = bank.parse().unwrap();
                for row in entries.as_array().unwrap() {
                    let slot = row[0].as_u64().unwrap();
                    let encoded = u16::try_from(if bank == u64::default() {
                        slot
                    } else {
                        root_slots + (bank - FIRST_NESTED_BANK) * bank_slots + slot
                    })
                    .unwrap();
                    let component = child(i32::from(encoded));
                    labels.insert(row[1].as_str().unwrap().to_owned(), component.clone());
                    index.insert(super::RuntimeChildId::new(encoded).unwrap(), component);
                }
            }
            root.runtime_lookup = Some(index);
            let root = Rc::new(RefCell::new(root));
            let mut components = vec![None; file + usize::from(true)];
            if input["parent"] != false {
                components[file] = Some(root);
            }
            let interface = Interface::new(components);
            let previous = child(i32::default());
            let previous_interface = Interface::new(Vec::new());
            let mut active = std::array::from_fn(|_| Active {
                component: Some(previous.clone()),
                interface: Some(previous_interface.clone()),
            });
            let mut store = Store::default();
            store.interfaces.insert(group, interface.clone());
            let mut adapter = CompiledScript {
                name: Some("proc,recorded_flat_child".into()),
                args,
                locals: args,
                code: vec![
                    instruction("push_int_local", Operand::Local(PARENT_ARGUMENT)),
                    instruction(operation.command(), Operand::Byte(u8::from(true))),
                    instruction("return", Operand::Byte(u8::default())),
                ],
            };
            let prefix = i32::try_from(
                result["ints"]
                    .as_array()
                    .unwrap()
                    .first()
                    .unwrap()
                    .as_i64()
                    .unwrap(),
            )
            .unwrap();
            let mut source = CompiledScript {
                name: Some("recorded_flat_child_caller".into()),
                args,
                locals: args,
                code: vec![
                    instruction("push_constant_string", Operand::Int(prefix)),
                    instruction("push_int_local", Operand::Local(PARENT_ARGUMENT)),
                    instruction("push_int_local", Operand::Local(CHILD_ARGUMENT)),
                    instruction("gosub_with_params", Operand::Script(SYNTHETIC_ADAPTER)),
                    instruction("return", Operand::Byte(u8::default())),
                ],
            };
            let mut table = Table::default();
            let adapter_bytes = table
                .bind_import(
                    SYNTHETIC_ADAPTER,
                    &mut adapter,
                    &native,
                    Specification {
                        role: Role::Adapter,
                        resource: None,
                        adapter_calls: BTreeMap::new(),
                        host_operations: BTreeMap::from([(HOST_PC, operation)]),
                    },
                )
                .unwrap();
            let source_bytes = table
                .bind_import(
                    SYNTHETIC_SOURCE,
                    &mut source,
                    &native,
                    Specification {
                        role: Role::Source,
                        resource: None,
                        adapter_calls: BTreeMap::from([(ADAPTER_CALL, SYNTHETIC_ADAPTER)]),
                        host_operations: BTreeMap::new(),
                    },
                )
                .unwrap();
            let adapter_metadata = table.group_bytes(SYNTHETIC_ADAPTER).unwrap();
            let source_metadata = table.group_bytes(SYNTHETIC_SOURCE).unwrap();
            let mut provider = Programs::default();
            provider.accounting.insert(
                SYNTHETIC_ADAPTER,
                decode_accounting(
                    SYNTHETIC_ADAPTER,
                    &adapter_bytes,
                    Some(&adapter_metadata),
                    &adapter,
                )
                .unwrap(),
            );
            provider.accounting.insert(
                SYNTHETIC_SOURCE,
                decode_accounting(
                    SYNTHETIC_SOURCE,
                    &source_bytes,
                    Some(&source_metadata),
                    &source,
                )
                .unwrap(),
            );
            provider.scripts.insert(SYNTHETIC_ADAPTER, adapter);
            provider.scripts.insert(SYNTHETIC_SOURCE, source.clone());
            let inputs = [
                Value::Int(packed),
                Value::Int(i32::try_from(input["child"].as_i64().unwrap()).unwrap()),
            ];
            let mut session =
                Session::for_script(SYNTHETIC_SOURCE, &source, &inputs, None).unwrap();
            let mut engine = Engine::default();
            {
                let mut host = ScriptHost {
                    engine: &mut engine,
                    store: &mut store,
                    active: &mut active,
                    properties: None,
                };
                while !Vm::new(&mut host, &provider).step(&mut session).unwrap() {}
            }
            assert_eq!(
                serde_json::to_value(session.snapshot().ints).unwrap(),
                result["ints"],
                "{}",
                case["name"]
            );
            assert!(Rc::ptr_eq(
                active[PRIMARY].component.as_ref().unwrap(),
                &previous
            ));
            assert!(Rc::ptr_eq(
                active[PRIMARY].interface.as_ref().unwrap(),
                &previous_interface
            ));
            let expected = result["component"].as_str().unwrap();
            match expected {
                "absent" => assert!(active[SECONDARY].component.is_none()),
                "previous" => assert!(Rc::ptr_eq(
                    active[SECONDARY].component.as_ref().unwrap(),
                    &previous
                )),
                label => assert!(Rc::ptr_eq(
                    active[SECONDARY].component.as_ref().unwrap(),
                    &labels[label]
                )),
            }
            match result["interface"].as_str().unwrap() {
                "absent" => assert!(active[SECONDARY].interface.is_none()),
                "previous" => assert!(Rc::ptr_eq(
                    active[SECONDARY].interface.as_ref().unwrap(),
                    &previous_interface
                )),
                _ => assert!(Rc::ptr_eq(
                    active[SECONDARY].interface.as_ref().unwrap(),
                    &interface
                )),
            }
            assert!(store.updated.is_empty());
        }
    }

    fn verify_runtime_child_slot_query() {
        use native910::execution::{HostOperation, RuntimeChildBanks};
        const ROOT_BANK: i32 = 0;
        const FIRST_OTHER_BANK: i32 = 1;
        const NESTED_BANK: i32 = 2;
        const FIRST_SLOT: i32 = 0;
        const INVALID_BANK: i32 = -1;
        const CONTAINER_KIND: i32 = 0;
        const TEXT_KIND: i32 = 4; // not a content id: component type tag.
        let source: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../revisions/950/cs2/950-1-to-910.json"
        ))
        .unwrap();
        let query = source["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|rule| {
                rule["operations"]["if_getnextcategorysubid"]["host_operation"].as_str()
            })
            .unwrap();
        let operation = HostOperation::parse(query).unwrap();
        assert_eq!(operation.spelling(), query);
        let HostOperation::NextRuntimeChildSlot { banks, .. } = operation else {
            unreachable!()
        };
        assert!(RuntimeChildBanks::new(u16::MAX, u16::MAX, u16::MAX).is_err());
        let (_, capacity) = banks.range(FIRST_OTHER_BANK).unwrap();
        let maximum_bank = i32::from(banks.bank_count()) - 1;
        for bank in [ROOT_BANK, FIRST_OTHER_BANK, maximum_bank] {
            let (_, width) = banks.range(bank).unwrap();
            for slot in [FIRST_SLOT, i32::from(width) - 1] {
                assert_eq!(
                    banks.decode(banks.encode(bank, slot).unwrap()),
                    Some((bank as u16, slot as u16))
                );
            }
            assert!(banks.encode(bank, i32::from(width)).is_err());
            assert!(banks.encode(bank, -1).is_err());
        }
        assert!(banks.range(maximum_bank + 1).is_err());
        assert!(banks.range(INVALID_BANK).is_err());
        assert!(banks.decode(u16::MAX).is_none());
        let component = |kind| {
            let mut component = Component::default();
            component.f.r#type = kind;
            component
        };
        let key =
            |bank, slot| super::RuntimeChildId::new(banks.encode(bank, slot).unwrap()).unwrap();
        let (mut store, mut active) = static_active();
        let root = active[0].component.clone().unwrap();
        let interface = active[0].interface.clone().unwrap();
        assert_eq!(
            store
                .next_runtime_child_slot(&root, FIRST_OTHER_BANK, banks)
                .unwrap(),
            FIRST_SLOT
        );
        for slot in FIRST_SLOT..i32::from(capacity) {
            store
                .attach_runtime_child(
                    &mut active[0],
                    &interface,
                    &root,
                    component(TEXT_KIND),
                    key(FIRST_OTHER_BANK, slot),
                )
                .unwrap();
        }
        let selected = active[0].component.clone().unwrap();
        assert!(store
            .next_runtime_child_slot(&root, FIRST_OTHER_BANK, banks)
            .is_err());
        assert!(root.borrow().runtime_lookup.is_none());
        assert!(Rc::ptr_eq(active[0].component.as_ref().unwrap(), &selected));
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &root,
                component(CONTAINER_KIND),
                key(ROOT_BANK, FIRST_SLOT),
            )
            .unwrap();
        let container = active[0].component.clone().unwrap();
        assert!(store
            .next_runtime_child_slot(&container, FIRST_OTHER_BANK, banks)
            .is_err());
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &container,
                component(TEXT_KIND),
                key(NESTED_BANK, FIRST_SLOT),
            )
            .unwrap();
        assert!(root.borrow().runtime_lookup.is_some());
        assert_eq!(
            store
                .next_runtime_child_slot(&root, FIRST_OTHER_BANK, banks)
                .unwrap(),
            i32::from(capacity)
        );
        assert_eq!(
            store
                .next_runtime_child_slot(&container, FIRST_OTHER_BANK, banks)
                .unwrap(),
            i32::from(capacity)
        );
        assert_eq!(
            store
                .next_runtime_child_slot(&container, NESTED_BANK, banks)
                .unwrap(),
            FIRST_SLOT + 1
        );
        const GAP_DIVISOR: u16 = 2;
        let gap = i32::from(capacity / GAP_DIVISOR);
        assert!(store.remove_runtime_child(&root, key(FIRST_OTHER_BANK, gap)));
        assert_eq!(
            store
                .next_runtime_child_slot(&root, FIRST_OTHER_BANK, banks)
                .unwrap(),
            gap
        );
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &root,
                component(TEXT_KIND),
                key(FIRST_OTHER_BANK, gap),
            )
            .unwrap();
        assert_eq!(
            store
                .next_runtime_child_slot(&root, FIRST_OTHER_BANK, banks)
                .unwrap(),
            i32::from(capacity)
        );
        let packed = root.borrow().f.parentlayer;
        assert!(store
            .packed_next_runtime_child_slot(-1, ROOT_BANK, banks)
            .is_err());
        assert!(store
            .packed_next_runtime_child_slot(packed, INVALID_BANK, banks)
            .is_err());
        assert!(store
            .next_runtime_child_slot(&selected, ROOT_BANK, banks)
            .is_err());
        let operand = Operand::Byte(u8::default());
        let arguments = [FIRST_OTHER_BANK, packed];
        let context = native910::vm::InstructionContext {
            script_name: None,
            script_id: None,
            event: None,
            pc: usize::default(),
            command: operation.command(),
            operand: &operand,
            secondary: true,
            int_locals: &arguments,
        };
        let selected = active[0].component.clone().unwrap();
        let dirty_before = store.updated.len();
        let mut ints = vec![packed];
        assert_eq!(
            super::component_operation(&mut store, &mut active, operation, &context, &mut ints)
                .unwrap(),
            Some(Value::Int(i32::from(capacity)))
        );
        assert!(ints.is_empty());
        assert_eq!(store.updated.len(), dirty_before);
        assert!(Rc::ptr_eq(active[0].component.as_ref().unwrap(), &selected));
        assert!(active[1].component.is_none());
        let missing_argument = native910::vm::InstructionContext {
            int_locals: &[],
            ..context
        };
        assert!(matches!(
            super::component_operation(
                &mut store,
                &mut active,
                operation,
                &missing_argument,
                &mut vec![packed]
            ),
            Err(VmError::TrapFailed { .. })
        ));
        verify_banked_child_selection(&mut store, &mut active, &root, banks);
        verify_recorded_flat_child_selection();
        store.clear_runtime_children(packed).unwrap();
        assert_eq!(
            store
                .next_runtime_child_slot(&container, FIRST_OTHER_BANK, banks)
                .unwrap(),
            FIRST_SLOT
        );
        assert!(root.borrow().runtime_lookup.is_some());
        drop(store);
        drop(active);
        drop(interface);
        drop(root);
        assert!(Store::default()
            .next_runtime_child_slot(&container, ROOT_BANK, banks)
            .is_err());
    }

    fn verify_recorded_text_child_creation() {
        use native910::{
            execution::{decode_accounting, HostOperation, Role, Specification, Table},
            opcode::OpcodeBook,
            vm::{Programs, Session},
        };
        const SOURCE: i32 = 0; // not a content id: isolated VM provider.
        const ADAPTER: i32 = 1; // not a content id: isolated VM provider.
        const PARENT_ARGUMENT: i32 = 0;
        const KIND_ARGUMENT: i32 = 1;
        const CHILD_ARGUMENT: i32 = 2;
        const ARGUMENTS: u16 = 3;
        const HOST_PC: usize = 1;
        const CALL_PC: usize = 4;
        const PRIMARY: usize = 0;
        const SECONDARY: usize = 1;
        const GROUP_SHIFT: u32 = 16;
        const FILE_MASK: i32 = 0xffff;
        const NONCONTAINER_KIND: i32 = 4;
        let recording: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/modern-text-child.json")).unwrap();
        let revision: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../revisions/950/cs2/950-1-to-910.json"
        ))
        .unwrap();
        let contract = revision["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|rule| rule["operations"]["cc_create"]["host_operation"].as_str())
            .unwrap();
        let mut operation = HostOperation::parse(contract).unwrap();
        let HostOperation::CreateFlatTextChild { limit, source, .. } = &mut operation else {
            unreachable!()
        };
        assert_eq!(
            u64::from(*limit),
            recording["child_limit"].as_u64().unwrap()
        );
        *source = Some((SOURCE, CALL_PC));
        let resource: Vec<u8> =
            serde_json::from_value(recording["template_resource"].clone()).unwrap();
        let template =
            rs910_config::ui_component_fields::TextChildTemplate::decode_resource(&resource)
                .unwrap();
        assert_eq!(template.encode_resource(), resource);
        let book = OpcodeBook::embedded().unwrap();
        let instruction = |command: &str, operand| Instruction {
            opcode: book.opcode_for(command).unwrap(),
            command: command.into(),
            operand,
        };
        let args = Counts {
            int: ARGUMENTS,
            ..Counts::default()
        };
        for case in recording["cases"].as_array().unwrap() {
            let input = &case["input"];
            let expected = &case["result"];
            let integer = |field: &str| i32::try_from(input[field].as_i64().unwrap()).unwrap();
            let packed = integer("packed");
            let group = ((packed as u32) >> GROUP_SHIFT) as i32;
            let file = (packed & FILE_MASK) as usize;
            let mut root = Component::default();
            root.f.parentlayer = packed;
            if input["container"] == false {
                root.f.r#type = NONCONTAINER_KIND;
            }
            let root = Rc::new(RefCell::new(root));
            let mut components = vec![None; file + usize::from(true)];
            if input["parent_present"] == true {
                components[file] = Some(root.clone());
            }
            let interface = Interface::new(components);
            let previous = Rc::new(RefCell::new(Component::default()));
            let previous_interface = Interface::new(Vec::new());
            let mut active = std::array::from_fn(|_| Active {
                component: Some(previous.clone()),
                interface: Some(previous_interface.clone()),
            });
            let selected = usize::from(integer("selector") != i32::default());
            let other = if selected == PRIMARY {
                SECONDARY
            } else {
                PRIMARY
            };
            let mut store = Store::default();
            if input["group_present"] == true {
                store.interfaces.insert(group, interface.clone());
            }
            let prefix = i32::try_from(expected["stack"][0].as_i64().unwrap()).unwrap();
            let mut caller = CompiledScript {
                name: Some("recorded_text_child_caller".into()),
                args,
                locals: args,
                code: vec![
                    instruction("push_constant_string", Operand::Int(prefix)),
                    instruction("push_int_local", Operand::Local(PARENT_ARGUMENT)),
                    instruction("push_int_local", Operand::Local(KIND_ARGUMENT)),
                    instruction("push_int_local", Operand::Local(CHILD_ARGUMENT)),
                    instruction("gosub_with_params", Operand::Script(ADAPTER)),
                    instruction("return", Operand::Byte(u8::default())),
                ],
            };
            let mut bridge = CompiledScript {
                name: Some("proc,recorded_text_child".into()),
                args,
                locals: args,
                code: vec![
                    instruction("push_int_local", Operand::Local(PARENT_ARGUMENT)),
                    instruction(
                        operation.command(),
                        Operand::Byte(u8::from(selected != PRIMARY)),
                    ),
                    instruction("return", Operand::Byte(u8::default())),
                ],
            };
            let mut table = Table::default();
            let bridge_bytes = table
                .bind_import(
                    ADAPTER,
                    &mut bridge,
                    &book,
                    Specification {
                        role: Role::Adapter,
                        resource: Some(resource.clone().into()),
                        adapter_calls: BTreeMap::new(),
                        host_operations: BTreeMap::from([(HOST_PC, operation)]),
                    },
                )
                .unwrap();
            let caller_bytes = table
                .bind_import(
                    SOURCE,
                    &mut caller,
                    &book,
                    Specification {
                        role: Role::Source,
                        resource: None,
                        adapter_calls: BTreeMap::from([(CALL_PC, ADAPTER)]),
                        host_operations: BTreeMap::new(),
                    },
                )
                .unwrap();
            let mut programs = Programs::default();
            for (id, bytes, script) in [
                (SOURCE, caller_bytes, caller.clone()),
                (ADAPTER, bridge_bytes, bridge),
            ] {
                programs.accounting.insert(
                    id,
                    decode_accounting(id, &bytes, table.group_bytes(id).as_deref(), &script)
                        .unwrap(),
                );
                programs.scripts.insert(id, script);
            }
            table
                .validate_links(
                    &programs
                        .scripts
                        .iter()
                        .map(|(id, script)| (*id, script.clone()))
                        .collect(),
                )
                .unwrap();
            let inputs = [
                Value::Int(packed),
                Value::Int(integer("kind")),
                Value::Int(integer("child_id")),
            ];
            let mut session =
                Session::for_script(SOURCE, &caller, &inputs, Some("recorded_creation".into()))
                    .unwrap();
            let mut engine = Engine::default();
            let result = {
                let mut host = ScriptHost {
                    engine: &mut engine,
                    store: &mut store,
                    active: &mut active,
                    properties: None,
                };
                loop {
                    match Vm::new(&mut host, &programs).step(&mut session) {
                        Ok(true) => break Ok(()),
                        Ok(false) => {}
                        Err(error) => break Err(error),
                    }
                }
            };
            assert_eq!(
                result.is_ok(),
                expected["success"].as_bool().unwrap(),
                "{}: {result:?}",
                case["name"]
            );
            if let Err(error) = result {
                assert!(matches!(error, VmError::TrapFailed { .. }));
            }
            assert_eq!(
                serde_json::to_value(session.snapshot().ints).unwrap(),
                expected["stack"],
                "{}",
                case["name"]
            );
            let changed = !Rc::ptr_eq(active[selected].component.as_ref().unwrap(), &previous);
            assert_eq!(changed, expected["selected"].as_bool().unwrap());
            assert!(Rc::ptr_eq(
                active[other].component.as_ref().unwrap(),
                &previous
            ));
            assert!(Rc::ptr_eq(
                active[other].interface.as_ref().unwrap(),
                &previous_interface
            ));
            assert_eq!(
                root.borrow()
                    .children
                    .as_ref()
                    .map_or(usize::default(), |children| children.borrow().len()),
                expected["children"].as_u64().unwrap() as usize
            );
            assert_eq!(
                root.borrow()
                    .sorted
                    .as_ref()
                    .map_or(usize::default(), |children| children.borrow().len()),
                expected["drawing"].as_u64().unwrap() as usize
            );
            if changed {
                let component = active[selected].component.as_ref().unwrap();
                let child = component.borrow();
                let identity = &expected["identity"];
                assert_eq!(
                    child.f.parentlayer,
                    (identity[0].as_i64().unwrap() as i32) << GROUP_SHIFT
                        | identity[1].as_i64().unwrap() as i32
                );
                assert_eq!(child.f.layer, child.f.parentlayer);
                assert_eq!(child.f.id, identity[2].as_i64().unwrap() as i32);
                assert!(Rc::ptr_eq(&child.runtime_parent().unwrap(), &root));
                assert!(Rc::ptr_eq(
                    active[selected].interface.as_ref().unwrap(),
                    &interface
                ));
                let origin = child.creation_origin.as_ref().unwrap();
                assert_eq!(
                    (origin.script, origin.instruction, origin.event.as_deref()),
                    (Some(SOURCE), CALL_PC, Some("recorded_creation"))
                );
                let actual = serde_json::json!({"colour":child.f.colour,"transparency":child.f.trans,"aspect":[child.f.aspectwidth,child.f.aspectheight],
                    "font":child.f.textfont,"empty":child.f.text.as_ref().is_some_and(Vec::is_empty),"line_height":child.f.textLineHeight,
                    "max_lines":child.f.maxlines,"horizontal_align":child.f.textHAlign,"vertical_align":child.f.textVAlign,
                    "flags":u8::from(child.f.fontmono || child.f.textshadow || child.f.textantimacro)});
                assert_eq!(actual, expected["fields"], "{}", case["name"]);
            } else {
                assert!(Rc::ptr_eq(
                    active[selected].interface.as_ref().unwrap(),
                    &previous_interface
                ));
            }
        }
    }

    fn verify_recorded_text_properties() {
        use native910::execution::{FontMapping, HostOperation, TextProperty};
        const VALUES_HEAD: usize = 0;
        const CONTAINER_TYPE: i32 = 0;
        let recording: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/modern-text-properties.json")).unwrap();
        let revision: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../revisions/950/cs2/950-1-to-910.json"
        ))
        .unwrap();
        let rules = revision["rules"].as_array().unwrap();
        let getter_recording: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/modern-font-identity.json")).unwrap();
        let metrics_recording: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/modern-font-reader-metrics.json"))
                .unwrap();
        for recording in [&recording, &getter_recording, &metrics_recording] {
            let text_type = recording["target_text_type"].as_i64().unwrap() as i32;
            let initial = recording["initial_scalars"].as_i64().unwrap() as i32;
            for case in recording["cases"].as_array().unwrap() {
                let input = &case["input"];
                let expected = &case["result"];
                let explicit = input["explicit"].as_bool().unwrap();
                let suffix = match input["property"].as_str().unwrap() {
                    "font" => "settextfont",
                    "read_font" => "getfontgraphic",
                    "read_font_metrics" => "getfontmetrics",
                    "alignment" => "settextalign",
                    "max_lines" => "setmaxlines",
                    _ => unreachable!(),
                };
                let command = format!("{}_{}", if explicit { "if" } else { "cc" }, suffix);
                let spelling = rules
                    .iter()
                    .find_map(|rule| rule["operations"][&command]["host_operation"].as_str())
                    .unwrap();
                let mut operation = HostOperation::parse(spelling).unwrap();
                let values: Vec<i32> = serde_json::from_value(input["values"].clone()).unwrap();
                if let HostOperation::ComponentText {
                    property: TextProperty::Font { mapping, .. },
                    ..
                } = &mut operation
                {
                    *mapping = native910::execution::FontBinding::Constant(FontMapping {
                        source: values[VALUES_HEAD],
                        target: values[VALUES_HEAD],
                    });
                }
                assert_eq!(
                    HostOperation::parse(&operation.spelling()).unwrap(),
                    operation
                );
                let packed = input["packed"].as_i64().unwrap() as i32;
                let mut component = Component::default();
                component.f.parentlayer = packed;
                component.f.id = input["child_id"].as_i64().unwrap() as i32;
                component.f.r#type = if input["runtime_class"] == recording["text_runtime_class"] {
                    text_type
                } else {
                    CONTAINER_TYPE
                };
                component.f.textfont = input["font"].as_i64().unwrap() as i32;
                if let HostOperation::ComponentText {
                    property:
                        TextProperty::ReadFont {
                            domain, initial, ..
                        },
                    ..
                } = &mut operation
                {
                    let mapping = FontMapping {
                        source: component.f.textfont,
                        target: component.f.textfont,
                    };
                    if component.f.id == -i32::from(true) {
                        *initial = Some(native910::execution::InitialFontMapping {
                            component: packed,
                            mapping,
                        });
                    } else {
                        component.set_source_font(*domain, mapping);
                    }
                    assert_eq!(
                        HostOperation::parse(&operation.spelling()).unwrap(),
                        operation
                    );
                }
                component.f.textHAlign = initial;
                component.f.textVAlign = initial;
                component.f.textLineHeight = initial;
                component.f.maxlines = initial;
                let component = Rc::new(RefCell::new(component));
                let file = usize::from(packed as u16);
                let mut files = vec![None; file + usize::from(true)];
                if input["present"] == true {
                    files[file] = Some(component.clone());
                }
                let interface = Interface::new(files);
                interface.borrow_mut().transient = input["transient"].as_bool().unwrap();
                let mut store = Store::default();
                store
                    .interfaces
                    .insert(((packed as u32) >> u16::BITS) as i32, interface.clone());
                let selected = usize::from(input["selector"].as_u64().unwrap() != u64::default());
                let mut active: [Active; 2] = std::array::from_fn(|_| Active::default());
                active[selected] = Active {
                    interface: Some(interface),
                    component: (input["present"] == true).then(|| component.clone()),
                };
                let prefix = expected["stack"][0].as_i64().unwrap() as i32;
                let mut ints = vec![prefix];
                ints.extend(&values);
                if explicit {
                    ints.push(packed);
                }
                let mut properties = crate::ui_properties::State::default();
                let mut changes = crate::ui_changes::Changes::default();
                let mut now = || i64::default();
                let operand = Operand::Byte(u8::from(selected != usize::default()));
                let context = native910::vm::InstructionContext {
                    script_name: Some("recorded_text_properties"),
                    script_id: None,
                    event: None,
                    pc: usize::default(),
                    command: operation.command(),
                    operand: &operand,
                    secondary: selected != usize::default(),
                    int_locals: &[],
                };
                let mut trap = |operation,
                                context: &native910::vm::InstructionContext<'_>,
                                ints: &mut Vec<i32>| {
                    native910::vm::Host::trap_operation_context(
                        &mut ScriptHost {
                            engine: &mut Engine::default(),
                            store: &mut store,
                            active: &mut active,
                            properties: Some(crate::ui_properties::Context {
                                state: &mut properties,
                                changes: &mut changes,
                                now: &mut now,
                                nested_count: i32::default(),
                            }),
                        },
                        operation,
                        context,
                        ints,
                        &mut Vec::new(),
                        &mut Vec::new(),
                    )
                };
                let result = if let Some(writes) = input["writes"].as_array() {
                    let mut reads = Vec::new();
                    let mut result = Ok(None);
                    for source in writes {
                        let mut setter = HostOperation::parse(
                            rules
                                .iter()
                                .find_map(|rule| {
                                    rule["operations"]["cc_settextfont"]["host_operation"].as_str()
                                })
                                .unwrap(),
                        )
                        .unwrap();
                        let source = source.as_i64().unwrap() as i32;
                        if let HostOperation::ComponentText {
                            property: TextProperty::Font { mapping, .. },
                            ..
                        } = &mut setter
                        {
                            *mapping = native910::execution::FontBinding::Constant(FontMapping {
                                source,
                                target: input["font"].as_i64().unwrap() as i32,
                            });
                        }
                        ints = vec![prefix, source];
                        let setter_context = native910::vm::InstructionContext {
                            command: setter.command(),
                            ..context
                        };
                        trap(setter, &setter_context, &mut ints).unwrap();
                        result = trap(operation, &context, &mut ints);
                        if let Ok(Some(native910::vm::Value::Int(font))) = result {
                            reads.push(font);
                        }
                    }
                    assert_eq!(
                        serde_json::to_value(reads).unwrap(),
                        expected["reads_after_writes"]
                    );
                    result
                } else {
                    trap(operation, &context, &mut ints)
                };
                if let Ok(Some(native910::vm::Value::Int(font))) = result {
                    ints.push(font);
                }
                assert_eq!(
                    result.is_ok(),
                    expected["success"].as_bool().unwrap(),
                    "{}: {result:?}",
                    case["name"]
                );
                assert_eq!(serde_json::to_value(ints).unwrap(), expected["stack"]);
                let component = component.borrow();
                assert_eq!(
                    serde_json::json!({
                        "font":component.f.textfont, "line_height":component.f.textLineHeight,
                        "max_lines":component.f.maxlines, "horizontal_align":component.f.textHAlign,
                        "vertical_align":component.f.textVAlign,
                    }),
                    serde_json::json!({
                        "font":expected["font"], "line_height":expected["line_height"], "max_lines":expected["max_lines"],
                        "horizontal_align":expected["horizontal_align"], "vertical_align":expected["vertical_align"],
                    }),
                    "{}",
                    case["name"]
                );
                let scheduled = expected["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|call| call[0] == "change");
                assert_eq!(
                    changes.cache.len(),
                    usize::from(scheduled.is_some()),
                    "{}",
                    case["name"]
                );
                if let Some(call) = scheduled {
                    let entry = changes.cache.values().next().unwrap();
                    assert_eq!(entry.kind(), call[1].as_i64().unwrap() as i32);
                    assert_eq!(entry.target(), call[2].as_u64().unwrap() as i64);
                }
                assert!(store.updated.is_empty());
            }
        }
        verify_font_projection_lifetimes(&getter_recording, rules);
        verify_dynamic_font_mappings(&getter_recording, rules);
    }

    fn verify_dynamic_font_mappings(recording: &serde_json::Value, rules: &[serde_json::Value]) {
        use native910::{
            execution::{
                decode_accounting, FontBinding, FontMapping, FontMappings, HostOperation, Role,
                Specification, Table, TextProperty,
            },
            opcode::OpcodeBook,
            vm::{Programs, Session},
        };
        const SOURCE: i32 = 0; // not a content id: isolated VM provider.
        const FONT_ARGUMENT: i32 = 0;
        const ARGUMENTS: u16 = 1;
        const CONTAINER_KIND: i32 = 0;
        let recorded = recording["cases"].as_array().unwrap();
        let round_trip = recorded
            .iter()
            .find(|case| case["input"]["writes"].is_array())
            .unwrap();
        let target = round_trip["input"]["font"].as_i64().unwrap() as i32;
        let sources: BTreeSet<i32> = recorded
            .iter()
            .map(|case| case["input"]["font"].as_i64().unwrap() as i32)
            .chain(
                round_trip["input"]["writes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_i64().unwrap() as i32),
            )
            .collect();
        let book = OpcodeBook::embedded().unwrap();
        let operation = |command: &str| {
            HostOperation::parse(
                rules
                    .iter()
                    .find_map(|rule| rule["operations"][command]["host_operation"].as_str())
                    .unwrap(),
            )
            .unwrap()
        };
        let HostOperation::ComponentText {
            property: TextProperty::ReadFont { domain, absent, .. },
            text_type,
            ..
        } = operation("cc_getfontgraphic")
        else {
            unreachable!()
        };
        let mappings = FontMappings::new(sources.iter().map(|source| FontMapping {
            source: *source,
            target: if *source == absent { absent } else { target },
        }))
        .unwrap();
        let resource = mappings.resource();
        assert_eq!(FontMappings::decode_resource(&resource).unwrap(), mappings);
        let duplicated = FontMapping {
            source: absent,
            target: absent,
        };
        assert!(FontMappings::new([duplicated, duplicated]).is_err());
        assert!(FontMappings::new([]).is_err());
        let mut noncanonical = resource.clone();
        noncanonical.push(b'\n');
        assert!(FontMappings::decode_resource(&noncanonical).is_err());
        for explicit in [false, true] {
            let (mut store, mut active) = static_active();
            let component = active[0].component.clone().unwrap();
            component.borrow_mut().f.r#type = text_type;
            let packed = component.borrow().f.parentlayer;
            let prefix = if explicit { "if" } else { "cc" };
            let mut setter = operation(&format!("{prefix}_settextfont"));
            if let HostOperation::ComponentText {
                property: TextProperty::Font { mapping, .. },
                ..
            } = &mut setter
            {
                *mapping = FontBinding::Resource;
            }
            assert!(setter.needs_resource());
            assert_eq!(HostOperation::parse(&setter.spelling()).unwrap(), setter);
            let instruction = |command: &str, operand| Instruction {
                opcode: book.opcode_for(command).unwrap(),
                command: command.into(),
                operand,
            };
            let mut code = vec![instruction("push_int_local", Operand::Local(FONT_ARGUMENT))];
            if explicit {
                code.push(instruction("push_constant_string", Operand::Int(packed)));
            }
            let setter_pc = code.len();
            code.push(instruction(setter.command(), Operand::Byte(u8::default())));
            code.push(instruction("return", Operand::Byte(u8::default())));
            let args = Counts {
                int: ARGUMENTS,
                ..Counts::default()
            };
            let mut script = CompiledScript {
                name: Some("proc,dynamic_font_map".into()),
                args,
                locals: args,
                code,
            };
            let specification = |resource| Specification {
                role: Role::Adapter,
                resource,
                adapter_calls: BTreeMap::new(),
                host_operations: BTreeMap::from([(setter_pc, setter)]),
            };
            assert!(Table::default()
                .bind_import(SOURCE, &mut script.clone(), &book, specification(None))
                .is_err());
            let mut table = Table::default();
            let bytes = table
                .bind_import(
                    SOURCE,
                    &mut script,
                    &book,
                    specification(Some(resource.clone().into())),
                )
                .unwrap();
            let accounting = decode_accounting(
                SOURCE,
                &bytes,
                table.group_bytes(SOURCE).as_deref(),
                &script,
            )
            .unwrap();
            let mut programs = Programs::default();
            programs.scripts.insert(SOURCE, script.clone());
            programs.accounting.insert(SOURCE, accounting);
            let mut state = crate::ui_properties::State::default();
            let mut changes = crate::ui_changes::Changes::default();
            let mut now = i64::default;
            let mut engine = Engine::default();
            let mut run = |font,
                           store: &mut Store,
                           active: &mut [Active; 2],
                           state: &mut crate::ui_properties::State,
                           changes: &mut crate::ui_changes::Changes| {
                let mut session =
                    Session::for_script(SOURCE, &script, &[Value::Int(font)], None).unwrap();
                let mut host = ScriptHost {
                    engine: &mut engine,
                    store,
                    active,
                    properties: Some(crate::ui_properties::Context {
                        state,
                        changes,
                        now: &mut now,
                        nested_count: i32::default(),
                    }),
                };
                loop {
                    match Vm::new(&mut host, &programs).step(&mut session) {
                        Ok(true) => return Ok(session.snapshot().ints),
                        Ok(false) => {}
                        Err(error) => return Err(error),
                    }
                }
            };
            for source in &sources {
                assert_eq!(
                    run(*source, &mut store, &mut active, &mut state, &mut changes).unwrap(),
                    Vec::<i32>::new()
                );
                assert_eq!(
                    component
                        .borrow_mut()
                        .source_font(domain, absent, None)
                        .unwrap(),
                    *source
                );
                assert_eq!(
                    component.borrow().f.textfont,
                    mappings.get(*source).unwrap().target
                );
            }
            assert_eq!(state.font_maps.len(), usize::from(true));
            let previous_font = component.borrow().f.textfont;
            let previous_identity = component.borrow().font_projection;
            let previous_changes = changes.clone();
            let previous_dirty = store.updated.len();
            let unmapped = sources.last().unwrap().wrapping_add(i32::from(true));
            assert!(mappings.get(unmapped).is_none());
            let error =
                run(unmapped, &mut store, &mut active, &mut state, &mut changes).unwrap_err();
            assert!(error.to_string().contains("no reviewed asset mapping"));
            assert_eq!(component.borrow().f.textfont, previous_font);
            assert_eq!(component.borrow().font_projection, previous_identity);
            assert_eq!(changes, previous_changes);
            assert_eq!(store.updated.len(), previous_dirty);
            component.borrow_mut().f.r#type = CONTAINER_KIND;
            assert_eq!(
                run(unmapped, &mut store, &mut active, &mut state, &mut changes).unwrap(),
                Vec::<i32>::new()
            );
            assert_eq!(component.borrow().f.textfont, previous_font);
            assert_eq!(component.borrow().font_projection, previous_identity);
            assert_eq!(changes, previous_changes);
            if !explicit {
                active[0].component = None;
                assert_eq!(
                    run(unmapped, &mut store, &mut active, &mut state, &mut changes).unwrap(),
                    Vec::<i32>::new()
                );
                assert_eq!(changes, previous_changes);
            }
            assert_eq!(
                component
                    .borrow_mut()
                    .source_font(domain, absent, None)
                    .unwrap(),
                *sources.last().unwrap()
            );
        }
    }

    fn verify_font_projection_lifetimes(
        recording: &serde_json::Value,
        rules: &[serde_json::Value],
    ) {
        use native910::execution::{FontMapping, HostOperation, TextProperty};
        let recorded = recording["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["input"]["writes"].is_array())
            .unwrap();
        let source = recorded["input"]["writes"][0].as_i64().unwrap() as i32;
        let target = recorded["input"]["font"].as_i64().unwrap() as i32;
        let mapping = FontMapping { source, target };
        let HostOperation::ComponentText {
            property: TextProperty::ReadFont { domain, absent, .. },
            text_type,
            ..
        } = HostOperation::parse(
            rules
                .iter()
                .find_map(|rule| rule["operations"]["cc_getfontgraphic"]["host_operation"].as_str())
                .unwrap(),
        )
        .unwrap()
        else {
            unreachable!()
        };
        let foreign = native910::execution::ContentDomain::parse(
            &domain.spelling().chars().rev().collect::<String>(),
        )
        .unwrap();
        let (mut store, mut active) = static_active();
        let component = active[0].component.clone().unwrap();
        let initial = native910::execution::InitialFontMapping {
            component: component.borrow().f.parentlayer,
            mapping,
        };
        {
            let mut fresh = Component::default();
            fresh.f.parentlayer = initial.component;
            fresh.f.textfont = target;
            let mismatch = native910::execution::InitialFontMapping {
                component: initial.component.wrapping_add(i32::from(true)),
                ..initial
            };
            assert!(fresh.source_font(domain, absent, Some(mismatch)).is_err());
            assert_eq!(
                fresh.source_font(domain, absent, Some(initial)).unwrap(),
                source
            );
            assert_eq!(fresh.f.textfont, target);
            fresh.set_target_font(target);
            assert!(fresh.source_font(domain, absent, Some(initial)).is_err());
            fresh.set_source_font(foreign, mapping);
            assert!(fresh.source_font(domain, absent, Some(initial)).is_err());
        }
        {
            let mut component = component.borrow_mut();
            component.f.r#type = text_type;
            component.set_target_font(absent);
            assert_eq!(component.source_font(domain, absent, None).unwrap(), absent);
            component.set_target_font(target);
            assert!(component.source_font(domain, absent, None).is_err());
            component.set_source_font(domain, mapping);
            assert_eq!(component.source_font(domain, absent, None).unwrap(), source);
            assert!(component.source_font(foreign, absent, None).is_err());
            let mut replacement = Component::default();
            assert_eq!(
                replacement.source_font(domain, absent, None).unwrap(),
                absent
            );
            assert_eq!(
                component.clone().source_font(domain, absent, None).unwrap(),
                source
            );
        }
        let operand = Operand::Byte(u8::default());
        let context = native910::vm::InstructionContext {
            script_name: Some("font_target_write"),
            script_id: None,
            event: None,
            pc: usize::default(),
            command: "cc_settextfont",
            operand: &operand,
            secondary: false,
            int_locals: &[],
        };
        let mut state = crate::ui_properties::State::default();
        let mut changes = crate::ui_changes::Changes::default();
        let mut now = i64::default;
        native910::vm::Host::trap_context(
            &mut ScriptHost {
                engine: &mut Engine::default(),
                store: &mut store,
                active: &mut active,
                properties: Some(crate::ui_properties::Context {
                    state: &mut state,
                    changes: &mut changes,
                    now: &mut now,
                    nested_count: i32::default(),
                }),
            },
            &context,
            &mut vec![target],
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(component
            .borrow_mut()
            .source_font(domain, absent, Some(initial))
            .is_err());
        component.borrow_mut().set_source_font(domain, mapping);
        let change_kind = if let HostOperation::ComponentText {
            property: TextProperty::Font { change_kind, .. },
            ..
        } = HostOperation::parse(
            rules
                .iter()
                .find_map(|rule| rule["operations"]["cc_settextfont"]["host_operation"].as_str())
                .unwrap(),
        )
        .unwrap()
        {
            change_kind
        } else {
            unreachable!()
        };
        let change = changes.push_server(
            i32::from(change_kind),
            i64::from(component.borrow().f.parentlayer as u32),
        );
        change.ints[0] = target;
        crate::ui_lifecycle::apply_change(&mut store, &mut state, change).unwrap();
        assert!(component
            .borrow_mut()
            .source_font(domain, absent, Some(initial))
            .is_err());
        component.borrow_mut().set_source_font(foreign, mapping);
        assert!(component
            .borrow_mut()
            .source_font(domain, absent, Some(initial))
            .is_err());
        assert_eq!(
            component
                .borrow_mut()
                .source_font(foreign, absent, None)
                .unwrap(),
            source
        );
    }

    fn verify_recorded_runtime_child_order() {
        use super::RuntimeChildId;
        const TEXT_KIND: i32 = 4; // not a content id: component type tag.
        let recording: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/runtime-child-order.json")).unwrap();
        for case in recording["cases"].as_array().unwrap() {
            let (mut store, mut active) = static_active();
            let root = active[0].component.clone().unwrap();
            let interface = active[0].interface.clone().unwrap();
            let mut retained = Vec::new();
            let input = &case["input"];
            let operations = input["operations"].as_array().unwrap();
            for (step, expected) in case["snapshots"].as_array().unwrap().iter().enumerate() {
                let (command, value) = if step == usize::default() {
                    ("create", input["child_id"].as_i64().unwrap())
                } else {
                    let operation = &operations[step - usize::from(true)];
                    (
                        operation[0].as_str().unwrap(),
                        operation[1].as_i64().unwrap(),
                    )
                };
                match command {
                    "create" => {
                        let mut child = Component::default();
                        child.f.r#type = TEXT_KIND;
                        store
                            .attach_runtime_child(
                                &mut active[0],
                                &interface,
                                &root,
                                child,
                                RuntimeChildId::new(u16::try_from(value).unwrap()).unwrap(),
                            )
                            .unwrap();
                        retained.push(active[0].component.clone().unwrap());
                    }
                    "start" | "end" => {
                        Store::reorder(
                            &interface,
                            Some(&retained[usize::try_from(value).unwrap()]),
                            command == "end",
                        )
                        .unwrap();
                    }
                    "empty_drawing" => root.borrow().sorted.as_ref().unwrap().borrow_mut().clear(),
                    _ => unreachable!(),
                }
                let labels = |array: Option<Array>| -> Vec<usize> {
                    array.map_or_else(Vec::new, |array| {
                        array
                            .borrow()
                            .iter()
                            .flatten()
                            .map(|child| {
                                retained
                                    .iter()
                                    .position(|label| Rc::ptr_eq(child, label))
                                    .unwrap()
                            })
                            .collect()
                    })
                };
                let component = root.borrow();
                let actual = serde_json::json!({
                    "ordered": labels(component.children.clone()),
                    "drawing": labels(component.sorted.clone()),
                    "effective": labels(component.child_drawing_order()),
                    "child_ids": retained.iter().map(|child| child.borrow().f.id).collect::<Vec<_>>(),
                });
                assert_eq!(actual, *expected, "{} at step {step}", case["name"]);
            }
        }
    }

    fn verify_owner_child_tree() {
        use super::RuntimeChildId;
        const CONTAINER_KIND: i32 = 0;
        const TEXT_KIND: i32 = 4; // not a content id: component type tag.
        const LOW_KEY: u16 = 2; // not a content id: synthetic runtime identity.
        const HIGH_KEY: u16 = 200; // not a content id: synthetic runtime identity.
        const NESTED_KEY: u16 = 4097; // not a content id: synthetic runtime identity.
        let key = |encoded| RuntimeChildId::new(encoded).unwrap();
        let component = |kind| {
            let mut child = Component::default();
            child.f.r#type = kind;
            child
        };
        let (mut store, mut active) = static_active();
        let root = active[0].component.clone().unwrap();
        let interface = active[0].interface.clone().unwrap();
        assert!(root.borrow().runtime_lookup.is_none());
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &root,
                component(CONTAINER_KIND),
                key(HIGH_KEY),
            )
            .unwrap();
        let container = active[0].component.clone().unwrap();
        assert!(root.borrow().sorted.is_none());
        Store::reorder(&interface, Some(&container), true).unwrap();
        let mut initial_leaf = component(TEXT_KIND);
        initial_leaf.f.hide = true;
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &root,
                initial_leaf,
                key(LOW_KEY),
            )
            .unwrap();
        let leaf = active[0].component.clone().unwrap();
        assert_eq!(leaf.borrow().runtime_entry_hidden(), Some(true));
        store.set_runtime_child_hidden(&leaf, false).unwrap();
        assert_eq!(leaf.borrow().runtime_entry_hidden(), Some(false));
        assert!(leaf.borrow().f.hide);
        let ordered = root.borrow().children.clone().unwrap();
        let drawing = root.borrow().sorted.clone().unwrap();
        assert_eq!(
            ordered
                .borrow()
                .iter()
                .flatten()
                .map(|child| child.borrow().f.id)
                .collect::<Vec<_>>(),
            [i32::from(LOW_KEY), i32::from(HIGH_KEY)]
        );
        assert!(Rc::ptr_eq(
            drawing.borrow().first().unwrap().as_ref().unwrap(),
            &container
        ));
        assert!(root.borrow().runtime_lookup.is_none());
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &container,
                component(TEXT_KIND),
                key(NESTED_KEY),
            )
            .unwrap();
        let grandchild = active[0].component.clone().unwrap();
        let grandchildren = container.borrow().children.clone().unwrap();
        assert!(root.borrow().runtime_lookup.is_some());
        for (id, child) in [
            (LOW_KEY, &leaf),
            (HIGH_KEY, &container),
            (NESTED_KEY, &grandchild),
        ] {
            assert!(Rc::ptr_eq(
                &root.borrow().runtime_child(key(id)).unwrap(),
                child
            ));
            assert!(crate::ui_hooks::attached(&mut store, child).unwrap());
            let packed = root.borrow().f.parentlayer;
            assert!(Rc::ptr_eq(
                &store.get(packed, i32::from(id)).unwrap().unwrap(),
                child
            ));
        }
        let packed = root.borrow().f.parentlayer;
        assert!(store
            .find(&mut active[1], packed, i32::from(NESTED_KEY), true)
            .unwrap());
        assert!(Rc::ptr_eq(
            active[1].component.as_ref().unwrap(),
            &grandchild
        ));
        // Entry visibility changes without rewriting the constructor flag.
        store.set_runtime_child_hidden(&grandchild, true).unwrap();
        assert_eq!(grandchild.borrow().runtime_entry_hidden(), Some(true));
        assert!(!grandchild.borrow().f.hide);
        // A target leaf chooses its immediate container, including nesting.
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &grandchild,
                component(TEXT_KIND),
                key(NESTED_KEY),
            )
            .unwrap();
        let replacement_leaf = active[0].component.clone().unwrap();
        assert_eq!(
            replacement_leaf.borrow().runtime_entry_hidden(),
            Some(false)
        );
        assert_eq!(grandchild.borrow().runtime_entry_hidden(), Some(false));
        // The production property host addresses the replacement through a
        // retained old target. Only exactly one hides it; no selection changes.
        {
            let mut retained = [active[1].clone(), Active::default()];
            let mut engine = Engine::default();
            let mut state = crate::ui_properties::State::default();
            let mut changes = crate::ui_changes::Changes::default();
            let mut now = || i64::default();
            let mut host = ScriptHost {
                engine: &mut engine,
                store: &mut store,
                active: &mut retained,
                properties: Some(crate::ui_properties::Context {
                    state: &mut state,
                    changes: &mut changes,
                    now: &mut now,
                    nested_count: i32::default(),
                }),
            };
            for (value, hidden) in [
                (i32::from(true), true),
                (i32::MAX, false),
                (i32::from(true), true),
                (i32::from(false), false),
            ] {
                Vm::new(&mut host, &())
                    .execute(&script(&[value], &[], "cc_sethide"), &[])
                    .unwrap();
                assert_eq!(
                    replacement_leaf.borrow().runtime_entry_hidden(),
                    Some(hidden)
                );
                assert_eq!(grandchild.borrow().runtime_entry_hidden(), Some(hidden));
                assert!(!replacement_leaf.borrow().f.hide);
                assert!(!grandchild.borrow().f.hide);
                assert!(Rc::ptr_eq(
                    host.active[0].component.as_ref().unwrap(),
                    &grandchild
                ));
            }
        }
        assert!(!crate::ui_hooks::attached(&mut store, &grandchild).unwrap());
        assert!(Rc::ptr_eq(
            &replacement_leaf.borrow().runtime_parent().unwrap(),
            &container
        ));
        // A namespace collision preserves selection and leaves an allocated,
        // empty child list on an otherwise empty target container.
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &root,
                component(CONTAINER_KIND),
                key(u16::default()),
            )
            .unwrap();
        let empty_container = active[0].component.clone().unwrap();
        assert!(store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &empty_container,
                component(TEXT_KIND),
                key(NESTED_KEY)
            )
            .is_err());
        assert!(Rc::ptr_eq(
            active[0].component.as_ref().unwrap(),
            &empty_container
        ));
        assert!(empty_container
            .borrow()
            .children
            .as_ref()
            .unwrap()
            .borrow()
            .is_empty());
        // Replacing a container recursively empties retained descendant arrays
        // before removing its owner index entries; aliases stay allocated.
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &root,
                component(CONTAINER_KIND),
                key(HIGH_KEY),
            )
            .unwrap();
        let replacement_container = active[0].component.clone().unwrap();
        assert!(grandchildren.borrow().is_empty());
        assert!(root.borrow().runtime_child(key(NESTED_KEY)).is_none());
        assert!(!crate::ui_hooks::attached(&mut store, &container).unwrap());
        assert!(!crate::ui_hooks::attached(&mut store, &replacement_leaf).unwrap());
        assert!(Rc::ptr_eq(
            &container.borrow().runtime_parent().unwrap(),
            &root
        ));
        Store::reorder(&interface, Some(&replacement_container), false).unwrap();
        assert!(Rc::ptr_eq(
            drawing.borrow().first().unwrap().as_ref().unwrap(),
            &replacement_container
        ));
        // Invalid author-supplied descendants cannot clear a valid replacement.
        let mut invalid = component(CONTAINER_KIND);
        invalid.children = Some(Rc::new(RefCell::new(vec![Some(leaf.clone())])));
        assert!(store
            .attach_runtime_child(&mut active[0], &interface, &root, invalid, key(HIGH_KEY))
            .is_err());
        assert!(Rc::ptr_eq(
            &root.borrow().runtime_child(key(HIGH_KEY)).unwrap(),
            &replacement_container
        ));
        // Single-child removal uses the selected parent's local namespace,
        // clears nested descendants and preserves retained arrays and selection.
        store
            .attach_runtime_child(
                &mut active[0],
                &interface,
                &replacement_container,
                component(TEXT_KIND),
                key(NESTED_KEY),
            )
            .unwrap();
        let retained_leaf = active[0].component.clone().unwrap();
        let retained_descendants = replacement_container.borrow().children.clone().unwrap();
        store
            .set_runtime_child_hidden(&retained_leaf, true)
            .unwrap();
        let dirty_before = store.updated.len();
        assert!(!store.remove_runtime_child(&empty_container, key(NESTED_KEY)));
        assert_eq!(store.updated.len(), dirty_before);
        assert!(store.remove_runtime_child(&root, key(HIGH_KEY)));
        assert_eq!(store.updated.len(), dirty_before + usize::from(true));
        assert!(retained_descendants.borrow().is_empty());
        assert!(root.borrow().runtime_child(key(HIGH_KEY)).is_none());
        assert!(root.borrow().runtime_child(key(NESTED_KEY)).is_none());
        assert_eq!(retained_leaf.borrow().runtime_entry_hidden(), Some(false));
        store
            .set_runtime_child_hidden(&retained_leaf, true)
            .unwrap();
        assert_eq!(retained_leaf.borrow().runtime_entry_hidden(), Some(false));
        assert!(Rc::ptr_eq(
            active[0].component.as_ref().unwrap(),
            &retained_leaf
        ));
        assert!(Rc::ptr_eq(
            root.borrow().children.as_ref().unwrap(),
            &ordered
        ));
        assert!(Rc::ptr_eq(root.borrow().sorted.as_ref().unwrap(), &drawing));
        assert!(ordered
            .borrow()
            .iter()
            .flatten()
            .any(|current| Rc::ptr_eq(current, &leaf)));
        assert!(!drawing
            .borrow()
            .iter()
            .flatten()
            .any(|current| Rc::ptr_eq(current, &replacement_container)));
        let packed = root.borrow().f.parentlayer;
        store.clear_runtime_children(packed).unwrap();
        assert!(ordered.borrow().is_empty());
        assert!(drawing.borrow().is_empty());
        assert!(root.borrow().runtime_lookup.as_ref().unwrap().is_empty());
        assert!(Rc::ptr_eq(
            root.borrow().children.as_ref().unwrap(),
            &ordered
        ));
        assert!(!crate::ui_hooks::attached(&mut store, &leaf).unwrap());
        // Children do not keep their owner or parent alive.
        let weak_root = Rc::downgrade(&root);
        drop(active);
        drop(store);
        drop(interface);
        drop(root);
        assert!(weak_root.upgrade().is_none());
        assert!(leaf.borrow().runtime_parent().is_none());
        assert_eq!(leaf.borrow().runtime_entry_hidden(), Some(false));
        store = Store::default();
        store.set_runtime_child_hidden(&leaf, true).unwrap();
        assert_eq!(leaf.borrow().runtime_entry_hidden(), Some(false));
        assert!(RuntimeChildId::new(u16::MAX).is_err());
    }

    fn verify_anchored_layout_recording() {
        use crate::ui_layout::{
            AspectConstraint, AxisRecipe, Layout, LayoutPadding, PositionAxis, PositionConstraint,
            SizeAxis, SizeConstraint, ViewportInsets,
        };
        use serde_json::Value;
        const WIDTH_AXIS: usize = 0;
        const HEIGHT_AXIS: usize = 1;
        const AXIS_COUNT: usize = 2;
        const PRIMARY_SLOT: usize = 0;
        const LEFT_EDGE: usize = 0;
        const TOP_EDGE: usize = 1;
        const RIGHT_EDGE: usize = 2;
        const BOTTOM_EDGE: usize = 3;
        let recording: Value =
            serde_json::from_str(include_str!("../fixtures/anchored-layout.json")).unwrap();
        let integers = |row: &Value, field: &str| -> [i32; AXIS_COUNT] {
            std::array::from_fn(|axis| i32::try_from(row[field][axis].as_i64().unwrap()).unwrap())
        };
        let fractions = |row: &Value, field: &str| -> [f32; AXIS_COUNT] {
            std::array::from_fn(|axis| {
                f32::from_bits(u32::try_from(row[field][axis].as_u64().unwrap()).unwrap())
            })
        };
        let viewport = |row: &Value| {
            row["crop"].as_bool().unwrap().then(|| {
                let edge = |index| i32::try_from(row["insets"][index].as_i64().unwrap()).unwrap();
                ViewportInsets {
                    left: edge(LEFT_EDGE),
                    top: edge(TOP_EDGE),
                    right: edge(RIGHT_EDGE),
                    bottom: edge(BOTTOM_EDGE),
                }
            })
        };
        let mode_table: Value =
            serde_json::from_str(include_str!("../../../../../revisions/950/cs2/layout.json"))
                .unwrap();
        assert_eq!(recording["build"], mode_table["build"]);
        assert_eq!(recording["client_md5"], mode_table["client_md5"]);
        let recipes = |kind: &str| -> Vec<AxisRecipe> {
            mode_table[kind]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    let float = |field| {
                        f32::from_bits(u32::try_from(row[field].as_u64().unwrap()).unwrap())
                    };
                    AxisRecipe {
                        pixel_scale: i32::try_from(row["pixel_scale"].as_i64().unwrap()).unwrap(),
                        fraction_scale: float("fraction_scale_bits"),
                        fraction_bias: float("fraction_bias_bits"),
                        fraction_subtract: row["fraction_subtract"].as_bool().unwrap(),
                        element_fraction: float("element_fraction_bits"),
                    }
                })
                .collect()
        };
        let size_recipes = recipes("size");
        let position_recipes = recipes("position");
        for row in recording["size_modes"].as_array().unwrap() {
            let mut constraint = SizeConstraint {
                aspect: AspectConstraint::HeightFromWidth,
                ..Default::default()
            };
            constraint
                .set_modes(
                    &size_recipes,
                    integers(row, "input"),
                    integers(row, "modes"),
                )
                .unwrap();
            assert_eq!(
                constraint.aspect,
                AspectConstraint::HeightFromWidth,
                "{}",
                row["name"]
            );
            let [width, height] = [constraint.width, constraint.height];
            assert_eq!(
                [width.pixels, height.pixels],
                integers(row, "pixels"),
                "{}",
                row["name"]
            );
            assert_eq!(
                [
                    width.parent_fraction.to_bits(),
                    height.parent_fraction.to_bits()
                ],
                fractions(row, "fraction_bits").map(f32::to_bits),
                "{}",
                row["name"]
            );
        }
        for row in recording["position_modes"].as_array().unwrap() {
            let constraint = PositionConstraint::from_modes(
                &position_recipes,
                integers(row, "input"),
                integers(row, "modes"),
            )
            .unwrap();
            let [x, y] = [constraint.x, constraint.y];
            assert_eq!(
                [x.pixels, y.pixels],
                integers(row, "pixels"),
                "{}",
                row["name"]
            );
            assert_eq!(
                [x.parent_fraction.to_bits(), y.parent_fraction.to_bits()],
                fractions(row, "fraction_bits").map(f32::to_bits),
                "{}",
                row["name"]
            );
            assert_eq!(
                [x.element_fraction.to_bits(), y.element_fraction.to_bits()],
                fractions(row, "anchor_bits").map(f32::to_bits),
                "{}",
                row["name"]
            );
        }
        for row in recording["size"].as_array().unwrap() {
            let mut component = Component::default();
            [component.f.width, component.f.height] = integers(row, "previous");
            [component.f.aspectwidth, component.f.aspectheight] = integers(row, "ratio");
            let offsets = integers(row, "pixels");
            let parent_fraction = fractions(row, "fraction_bits");
            component.size_constraint = Some(SizeConstraint {
                width: SizeAxis {
                    pixels: offsets[WIDTH_AXIS],
                    parent_fraction: parent_fraction[WIDTH_AXIS],
                },
                height: SizeAxis {
                    pixels: offsets[HEIGHT_AXIS],
                    parent_fraction: parent_fraction[HEIGHT_AXIS],
                },
                aspect: match row["aspect"].as_str().unwrap() {
                    "independent" => AspectConstraint::Independent,
                    "width_from_height" => AspectConstraint::WidthFromHeight,
                    "height_from_width" => AspectConstraint::HeightFromWidth,
                    other => panic!("unknown recorded aspect {other}"),
                },
            });
            component.viewport_layout = viewport(row);
            component.hooks.insert("onresize", Vec::new());
            let component = Rc::new(RefCell::new(component));
            let mut layout = Layout {
                debug_bounds: true,
                ..Default::default()
            };
            let result = layout.size(&component, integers(row, "parent"), true);
            assert_eq!(
                result.is_err(),
                row.get("trap").is_some(),
                "{}",
                row["name"]
            );
            let fields = &component.borrow().f;
            assert_eq!(
                [fields.width, fields.height],
                integers(row, "size"),
                "{}",
                row["name"]
            );
            assert_eq!(
                layout.hooks.len(),
                usize::from(row["changed"].as_bool().unwrap_or(false)),
                "{}",
                row["name"]
            );
        }
        for row in recording["position"].as_array().unwrap() {
            let mut component = Component::default();
            let dimensions = integers(row, "dimensions");
            let size_fractions = fractions(row, "size_fraction_bits");
            component.size_constraint = Some(SizeConstraint {
                width: SizeAxis {
                    pixels: dimensions[WIDTH_AXIS],
                    parent_fraction: size_fractions[WIDTH_AXIS],
                },
                height: SizeAxis {
                    pixels: dimensions[HEIGHT_AXIS],
                    parent_fraction: size_fractions[HEIGHT_AXIS],
                },
                ..Default::default()
            });
            let offsets = integers(row, "pixels");
            let parent_fraction = fractions(row, "fraction_bits");
            let element_fraction = fractions(row, "anchor_bits");
            component.position_constraint = Some(PositionConstraint {
                x: PositionAxis {
                    pixels: offsets[WIDTH_AXIS],
                    parent_fraction: parent_fraction[WIDTH_AXIS],
                    element_fraction: element_fraction[WIDTH_AXIS],
                },
                y: PositionAxis {
                    pixels: offsets[HEIGHT_AXIS],
                    parent_fraction: parent_fraction[HEIGHT_AXIS],
                    element_fraction: element_fraction[HEIGHT_AXIS],
                },
            });
            component.viewport_layout = viewport(row);
            let component = Rc::new(RefCell::new(component));
            let mut layout = Layout {
                debug_bounds: true,
                ..Default::default()
            };
            let (mut store, active) = static_active();
            let root = active[PRIMARY_SLOT].component.as_ref().unwrap();
            let interface = active[PRIMARY_SLOT].interface.as_ref().unwrap();
            {
                let mut root = root.borrow_mut();
                [root.f.width, root.f.height] = integers(row, "parent");
                let edges = std::array::from_fn(|index| {
                    u8::try_from(row["parent_padding"][index].as_u64().unwrap()).unwrap()
                });
                root.layout_padding = Some(LayoutPadding::from_edges(edges));
            }
            // Static ownership exercises parent resolution before layout.
            component.borrow_mut().f.layer = root.borrow().f.parentlayer;
            layout.align(&mut store, interface, &component).unwrap();
            let fields = &component.borrow().f;
            assert_eq!(
                [fields.width, fields.height],
                integers(row, "size"),
                "{}",
                row["name"]
            );
            assert_eq!(
                [fields.x, fields.y],
                integers(row, "position"),
                "{}",
                row["name"]
            );
        }
        let (mut store, mut active) = static_active();
        let component = active[PRIMARY_SLOT].component.clone().unwrap();
        component.borrow_mut().size_constraint = Some(SizeConstraint::default());
        component.borrow_mut().position_constraint = Some(PositionConstraint::default());
        let mut engine = Engine::default();
        let mut state = crate::ui_properties::State::default();
        let mut changes = crate::ui_changes::Changes::default();
        let mut now = i64::default;
        let mut host = ScriptHost {
            engine: &mut engine,
            store: &mut store,
            active: &mut active,
            properties: Some(crate::ui_properties::Context {
                state: &mut state,
                changes: &mut changes,
                now: &mut now,
                nested_count: 0,
            }),
        };
        let pixel_row = recording["size"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "pixels")
            .unwrap();
        let [width, height] = integers(pixel_row, "pixels");
        let fixed_mode = i32::default();
        Vm::new(&mut host, &())
            .execute(
                &script(&[width, height, fixed_mode, fixed_mode], &[], "cc_setsize"),
                &[],
            )
            .unwrap();
        assert!(component.borrow().size_constraint.is_none());
        assert!(component.borrow().position_constraint.is_some());
        component.borrow_mut().size_constraint = Some(SizeConstraint::default());
        Vm::new(&mut host, &())
            .execute(
                &script(
                    &[width, height, fixed_mode, fixed_mode],
                    &[],
                    "cc_setposition",
                ),
                &[],
            )
            .unwrap();
        assert!(component.borrow().position_constraint.is_none());
        assert!(component.borrow().size_constraint.is_some());
    }

    #[test]
    fn component_runtime_exceptions_are_trap_failures_not_unknown_commands() {
        crate::ui_properties::verify_retained_player_recordings();
        crate::ui_hooks::verify_variable_event_recording();
        verify_recursive_child_removal();
        verify_owner_child_tree();
        verify_recorded_runtime_child_order();
        verify_recorded_text_child_creation();
        verify_recorded_text_properties();
        verify_runtime_child_slot_query();
        verify_anchored_layout_recording();
        // cc_delete: the active component is a static component
        // (`com.id == -1`), which fails the command.
        match run(&[], &[], "cc_delete") {
            Err(VmError::TrapFailed { command, .. }) => assert_eq!(command, "cc_delete"),
            other => panic!("cc_delete on a static component: {other:?}"),
        }
        // cc_create pops three ints (`isp -= 3`); one
        // pushed int indexes below the stack base -> the VM underflow kind.
        assert!(matches!(
            run(&[1], &[], "cc_create"),
            Err(VmError::StackUnderflow { stack: "int" })
        ));
        // Property owner: if_settext on an interface
        // that cannot be opened -> Component.get returns null
        //  and cc_if_settext dereferences it.
        match run(&[88 << 16], &["text"], "if_settext") {
            Err(VmError::TrapFailed { command, .. }) => assert_eq!(command, "if_settext"),
            other => panic!("if_settext on a missing interface: {other:?}"),
        }
        // Property owner: formatminimenu pops twelve ints.
        assert!(matches!(
            run(&[1, 2, 3], &[], "formatminimenu"),
            Err(VmError::StackUnderflow { stack: "int" })
        ));
    }

    #[test]
    fn genuinely_unknown_commands_still_report_unknown_command() {
        // Not a 910 command: it falls through graph and properties to
        // the engine, which reports the command by its own name.
        match run(&[], &[], "not_a_910_command") {
            Err(VmError::UnknownCommand { command }) => assert_eq!(command, "not_a_910_command"),
            other => panic!("unknown command: {other:?}"),
        }
    }
}
