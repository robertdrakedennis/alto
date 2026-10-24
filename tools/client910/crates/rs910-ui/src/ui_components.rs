//! Component references and child-array aliases are retained, including detached
//! children still referenced by scripts or a separately sorted drawing array.
use crate::ui_component_fields::Fields;
pub use crate::utf16_text::Text;
use native910::interface::{self as wire, ComponentBody};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
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

#[derive(Clone, Debug)]
pub struct Component {
    /// Authoring identity recorded after a successful runtime child creation.
    /// It observes the owning instruction without changing lookup or hook state.
    pub creation_origin: Option<CreationOrigin>,
    pub f: Fields,
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

/// Construction counter behind [`Component::particle_serial`].
const FIRST_COMPONENT_SERIAL: u64 = 1; // not a content id
const COMPONENT_SERIAL_INCREMENT: u64 = 1; // not a content id
static PARTICLE_SERIAL: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(FIRST_COMPONENT_SERIAL);

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static REPLAY_COMPONENT_SERIAL: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn next_component_serial() -> u64 {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(serial) = REPLAY_COMPONENT_SERIAL.with(|counter| {
        counter.get().inspect(|&serial| {
            counter.set(Some(serial + COMPONENT_SERIAL_INCREMENT));
        })
    }) {
        return serial;
    }
    PARTICLE_SERIAL.fetch_add(
        COMPONENT_SERIAL_INCREMENT,
        std::sync::atomic::Ordering::Relaxed,
    )
}

/// Recorded sessions allocate component identities from the fresh-process
/// origin, retaining exact serial comparisons and ordinary construction order.
#[cfg(any(test, feature = "test-hooks"))]
pub fn with_fresh_component_serials_for_test<T>(replay: impl FnOnce() -> T) -> T {
    struct Restore(Option<u64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            REPLAY_COMPONENT_SERIAL.with(|counter| counter.set(self.0));
        }
    }
    let _restore = Restore(
        REPLAY_COMPONENT_SERIAL.with(|counter| counter.replace(Some(FIRST_COMPONENT_SERIAL))),
    );
    replay()
}
impl Default for Component {
    fn default() -> Self {
        Self {
            creation_origin: None,
            f: Fields::default(),
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
            params: None,
            children: None,
            sorted: None,
            draggable: None,
            recolour: None,
            retexture: None,
            group_kind: None,
            npc_customisation: None,
            particle_serial: next_component_serial(),
        }
    }
}
impl Component {
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
impl<H: native910::vm::Host> native910::vm::Host for ScriptHost<'_, H> {
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
    /// cc_if_sendto{front,back}. Cloning an aliased child
    /// array uses the child's index; later moves search by object identity.
    pub fn reorder(
        interface: &InterfaceRef,
        component: Option<&Ref>,
        front: bool,
    ) -> anyhow::Result<()> {
        let Some(c) = component else { return Ok(()) };
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

    #[test]
    fn component_runtime_exceptions_are_trap_failures_not_unknown_commands() {
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
}
