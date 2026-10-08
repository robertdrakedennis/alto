//! Retained component property effects.
//! Layout runs synchronously; delayed-change marks happen after the
//! mutation/layout prefix, including writes whose values were already equal.
pub use crate::ui_configs::Param;
use crate::{
    ui_changes::Changes,
    ui_components::{Active, Arg, InterfaceRef, Ref, Store, Text},
    ui_layout::Layout,
};
use anyhow::Context as _;
use std::collections::BTreeMap;
#[derive(Default)]
pub struct Menu {
    pub format: [i32; 12],
    pub custom: bool,
    pub open: bool,
    pub bounds: [i32; 4],
    pub redraw: Vec<[i32; 4]>,
    /// Js5 loadFile requests retain available bytes for the menu consumer.
    pub resources: Vec<(&'static str, i32, Option<Vec<u8>>)>,
    pub decoded: std::cell::RefCell<BTreeMap<(i32, bool), std::rc::Rc<crate::ui_sprites::Sprite>>>,
    /// The submenu arrow sprite: frame 0 of its sprite group.
    pub submenu_arrow: Option<std::rc::Rc<crate::ui_sprites::Sprite>>,
    pack: Option<crate::cache::Pack>,
}
impl Menu {
    fn format(&mut self, args: [i32; 12]) -> anyhow::Result<()> {
        // Close the menu and reset its visual assets.
        self.open = false;
        self.redraw.push(self.bounds);
        self.format = args;
        self.resources.clear();
        self.decoded.get_mut().clear();
        let pack = self
            .pack
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("menu pack not installed"))?;
        for (archive, id) in [4, 5, 6, 7, 8, 11]
            .map(|i| ("sprites", args[i]))
            .into_iter()
            .chain([("fontmetrics", args[11])])
        {
            let bytes = if id < 0 {
                None
            } else {
                crate::js5_fetch::fetch_file(pack, archive, id as u32)?
            };
            self.resources.push((archive, id, bytes));
        }
        self.custom = true;
        Ok(())
    }
}
#[derive(Default)]
pub struct State {
    /// Shared cache-backed model animation owner installed by Runtime.
    pub model_animations:
        Option<std::rc::Rc<std::cell::RefCell<crate::ui_model_animation::Animations>>>,
    pub objs: Option<std::rc::Rc<crate::config::ObjStore>>,
    /// `npcTypeList`, shared with retained type-2 interface head models.
    pub npcs: Option<std::rc::Rc<crate::config::NpcStore>>,
    pub interaction: crate::ui_interaction::Interaction,
    pub menu: Menu,
    /// `MiniMenu` option list and active entry (ui_minimenu.rs).
    pub minimenu: crate::ui_minimenu::MiniMenu,
    pub life: crate::ui_lifecycle::Life,
    pub configs: crate::ui_configs::Configs,
    pub fonts: Option<crate::ui_fonts::Fonts>,
    /// Immutable reviewed font maps shared by script and hook consumers.
    pub font_maps:
        BTreeMap<native910::execution::ResourceDigest, native910::execution::FontMappings>,
    pub icons: Option<std::rc::Rc<std::cell::RefCell<crate::ui_icons::Icons>>>,
    pub sprites: Option<crate::ui_sprites::Resources>,
    /// Component.interfacesDirty, shared across resource lookups.
    pub resource_missing: bool,
    pub layout: Layout,
    pub params: BTreeMap<i32, Param>,
    pub debug_visible: [bool; 2],
    /// `drawDebug` inputs, installed by the window owner.
    pub debug_stats: crate::debug_overlay::DebugStats,
    /// setViewport output; shared by the CS2 query and scene dispatch.
    pub viewport: Option<([i32; 4], i32)>,
    /// The last scene viewport drawn left out the projectiles (the variant
    /// of the scene component that never pushes them).
    pub scene_without_projectiles: bool,
    /// The scene viewport component's own height (canvas pixels), which the
    /// viewport size query receives.
    pub viewport_component_height: Option<i32>,
    /// Mutable `viewportFov*`, `viewportZoom*` and viewport clamp
    /// fields. The UI scene owner installs this profile on the live camera.
    pub viewport_profile: crate::camera::ViewportProfile,
    /// currentPlayerUid, supplied by the live game variable owner.
    pub local_player_uid: i32,
    /// Quest icon tags for scene minimenu entries, installed by the retained
    /// scene owner before `ui_loop` copies options into MiniMenu.
    pub scene_quest_text: BTreeMap<(i32, i64), String>,
    /// Active clan-channel member names used by the linked-component actions.
    /// The bool selects the affined (3) or listened (4) group user kind id.
    pub active_clan_channel: Option<(bool, Vec<String>)>,
    /// `hoverComponent/hoverTextX/Y` retained by
    /// `updateMouseOverText` and consumed by `drawHoverText` after the normal
    /// interface pass.
    pub hover_text: Option<HoverText>,
    /// Scene/social snapshot and requests of the game-coupled component
    /// commands (ui_host_game.rs).
    pub game: crate::ui_runtime::host_game::UiState,
    /// The `setup_messagebox` style, copied from the engine at the start of
    /// each tick for the commands that draw a message box mid-script.
    pub message_box: crate::message_box::MessageBox,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverText {
    pub origin: [i32; 2],
    pub size: [i32; 2],
    pub colour: i32,
    pub shadow: i32,
    pub halign: i32,
    pub valign: i32,
    pub font: i32,
    pub font_mono: bool,
}
impl State {
    /// setViewport, current default FOV/limit profile.
    pub fn set_viewport(&mut self, mut rect: [i32; 4]) -> anyhow::Result<[i32; 4]> {
        self.viewport_component_height = Some(rect[3]);
        rect[2] = rect[2].max(1);
        rect[3] = rect[3].max(1);
        let t = rect[3].wrapping_sub(334).clamp(0, 100);
        let p = self.viewport_profile;
        let mut fov = (p.fov_min - p.fov_max) * t / 100 + p.fov_max;
        let aspect = rect[3] * fov * 512 / (rect[2] * 334);
        if aspect < p.min_height {
            fov = rect[2] * p.min_height * 334 / (rect[3] * 512);
            if fov > p.max_fov {
                fov = p.max_fov;
                let width = rect[3] * fov * 512 / (p.min_height * 334);
                let letterbox = (rect[2] - width) / 2;
                rect[0] += letterbox;
                rect[2] -= letterbox * 2;
            }
        } else if aspect > p.max_height {
            // Field of view from the height: `width * max_height * 334 / (height * 512)`.
            fov = rect[2] * p.max_height * 334 / (rect[3] * 512);
            if fov < p.min_fov {
                fov = p.min_fov;
                let height = rect[2] * p.max_height * 334 / (fov * 512);
                let letterbox = (rect[3] - height) / 2;
                rect[1] += letterbox;
                rect[3] -= letterbox * 2;
            }
        }
        let zoom = rect[3].wrapping_mul(fov) / 334;
        rect[2] = rect[2] as i16 as i32;
        rect[3] = rect[3] as i16 as i32;
        self.viewport = Some((rect, zoom));
        Ok(rect)
    }
    /// A cutscene's picture ratio `width:height` (both non-zero): the scene
    /// view takes one field of view and one height limit, so a viewport of
    /// another shape gets black bars ([`Self::set_viewport`]). Returns the
    /// limits it replaced (`[min height, max height, min fov, max fov]`),
    /// which the end of the cutscene restores.
    pub fn set_cutscene_ratio(&mut self, width: i32, height: i32) -> [i32; 4] {
        let p = &mut self.viewport_profile;
        let saved = [p.min_height, p.max_height, p.min_fov, p.max_fov];
        if height != 0 && width != 0 {
            p.min_fov = 334;
            p.max_fov = 334;
            let limit = i32::from((height * 512 / width) as i16);
            p.max_height = limit;
            p.min_height = limit;
        }
        saved
    }

    pub fn set_viewport_fov(&mut self, max: i32, min: i32) {
        let max = max as i16 as i32;
        let min = min as i16 as i32;
        self.viewport_profile.fov_max = if max <= 0 { 256 } else { max };
        self.viewport_profile.fov_min = if min <= 0 { 205 } else { min };
    }
    pub fn set_viewport_zoom(&mut self, min: i32, max: i32) {
        let min = min as i16 as i32;
        let max = max as i16 as i32;
        self.viewport_profile.zoom_min = if min <= 0 { 256 } else { min };
        self.viewport_profile.zoom_max = if max <= 0 { 320 } else { max };
    }
    pub fn clamp_viewport_fov(
        &mut self,
        min_fov: i32,
        max_fov: i32,
        min_height: i32,
        max_height: i32,
    ) {
        let p = &mut self.viewport_profile;
        let min_fov = min_fov as i16 as i32;
        let max_fov = max_fov as i16 as i32;
        let min_height = min_height as i16 as i32;
        let max_height = max_height as i16 as i32;
        p.min_fov = if min_fov <= 0 { 1 } else { min_fov };
        p.max_fov = if max_fov <= 0 {
            32767
        } else {
            max_fov.max(p.min_fov)
        };
        p.min_height = if min_height <= 0 { 1 } else { min_height };
        p.max_height = if max_height <= 0 {
            32767
        } else {
            max_height.max(p.min_height)
        };
    }
    pub fn load(&mut self, pack: &crate::cache::Pack) -> anyhow::Result<()> {
        self.menu.pack = Some(pack.clone());
        // setDefaults + loadSprites.
        let arrow = crate::protocol910::pack_defaults::load(pack)?
            .graphics
            .scalars
            .submenu_arrow;
        self.menu.submenu_arrow = crate::minimap::sprite_frames(pack, arrow)?
            .into_iter()
            .next();
        self.load_params(pack)?;
        self.configs = crate::ui_configs::Configs::load(pack)?;
        self.fonts = Some(crate::ui_fonts::Fonts::from_pack(pack.clone(), Some(0))?);
        self.sprites = Some(crate::ui_sprites::Resources::from_pack(pack.clone())?);
        Ok(())
    }
    /// The component sprite lookup. The missing-resource flag resets before
    /// sprite lookup, including lookups which subsequently throw.
    pub fn component_sprite(
        &mut self,
        c: &Ref,
        factory: &mut impl crate::ui_sprites::Factory,
    ) -> anyhow::Result<Option<std::rc::Rc<crate::ui_sprites::Sprite>>> {
        self.resource_missing = false;
        self.sprites
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("sprite provider not installed"))?
            .sprite(&c.borrow().f, &mut self.resource_missing, factory)
    }
    /// Component.getGraphic does not touch interfacesDirty.
    pub fn component_graphic(
        &mut self,
        c: &Ref,
        factory: &mut impl crate::ui_sprites::Factory,
    ) -> anyhow::Result<Option<std::rc::Rc<crate::ui_sprites::Mask>>> {
        self.sprites
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("sprite provider not installed"))?
            .mask(&c.borrow().f, factory)
    }
    /// The component font lookups update the interface-dirty flag after lookup;
    /// an exception leaves the preceding flag unchanged.
    pub fn component_font(
        &mut self,
        c: &Ref,
    ) -> anyhow::Result<Option<std::rc::Rc<crate::ui_fonts::Font>>> {
        let f = &c.borrow().f;
        let font = self
            .fonts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("font provider not installed"))?
            .get_font(f.textfont, false, f.fontmono)?;
        self.resource_missing = font.is_none();
        Ok(font)
    }
    pub fn component_metrics(
        &mut self,
        c: &Ref,
    ) -> anyhow::Result<Option<std::rc::Rc<crate::font_metrics::Metrics>>> {
        let font = self
            .fonts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("font provider not installed"))?
            .get_metrics(c.borrow().f.textfont, true, true)?;
        self.resource_missing = font.is_none();
        Ok(font)
    }
    pub fn load_params(&mut self, pack: &crate::cache::Pack) -> anyhow::Result<()> {
        for (id, b) in pack.read_group("config", 11)? {
            self.params.insert(id as i32, Param::decode(&b)?);
        }
        Ok(())
    }
}
pub struct Context<'a> {
    pub state: &'a mut State,
    pub changes: &'a mut Changes,
    pub now: &'a mut dyn FnMut() -> i64,
    pub nested_count: i32,
}
/// These retail handlers only discard arguments;
/// they do not resolve a component or read the active pointer.
pub const DISCARDS: &[(&str, usize)] = &[
    // disabled_command_412: discards the interface-target argument.
    ("discardInterfaceTargetArg", 1),
    // disabled_command_298.
    ("discardThreeInterfaceArgs", 3),
    // disabled_command_469.
    ("discardInterfaceArg", 1),
    // interface_setpickingradius.
    ("interface_setpickingradius", 1),
    // disabled_command_1292.
    ("discardLogoutArg", 1),
    ("discardCurrentSwipeSettingArg", 1),
    ("cc_setswipedeadtime", 1),
    ("if_setswipedeadtime", 2),
    ("cc_setswipedeadzone", 1),
    ("if_setswipedeadzone", 2),
    ("cc_setswipeflags", 1),
    ("if_setswipeflags", 2),
    ("cc_addswipeflags", 1),
    ("if_addswipeflags", 2),
    ("cc_delswipeflags", 1),
    ("if_delswipeflags", 2),
    ("cc_setpinchflags", 1),
    ("if_setpinchflags", 2),
    ("cc_addpinchflags", 1),
    ("if_addpinchflags", 2),
    ("cc_delpinchflags", 1),
    ("if_delpinchflags", 2),
    ("cc_setpinchdeadzone", 1),
    ("if_setpinchdeadzone", 2),
    ("cc_setsubtractinsets", 1),
    ("if_setsubtractinsets", 2),
];
pub const SETTERS: &[(&str, usize, usize)] = &[
    ("setposition", 4, 0),
    ("setsize", 4, 0),
    ("setaspect", 2, 0),
    ("sethide", 1, 0),
    ("setnoclickthrough", 1, 0),
    ("setscrollpos", 2, 0),
    ("setscrollsize", 2, 0),
    ("setcolour", 1, 0),
    ("setfill", 1, 0),
    ("settrans", 1, 0),
    ("setlinewid", 1, 0),
    ("setgraphic", 1, 0),
    ("set2dangle", 1, 0),
    ("settiling", 1, 0),
    ("settext", 0, 1),
    ("settextfont", 1, 0),
    ("settextalign", 3, 0),
    ("settextshadow", 1, 0),
    ("settextantimacro", 1, 0),
    ("setoutline", 1, 0),
    ("setgraphicshadow", 1, 0),
    ("setvflip", 1, 0),
    ("sethflip", 1, 0),
    ("setalpha", 1, 0),
    ("setlinedirection", 1, 0),
    ("setmaxlines", 1, 0),
    ("setfontmono", 1, 0),
    ("setclickmask", 1, 0),
    ("setheld", 1, 0),
    ("setmodel", 1, 0),
    ("setmodelanim", 1, 0),
    ("setplayermodel_self", 0, 0),
    // cc_if_setnpchead, cc_if_setnpcmodel,
    // cc_if_setplayermodel, cc_if_setplayerhead_self.
    ("setnpchead", 1, 0),
    ("setnpcmodel", 1, 0),
    ("setplayermodel", 1, 0),
    ("setplayerhead_self", 0, 0),
    ("setobject", 2, 0),
    ("setobject_nonum", 2, 0),
    ("setobject_alwaysnum", 2, 0),
    ("setobject_wearcol", 2, 0),
    ("setobject_wearcol_nonum", 2, 0),
    ("setobject_wearcol_alwaysnum", 2, 0),
    ("setmodelangle", 6, 0),
    ("setmodelorthog", 1, 0),
    ("setmodeltint", 4, 0),
    ("setmodellighting", 10, 0),
    ("resetmodellighting", 0, 0),
    ("setmodelzoom", 1, 0),
    ("setmodelorigin", 2, 0),
    ("setrecol", 3, 0),
    ("setretex", 3, 0),
    ("setopchar", 2, 0),
    ("setoptchar", 1, 0),
    ("setopkey", 11, 0), // if_ uses one key/mod pair (three integers).
    ("setoptkey", 2, 0),
    ("setopkeyrate", 3, 0),
    ("setoptkeyrate", 2, 0),
    ("setopkeyignoreheld", 1, 0),
    ("setoptkeyignoreheld", 0, 0),
    ("setop", 1, 1),
    ("setopcursor", 2, 0),
    ("clearops", 0, 0),
    ("setopbase", 0, 1),
    ("settargetverb", 0, 1),
    ("setpausetext", 0, 1),
    ("settargetcursors", 2, 0),
    ("settargetopcursor", 1, 0),
    ("setmouseovercursor", 1, 0),
    ("setdragrenderbehaviour", 1, 0),
    ("setdragdeadzone", 1, 0),
    ("setdragdeadtime", 1, 0),
    ("setdraggable", 2, 0),
    ("setparam", 2, 0),
    ("setparam_int", 2, 0),
    ("setparam_string", 1, 1),
    ("setlinkactiveclanchannel", 1, 0),
    ("resetlinkplayer", 0, 0),
    ("clearscripthooks", 0, 0),
];
pub const GETTERS: &[&str] = &[
    "getx",
    "gety",
    "getwidth",
    "getheight",
    "gethide",
    "getlayer",
    "getparentlayer",
    "getcolour",
    "getscrollx",
    "getscrolly",
    "gettext",
    "getscrollwidth",
    "getscrollheight",
    "getmodelzoom",
    "getmodelangle_x",
    "getmodelangle_y",
    "getmodelangle_z",
    "gettrans",
    "getmodelxof",
    "getmodelyof",
    "getgraphic",
    "get2dangle",
    "getmodel",
    "getfontgraphic",
    "getfontmetrics",
    "getinvobject",
    "getinvcount",
    "getid",
    "gettargetmask",
    "getop",
    "getopbase",
    "getnextsubid",
    "param",
];
pub const HOOKS: &[(&str, &str, Option<&str>)] = &[
    ("setonclick", "onclick", None),
    ("setonhold", "onhold", None),
    ("setonrelease", "onrelease", None),
    ("setonmouseover", "onmouseover", None),
    ("setonmouseleave", "onmouseleave", None),
    ("setondrag", "ondrag", None),
    ("setondragcomplete", "ondragcomplete", None),
    ("setontargetleave", "ontargetleave", None),
    ("setontargetenter", "ontargetenter", None),
    (
        "setonvartransmit",
        "onvartransmit",
        Some("onvartransmitlist"),
    ),
    ("setontimer", "ontimer", None),
    ("setonop", "onop", None),
    ("setonopt", "onopt", None),
    ("setonclickrepeat", "onclickrepeat", None),
    ("setonmouserepeat", "onmouserepeat", None),
    (
        "setoninvtransmit",
        "oninvtransmit",
        Some("oninvtransmitlist"),
    ),
    (
        "setonstattransmit",
        "onstattransmit",
        Some("onstattransmitlist"),
    ),
    ("setonscrollwheel", "onscrollwheel", None),
    ("setonchattransmit", "onchattransmit", None),
    ("setonkey", "onkey", None),
    ("setonfriendtransmit", "onfriendtransmit", None),
    ("setonclantransmit", "onclantransmit", None),
    ("setonmisctransmit", "onmisctransmit", None),
    ("setondialogabort", "ondialogabort", None),
    ("setonsubchange", "onsubchange", None),
    ("setonstocktransmit", "onstocktransmit", None),
    ("setoncamfinished", "oncamfinished", None),
    (
        "setonvarctransmit",
        "onvarctransmit",
        Some("onvarctransmitlist"),
    ),
    (
        "setonvarcstrtransmit",
        "onvarcstrtransmit",
        Some("onvarcstrtransmitlist"),
    ),
    ("setonvarclantransmit", "onvarclantransmit", None),
    ("setonresize", "onresize", None),
    ("setonclansettingstransmit", "onclansettingstransmit", None),
    ("setonclanchanneltransmit", "onclanchanneltransmit", None),
    ("setonplayergrouptransmit", "onplayergrouptransmit", None),
    (
        "setonplayergroupvarptransmit",
        "onplayergroupvarptransmit",
        None,
    ),
    ("setoncameraupdatetransmit", "oncameraupdatetransmit", None),
];
/// `VmError::StackUnderflow` for the VM stack holding `T` (the original client's
/// `intStack`/`objectStack`/`longStack` index going below zero).
fn underflow<T>() -> anyhow::Error {
    let stack = match std::any::type_name::<T>() {
        "i32" => "int",
        "i64" => "long",
        _ => "object",
    };
    native910::vm::VmError::StackUnderflow { stack }.into()
}
fn pop<T>(s: &mut Vec<T>) -> anyhow::Result<T> {
    s.pop().ok_or_else(underflow::<T>)
}
fn take<T>(s: &mut Vec<T>, n: usize) -> anyhow::Result<Vec<T>> {
    if s.len() < n {
        return Err(underflow::<T>());
    }
    Ok(s.split_off(s.len() - n))
}
fn vm_string(s: Option<&[u16]>) -> anyhow::Result<native910::vm::Value> {
    Ok(s.map_or(native910::vm::Value::Null, |s| {
        native910::vm::Value::Str(native910::jstr::from_units(s))
    }))
}
/// A popped hook: its arguments and the optional transmit int array.
type PoppedHook = (Option<Vec<Arg>>, Option<Vec<i32>>);
/// popIntArray/popHookArgs.
pub(crate) fn parse_hook(
    i: &mut Vec<i32>,
    s: &mut Vec<String>,
    l: &mut Vec<i64>,
) -> anyhow::Result<PoppedHook> {
    parse_hook_with(i, s, l, Some)
}
fn parse_hook_objects(
    i: &mut Vec<i32>,
    s: &mut Vec<Option<String>>,
    l: &mut Vec<i64>,
) -> anyhow::Result<PoppedHook> {
    parse_hook_with(i, s, l, std::convert::identity)
}
fn parse_hook_with<T>(
    i: &mut Vec<i32>,
    s: &mut Vec<T>,
    l: &mut Vec<i64>,
    object: impl Fn(T) -> Option<String>,
) -> anyhow::Result<PoppedHook> {
    // popIntArray/popHookArgs. A non-positive
    // Y-list count leaves Y in the signature, where it consumes an int.
    let mut signature: Vec<u16> = native910::jstr::units(&object(pop(s)?).ok_or_else(|| {
        native910::vm::VmError::NullObject {
            command: "hook signature".into(),
        }
    })?);
    let mut transmit = None;
    if signature.last() == Some(&(b'Y' as u16)) {
        let count = pop(i)?;
        if count > 0 {
            transmit = Some(take(i, count as usize)?);
            signature.pop();
        }
    }
    let mut args = vec![Arg::Null; signature.len() + 1];
    for n in (0..signature.len()).rev() {
        args[n + 1] = match signature[n] {
            unit if unit == u16::from(b's') => {
                object(pop(s)?).map_or(Arg::Null, |s| Arg::String(native910::jstr::units(&s)))
            }
            unit if unit == u16::from(b'l') => Arg::Long(pop(l)?),
            _ => Arg::Int(pop(i)?),
        };
    }
    let id = pop(i)?;
    args[0] = Arg::Int(id);
    Ok((if id == -1 { None } else { Some(args) }, transmit))
}
impl Context<'_> {
    pub fn dispatch(
        &mut self,
        store: &mut Store,
        active: &mut [Active; 2],
        ctx: &native910::vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
        strs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> Option<anyhow::Result<Option<native910::vm::Value>>> {
        if ctx.command == "formatminimenu" {
            return Some((|| {
                let a = take(ints, 12)?;
                self.state.minimenu.close_popup();
                self.state.menu.format(a.try_into().unwrap())?;
                Ok(None)
            })());
        }
        if ctx.command == "defaultminimenu" {
            self.state.menu.custom = false;
            self.state.menu.resources.clear();
            self.state.menu.decoded.get_mut().clear();
            return Some(Ok(None));
        }
        if ctx.command == "minimenuopen" {
            return Some((|| {
                let a = take(ints, 2)?;
                Ok(Some(native910::vm::Value::Int(
                    crate::ui_interaction::menu_open_for(store, self.state, a[0], a[1])? as i32,
                )))
            })());
        }
        if matches!(
            ctx.command,
            "viewport_clampfov"
                | "viewport_setfov"
                | "viewport_setzoom"
                | "viewport_getfov"
                | "viewport_getzoom"
        ) {
            // These mutate the live viewport
            // profile; the next scene call applies the same original clamps and
            // the renderer copies the profile into SceneCamera.
            return Some((|| {
                if ctx.command.starts_with("viewport_get") {
                    let p = self.state.viewport_profile;
                    if ctx.command == "viewport_getfov" {
                        ints.extend([p.fov_max, p.fov_min]);
                    } else {
                        ints.extend([p.zoom_min, p.zoom_max]);
                    }
                    return Ok(None);
                }
                if ctx.command == "viewport_setfov" {
                    let values = take(ints, 2)?;
                    self.state.set_viewport_fov(values[0], values[1]);
                } else if ctx.command == "viewport_setzoom" {
                    let values = take(ints, 2)?;
                    self.state.set_viewport_zoom(values[0], values[1]);
                } else {
                    let values = take(ints, 4)?;
                    self.state
                        .clamp_viewport_fov(values[0], values[1], values[2], values[3]);
                }
                Ok(None)
            })());
        }
        if ctx.command == "viewport_geteffectivesize" {
            // The effective size uses the actual layout viewport, writes the
            // viewport globals, then pushes width followed by height.
            return Some((|| {
                let c = self
                    .state
                    .layout
                    .viewport
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("viewport component missing"))?
                    .borrow();
                let rect = [0, 0, c.f.width, c.f.height];
                drop(c);
                let rect = self.state.set_viewport(rect)?;
                ints.extend([rect[2], rect[3]]);
                Ok(None)
            })());
        }
        if let Some(result) =
            crate::ui_fonts::dispatch(self.state.fonts.as_ref(), ctx.command, ints, strs)
        {
            return Some(result);
        }
        if let Some(result) =
            crate::ui_runtime::host_game::component(self, store, active, ctx, ints, strs, longs)
        {
            return Some(result);
        }
        if let Some((_, n)) = DISCARDS.iter().find(|(name, _)| *name == ctx.command) {
            return Some(take(ints, *n).map(|_| None));
        }
        if matches!(
            ctx.command,
            "cc_setonverticalswipe"
                | "if_setonverticalswipe"
                | "cc_setonhorizontalswipe"
                | "if_setonhorizontalswipe"
                | "cc_discardGestureHook"
                | "if_discardGestureHook"
        ) {
            // The command resolves named inputs, parses the hook,
            // then discards it. No component field or hashook flag is changed.
            // cc_if_discardhook/if_discardhook/cc_discardhook are
            // the same parse-and-discard body.
            return Some((|| {
                if ctx.command.starts_with("if_") {
                    let packed = pop(ints)?;
                    store.get(packed, -1)?;
                    anyhow::ensure!(packed >> 16 >= 0, "negative interface array index");
                }
                parse_hook(ints, strs, longs)?;
                Ok(None)
            })());
        }
        if matches!(ctx.command, "cc_callonresize" | "if_callonresize") {
            return Some((|| {
                let c = if ctx.command == "if_callonresize" {
                    store.get(pop(ints)?, -1)?
                } else {
                    active[ctx.secondary as usize].component.clone()
                };
                // Check depth before dereferencing the
                // component or checking whether its resize hook is present.
                anyhow::ensure!(self.nested_count < 10, "resize hook nesting limit");
                let c = c.ok_or_else(|| anyhow::anyhow!("resize component missing"))?;
                let args = c.borrow().hooks.get("onresize").cloned();
                if let Some(args) = args {
                    let mut r = crate::ui_hooks::Request::component(&c, args);
                    r.nested_count = self.nested_count + 1;
                    self.state.layout.hooks.push_back(r);
                }
                Ok(None)
            })());
        }
        if let Some(result) =
            self.state
                .configs
                .dispatch(&self.state.params, ctx.command, ints, strs)
        {
            return Some(result.map(Some));
        }
        if let Some(result) = crate::ui_lifecycle::command(store, self.state, ctx.command, ints) {
            return Some(result);
        }
        let mut objects = std::mem::take(strs).into_iter().map(Some).collect();
        let result = self.dispatch_components(store, active, ctx, ints, &mut objects, longs);
        *strs = objects.into_iter().flatten().collect();
        result
    }
    /// Component handlers that copy object references retain null values.
    pub(crate) fn dispatch_objects(
        &mut self,
        store: &mut Store,
        active: &mut [Active; 2],
        ctx: &native910::vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> Option<anyhow::Result<Option<native910::vm::Value>>> {
        let suffix = ctx
            .command
            .strip_prefix("cc_")
            .or_else(|| ctx.command.strip_prefix("if_"))?;
        if matches!(
            suffix,
            "setonverticalswipe" | "setonhorizontalswipe" | "discardGestureHook"
        ) {
            return Some((|| {
                if ctx.command.starts_with("if_") {
                    let packed = pop(ints)?;
                    store.get(packed, -1)?;
                    anyhow::ensure!(packed >> 16 >= 0, "negative interface array index");
                }
                parse_hook_objects(ints, objects, longs)?;
                Ok(None)
            })());
        }
        // Commands with no object inputs continue through their ordinary owners.
        if !HOOKS.iter().any(|h| h.0 == suffix)
            && suffix != "setondragcomplete_alias"
            && !SETTERS.iter().any(|s| s.0 == suffix && s.2 != 0)
        {
            return None;
        }
        self.dispatch_components(store, active, ctx, ints, objects, longs)
    }
    /// Text scalar projection consumes values before an absent text capability
    /// returns. Byte fields retain their stored unsigned values.
    pub(crate) fn text_component(
        &mut self,
        store: &mut Store,
        active: &mut [Active; 2],
        ctx: &native910::vm::InstructionContext<'_>,
        operation: native910::execution::HostOperation,
        resource: Option<&native910::execution::Resource>,
        ints: &mut Vec<i32>,
    ) -> anyhow::Result<Option<native910::vm::Value>> {
        use native910::execution::{HostOperation, TextProperty};
        let HostOperation::ComponentText {
            property,
            text_type,
            explicit,
        } = operation
        else {
            anyhow::bail!("text consumer contract missing");
        };
        let (interface, component) = if explicit {
            let packed = pop(ints)?;
            let component = store.explicit_component(packed)?;
            let group = ((packed as u32) >> u16::BITS) as i32;
            (store.interfaces.get(&group).cloned(), component)
        } else {
            let selected = &active[usize::from(ctx.secondary)];
            (selected.interface.clone(), selected.component.clone())
        };
        let values = take(ints, usize::from(property.integer_arguments()))?;
        let Some(component) =
            component.filter(|component| component.borrow().f.r#type == text_type)
        else {
            return Ok(match property {
                TextProperty::ReadFont { absent, .. } => Some(native910::vm::Value::Int(absent)),
                _ => None,
            });
        };
        let mut component = component.borrow_mut();
        match property {
            TextProperty::Font {
                domain,
                change_kind,
                mapping,
            } => {
                use native910::execution::{FontBinding, FontMappings};
                const FONT_ARGUMENT: usize = 0;
                let source_font = values[FONT_ARGUMENT];
                let mapping = match mapping {
                    FontBinding::Unbound => anyhow::bail!("font consumer has no named binding"),
                    FontBinding::Constant(mapping) => {
                        anyhow::ensure!(
                            source_font == mapping.source,
                            "font value differs from the pinned donor use"
                        );
                        mapping
                    }
                    FontBinding::Resource => {
                        let resource = resource.context("font map resource missing")?;
                        let fonts = match self.state.font_maps.entry(resource.digest()) {
                            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                            std::collections::btree_map::Entry::Vacant(entry) => {
                                entry.insert(FontMappings::decode_resource(resource.bytes())?)
                            }
                        };
                        fonts.get(source_font).with_context(|| format!("font value {source_font} has no reviewed asset mapping; add it to this consumer's font map"))?
                    }
                };
                component.set_source_font(domain, mapping);
                if component.f.id == -i32::from(true)
                    && !interface
                        .context("text interface missing")?
                        .borrow()
                        .transient
                {
                    let packed = component.f.parentlayer;
                    let packed = if packed as u16 == u16::MAX {
                        -i32::from(true)
                    } else {
                        packed
                    };
                    self.changes.push_client(
                        i32::from(change_kind),
                        i64::from(packed as u32),
                        (self.now)(),
                    );
                }
            }
            TextProperty::ReadFont {
                domain,
                absent,
                initial,
                ..
            } => {
                return Ok(Some(native910::vm::Value::Int(
                    component.source_font(domain, absent, initial)?,
                )));
            }
            TextProperty::Alignment => {
                const HORIZONTAL: usize = 0;
                const VERTICAL: usize = 1;
                const LINE_HEIGHT: usize = 2;
                component.f.textHAlign = i32::from(values[HORIZONTAL] as u8);
                component.f.textVAlign = i32::from(values[VERTICAL] as u8);
                component.f.textLineHeight = i32::from(values[LINE_HEIGHT] as u8);
            }
            TextProperty::MaxLines => component.f.maxlines = i32::from(values[0] as u8),
        }
        Ok(None)
    }

    /// Revision-bound paint writes use the ordinary component and change owners.
    pub(crate) fn paint_component(
        &mut self,
        store: &mut Store,
        active: &mut [Active; 2],
        ctx: &native910::vm::InstructionContext<'_>,
        property: native910::execution::PaintProperty,
        explicit: bool,
        ints: &mut Vec<i32>,
    ) -> anyhow::Result<()> {
        use native910::execution::PaintProperty;
        let (interface, component) = if explicit {
            let packed = pop(ints)?;
            let component = store.get(packed, -1)?.context("component missing")?;
            let interface = store
                .interfaces
                .get(&(packed >> u16::BITS))
                .cloned()
                .context("interface missing")?;
            (interface, component)
        } else {
            let selected = &active[usize::from(ctx.secondary)];
            (
                selected.interface.clone().context("no active interface")?,
                selected.component.clone().context("no active component")?,
            )
        };
        let value = pop(ints)?;
        let mut component = component.borrow_mut();
        match property {
            PaintProperty::Colour { change_kind } => {
                component.f.colour = value;
                if component.f.id == -1 && !interface.borrow().transient {
                    // A reserved file identity denotes the all-ones component.
                    let packed = component.f.parentlayer;
                    let packed = if packed as u16 == u16::MAX {
                        -1
                    } else {
                        packed
                    };
                    self.changes.push_client(
                        i32::from(change_kind),
                        i64::from(packed as u32),
                        (self.now)(),
                    );
                }
            }
            PaintProperty::Fill { rectangle_type } => {
                if component.f.r#type == rectangle_type {
                    component.f.fill = value == i32::from(true);
                }
            }
            PaintProperty::Transparency => component.f.trans = i32::from(value as u8),
        }
        Ok(())
    }

    fn dispatch_components(
        &mut self,
        store: &mut Store,
        active: &mut [Active; 2],
        ctx: &native910::vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
        strs: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> Option<anyhow::Result<Option<native910::vm::Value>>> {
        let (named, suffix) = if ctx.command == "getparentlayer_alias" {
            // Opcode book retains the obfuscated name for opcode 1431;
            // It dispatches cc_getparentlayer.
            (false, "getparentlayer")
        } else if let Some(s) = ctx.command.strip_prefix("if_") {
            (true, s)
        } else if let Some(s) = ctx.command.strip_prefix("cc_") {
            (false, s)
        } else {
            return None;
        };
        let suffix = if suffix == "setondragcomplete_alias" {
            "setondragcomplete"
        } else {
            suffix
        };
        let setter = SETTERS.iter().find(|s| s.0 == suffix).copied();
        let hook = HOOKS.iter().find(|h| h.0 == suffix).copied();
        if setter.is_none() && hook.is_none() && !GETTERS.contains(&suffix) {
            return None;
        }
        Some((|| {
            let (interface, c) = if named {
                let packed = pop(ints)?;
                let c = store
                    .get(packed, -1)?
                    .ok_or_else(|| anyhow::anyhow!("component missing"))?;
                let interface = store
                    .interfaces
                    .get(&(packed >> 16))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("interface missing"))?;
                (interface, c)
            } else {
                let a = &active[ctx.secondary as usize];
                (
                    a.interface
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("no active interface"))?,
                    a.component
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("no active component"))?,
                )
            };
            if let Some((_, field, list)) = hook {
                self.hook(&c, field, list, ints, strs, longs)?;
                return Ok(None);
            }
            if let Some((_, ni, ns)) = setter {
                let ni = if named && suffix == "setopkey" { 3 } else { ni };
                let i = take(ints, ni)?;
                #[cfg(any(test, feature = "test-hooks"))]
                if suffix == "sethide" && crate::ui_debug_flags::flags().ui_write_trace {
                    eprintln!(
                        "visibility write: script {:?} pc {} component {}/{} args {i:?}",
                        ctx.script_id,
                        ctx.pc,
                        c.borrow().f.parentlayer,
                        c.borrow().f.id
                    );
                }
                let s = take(strs, ns)?
                    .into_iter()
                    .map(|s| s.map(|s| native910::jstr::units(&s)))
                    .collect::<Vec<_>>();
                self.set(store, &interface, &c, suffix, &i, &s)?;
                Ok(None)
            } else {
                self.get(store, &interface, &c, suffix, named, ints)
                    .map(Some)
            }
        })())
    }
    fn hook(
        &self,
        c: &Ref,
        field: &'static str,
        list: Option<&'static str>,
        i: &mut Vec<i32>,
        s: &mut Vec<Option<String>>,
        l: &mut Vec<i64>,
    ) -> anyhow::Result<()> {
        let (args, transmit) = parse_hook_objects(i, s, l)?;
        let mut c = c.borrow_mut();
        if field == crate::ui_properties::RETAINED_PLAYER_HOOK {
            c.retained_player_transmit = None;
        }
        if let Some(args) = args {
            c.hooks.insert(field, args);
        } else {
            c.hooks.remove(field);
        }
        if let Some(list) = list {
            if let Some(t) = transmit {
                c.transmits.insert(list, t);
            } else {
                c.transmits.remove(list);
            }
        }
        c.f.hashook = true;
        Ok(())
    }
    fn set(
        &mut self,
        store: &mut Store,
        interface: &InterfaceRef,
        c: &Ref,
        command: &str,
        i: &[i32],
        s: &[Option<Text>],
    ) -> anyhow::Result<()> {
        if command == "sethide" && c.borrow().has_runtime_parent() {
            return store.set_runtime_child_hidden(c, i[0] == 1);
        }
        let mut update = false;
        let mut align = false;
        let mut redraw = false;
        let mut customisation_cleared = false;
        let mut marks = vec![];
        {
            let mut c0 = c.borrow_mut();
            if command == "settextfont" {
                c0.set_target_font(i[0]);
            }
            let crate::ui_components::Component {
                f,
                model_animator,
                size_constraint,
                position_constraint,
                ..
            } = &mut *c0;
            macro_rules! integer {
                ($field:ident) => {{
                    f.$field = i[0];
                    update = true;
                }};
            }
            macro_rules! boolean {
                ($field:ident) => {{
                    f.$field = i[0] == 1;
                    update = true;
                }};
            }
            match command {
                "setposition" => {
                    *position_constraint = None;
                    f.xpos = i[0];
                    f.ypos = i[1];
                    f.xmode = i[2].clamp(0, 5) as i8;
                    f.ymode = i[3].clamp(0, 5) as i8;
                    update = true;
                    align = true;
                    redraw = true;
                    marks.push((11, 0));
                }
                "setsize" => {
                    *size_constraint = None;
                    f.wsize = i[0];
                    f.hsize = i[1];
                    f.modelobjwidth = 0;
                    f.modelobjheight = 0;
                    f.widthSizeMode = i[2].clamp(0, 4) as i8;
                    f.heightSizeMode = i[3].clamp(0, 4) as i8;
                    update = true;
                    align = true;
                    redraw = true;
                }
                "setaspect" => {
                    f.aspectwidth = i[0];
                    f.aspectheight = i[1];
                    update = true;
                    align = true;
                    redraw = true;
                }
                "sethide" => {
                    let v = i[0] == 1;
                    if crate::ui_debug_flags::flags().ui_trace_hide {
                        log::info!(
                            "[trace] sethide {}:{} (packed {}) hide {} -> {v}",
                            f.parentlayer >> 16,
                            f.parentlayer & 0xFFFF,
                            f.parentlayer,
                            f.hide
                        );
                    }
                    update = f.hide != v;
                    f.hide = v;
                    marks.push((7, 0));
                }
                "setnoclickthrough" => f.noclickthrough = i[0] == 1,
                "setscrollpos" => {
                    f.scrollx = i[0].min(f.scrollwidth.wrapping_sub(f.width)).max(0);
                    f.scrolly = i[1].min(f.scrollheight.wrapping_sub(f.height)).max(0);
                    update = true;
                    marks.push((12, 0));
                }
                "setscrollsize" => {
                    f.scrollwidth = i[0];
                    f.scrollheight = i[1];
                    update = true;
                    redraw = true;
                }
                "setcolour" => {
                    integer!(colour);
                    marks.push((6, 0));
                }
                "setfill" => boolean!(fill),
                "settrans" => integer!(trans),
                "setlinewid" => integer!(linewid),
                "setgraphic" => {
                    f.invobject = -1;
                    update = f.graphic != i[0];
                    f.graphic = i[0];
                    marks.push((13, 0));
                }
                "set2dangle" => integer!(angle2d),
                "settiling" => boolean!(tiling),
                "settext" => {
                    let text = s[0]
                        .as_ref()
                        .ok_or_else(|| native910::vm::VmError::NullObject {
                            command: "settext".into(),
                        })?;
                    update = f.text.as_ref() != Some(text);
                    f.text = Some(text.clone());
                    marks.push((3, 0));
                }
                "settextfont" => {
                    update = true;
                    marks.push((15, 0));
                }
                "settextalign" => {
                    f.textHAlign = i[0];
                    f.textVAlign = i[1];
                    f.textLineHeight = i[2];
                    update = true;
                }
                "settextshadow" => boolean!(textshadow),
                "settextantimacro" => {
                    boolean!(textantimacro);
                    marks.push((23, 0));
                }
                "setoutline" => integer!(outline),
                "setgraphicshadow" => integer!(graphicshadow),
                "setvflip" => boolean!(vflip),
                "sethflip" => boolean!(hflip),
                "setalpha" => boolean!(alpha),
                "setlinedirection" => boolean!(linedirection),
                "setmaxlines" => integer!(maxlines),
                "setfontmono" => {
                    boolean!(fontmono);
                    marks.push((21, 0));
                }
                "setclickmask" => {
                    boolean!(clickmask);
                    marks.push((22, 0));
                }
                "setheld" => boolean!(held),
                "setmodel" => {
                    f.modelkind = 1;
                    integer!(model);
                    marks.push((4, 0));
                }
                "setmodelanim" => {
                    let sequence = i[0];
                    if f.modelanim != sequence {
                        if sequence == -1 {
                            f.modelanim = -1;
                            *model_animator = None;
                        } else {
                            let service = self
                                .state
                                .model_animations
                                .as_ref()
                                .context("model animation service not installed")?
                                .clone();
                            let mut playback = model_animator.take().unwrap_or_default();
                            service.borrow_mut().start(&mut playback, sequence)?;
                            *model_animator = Some(playback);
                            f.modelanim = sequence;
                        }
                        update = true;
                    }
                    marks.push((5, 0));
                }
                "setplayermodel_self" => {
                    f.modelkind = 5;
                    f.model = self.state.local_player_uid;
                    f.modelNameHash = 0;
                    marks.push((4, 0));
                }
                // Model kinds 2/6 clear `customisation`; kinds 3/5 keep it.
                // None of these four calls componentUpdated.
                "setnpchead" | "setnpcmodel" => {
                    f.modelkind = if command == "setnpchead" { 2 } else { 6 };
                    f.model = i[0];
                    customisation_cleared = true;
                    marks.push((4, 0));
                }
                "setplayermodel" => {
                    f.modelkind = 5;
                    f.model = i[0];
                    marks.push((4, 0));
                }
                "setplayerhead_self" => {
                    f.modelkind = 3;
                    f.model = self.state.local_player_uid;
                    f.modelNameHash = 0;
                    marks.push((4, 0));
                }
                // cc_if_setobject_data.
                "setobject"
                | "setobject_nonum"
                | "setobject_alwaysnum"
                | "setobject_wearcol"
                | "setobject_wearcol_nonum"
                | "setobject_wearcol_alwaysnum" => {
                    marks.extend([(9, 0), (8, 0), (10, 0)]);
                    if i[0] == -1 {
                        f.modelkind = 1;
                        f.model = -1;
                        f.invobject = -1;
                    } else {
                        let default = crate::config::decode_obj(i[0] as u32, &[0])?;
                        let o = self
                            .state
                            .objs
                            .as_ref()
                            .context("object types not installed")?
                            .get(i[0] as u32)
                            .unwrap_or(&default);
                        let p = &o.inventory;
                        f.invobject = i[0];
                        f.invcount = i[1];
                        f.usePlayerModel = command.contains("wearcol");
                        f.modelangle_x = p.angles[0];
                        f.modelangle_y = p.angles[1];
                        f.modelangle_z = p.angles[2];
                        f.modelxof = p.offset[0];
                        f.modelyof = p.offset[1];
                        f.modelzoom = p.zoom;
                        f.obj_count_display = if command.ends_with("nonum") {
                            0
                        } else if command.ends_with("alwaysnum") {
                            1
                        } else {
                            2
                        };
                        if f.modelobjwidth > 0 {
                            f.modelzoom = f.modelzoom.wrapping_mul(32) / f.modelobjwidth;
                        } else if f.wsize > 0 {
                            f.modelzoom = f.modelzoom.wrapping_mul(32) / f.wsize;
                        }
                    }
                }
                "setmodelangle" => {
                    f.modelxof = i[0];
                    f.modelyof = i[1];
                    f.modelangle_x = i[2];
                    f.modelangle_y = i[3];
                    f.modelangle_z = i[4];
                    f.modelzoom = i[5];
                    update = true;
                    marks.extend([(8, 0), (10, 0)]);
                }
                "setmodelorthog" => boolean!(modelorthog),
                "setmodeltint" => {
                    f.tint_hue = i[0];
                    f.tint_saturation = i[1];
                    f.tint_luminence = i[2];
                    f.tint_weight = i[3];
                    update = true;
                }
                "setmodellighting" => {
                    f.customlighting = true;
                    f.lightDirX = i[0].clamp(0, 2816);
                    f.lightDirY = i[1].clamp(0, 2816);
                    f.lightDirZ = i[2].clamp(0, 2816);
                    f.lightColour =
                        (i[3].clamp(0, 255) << 16) | (i[4].clamp(0, 255) << 8) | i[5].clamp(0, 255);
                    f.lightExtraA = i[6];
                    f.lightExtraB = i[7];
                    f.lightExtraC = i[8];
                    f.lightExtraD = i[9];
                    update = true;
                }
                "resetmodellighting" => {
                    f.customlighting = false;
                    update = true;
                }
                "setmodelzoom" => {
                    integer!(modelzoom);
                    marks.push((8, 0));
                }
                "setmodelorigin" => {
                    f.modelorigin_x = i[0];
                    f.modelorigin_y = i[1];
                    update = true;
                }
                "setlinkactiveclanchannel" => {
                    let (affined, users) = self
                        .state
                        .active_clan_channel
                        .as_ref()
                        .context("active clan channel is null")?;
                    let index = usize::try_from(i[0])
                        .map_err(|_| anyhow::anyhow!("negative clan-channel member index"))?;
                    let name = users
                        .get(index)
                        .cloned()
                        .context("clan-channel member index outside roster")?;
                    f.link = Some(name.encode_utf16().collect());
                    c0.group_kind = Some(if *affined { 3 } else { 4 });
                    update = true;
                }
                "resetlinkplayer" => {
                    f.link = None;
                    c0.group_kind = None;
                }
                "setdragrenderbehaviour" => {
                    if (0..4).contains(&i[0]) {
                        f.dragrenderbehaviour = i[0];
                    }
                }
                "setdragdeadzone" => f.dragdeadzone = i[0],
                "setdragdeadtime" => f.dragdeadtime = i[0],
                "setopbase" => f.opbase = s[0].clone(),
                "settargetverb" => f.targetverb = s[0].clone(),
                "setpausetext" => f.pausetext = s[0].clone(),
                "settargetcursors" => {
                    f.targetCursor = i[0];
                    f.targetDefaultCursor = i[1];
                }
                "settargetopcursor" => f.targetopcursor = i[0],
                "setmouseovercursor" => f.mouseovercursor = i[0],
                "clearops" => c0.ops = None,
                "setop" => {
                    let id = i[0].wrapping_sub(1);
                    if (0..10).contains(&id) {
                        let ops = c0.ops.get_or_insert_with(Vec::new);
                        if ops.len() <= id as usize {
                            ops.resize(id as usize + 1, None);
                        }
                        ops[id as usize] = s[0].clone();
                    }
                }
                "setopcursor" => {
                    let id = i[0].wrapping_sub(1);
                    if (0..10).contains(&id) {
                        let old = c0.opname.as_ref().map(Vec::len);
                        let a = c0.opname.get_or_insert_with(Vec::new);
                        if a.len() <= id as usize {
                            a.resize(id as usize + 1, 0);
                            if let Some(old) = old {
                                a[old..id as usize].fill(-1);
                            }
                        }
                        a[id as usize] = i[1];
                    }
                }
                "setrecol" | "setretex" => {
                    if (0..5).contains(&i[0]) {
                        let a = if command == "setrecol" {
                            &mut c0.recolour
                        } else {
                            &mut c0.retexture
                        };
                        let a = a.get_or_insert(([0; 5], [0; 5]));
                        a.0[i[0] as usize] = i[1] as i16;
                        a.1[i[0] as usize] = i[2] as i16;
                        update = true;
                        marks.push((
                            if command == "setrecol" { 17 } else { 20 },
                            (i[0] as i64) << 32,
                        ));
                    }
                }
                "setparam" | "setparam_int" => {
                    let p = self.state.params.get(&i[0]).cloned().unwrap_or_default();
                    if p.integer == i[1] {
                        c0.remove_param(i[0]);
                    } else {
                        c0.set_param_int(i[0], i[1])?;
                    }
                }
                "setparam_string" => {
                    let p = self.state.params.get(&i[0]).cloned().unwrap_or_default();
                    let default = p
                        .text
                        .ok_or_else(|| anyhow::anyhow!("null default parameter string"))?;
                    if Some(&default) == s[0].as_ref() {
                        c0.remove_param(i[0]);
                    } else {
                        c0.set_param_string(i[0], s[0].clone())?;
                    }
                }
                "clearscripthooks" => {
                    f.lastVarTransmit = 0;
                    f.lastInvTransmit = 0;
                    f.lastStatTransmit = 0;
                    f.lastVarcTransmit = 0;
                    f.lastVarcstrTransmit = 0;
                    f.lastVarclanTransmit = 0;
                    f.lastRedrawCycle = -1;
                    c0.hooks.clear();
                    c0.transmits.clear();
                    c0.retained_player_transmit = None;
                }
                "setopchar"
                | "setoptchar"
                | "setopkey"
                | "setoptkey"
                | "setopkeyrate"
                | "setoptkeyrate"
                | "setopkeyignoreheld"
                | "setoptkeyignoreheld" => {
                    key_binding(&mut c0, command, i)?;
                }
                "setdraggable" => {} // Resolve after releasing the component borrow (self-links are legal).
                // No setter owner for this command: the VM's unknown-command
                // kind (the command is not routed), not a failed trap.
                _ => {
                    return Err(native910::vm::VmError::UnknownCommand {
                        command: command.into(),
                    }
                    .into())
                }
            }
        }
        if customisation_cleared {
            c.borrow_mut().npc_customisation = None;
        }
        if command == "setdraggable" {
            c.borrow_mut().draggable = if i == [-1, -1] {
                None
            } else {
                store.get(i[0], i[1])?
            };
        }
        if update {
            store.updated.push(c.clone());
        }
        if align {
            self.state.layout.align(store, interface, c)?;
        }
        if redraw && c.borrow().f.r#type == 0 {
            self.state.layout.redraw(store, interface, c, false)?;
        }
        if c.borrow().f.id == -1 && !interface.borrow().transient {
            let packed = c.borrow().f.parentlayer as i64;
            for (kind, extra) in marks {
                self.changes.push_client(kind, extra | packed, (self.now)());
            }
        }
        Ok(())
    }
    fn get(
        &self,
        store: &mut Store,
        interface: &InterfaceRef,
        c: &Ref,
        command: &str,
        named: bool,
        i: &mut Vec<i32>,
    ) -> anyhow::Result<native910::vm::Value> {
        use native910::vm::Value;
        if command == "getparentlayer" {
            return Ok(Value::Int(
                self.state
                    .layout
                    .parent(store, interface, c)?
                    .map_or(-1, |c| c.borrow().f.parentlayer),
            ));
        }
        if command == "gettargetmask" {
            return Ok(Value::Int((self.state.layout.active_mask(c) >> 11) & 127));
        }
        let c0 = c.borrow();
        let f = &c0.f;
        let v = match command {
            "getx" => f.x,
            "gety" => f.y,
            "getwidth" => f.width,
            "getheight" => f.height,
            "gethide" => {
                let hidden = if !named {
                    f.hide
                } else if self.state.layout.debug_bounds
                    && (self.state.layout.active_mask(c) != 0 || f.r#type == 0)
                {
                    false
                } else if f.clientcode == 1405 {
                    f.hide || (!self.state.debug_visible[0] && !self.state.debug_visible[1])
                } else {
                    f.hide
                };
                hidden as i32
            }
            "getlayer" => f.layer,
            "getcolour" => f.colour,
            "getscrollx" => f.scrollx,
            "getscrolly" => f.scrolly,
            "getscrollwidth" => f.scrollwidth,
            "getscrollheight" => f.scrollheight,
            "getmodelzoom" => f.modelzoom,
            "getmodelangle_x" => f.modelangle_x,
            "getmodelangle_y" => f.modelangle_y,
            "getmodelangle_z" => f.modelangle_z,
            "gettrans" => f.trans,
            "getmodelxof" => f.modelxof,
            "getmodelyof" => f.modelyof,
            "getgraphic" => f.graphic,
            "get2dangle" => f.angle2d,
            "getmodel" => {
                if f.modelkind == 1 {
                    f.model
                } else {
                    -1
                }
            }
            "getfontgraphic" | "getfontmetrics" => f.textfont,
            "getinvobject" => f.invobject,
            "getinvcount" => {
                if f.invobject == -1 {
                    0
                } else {
                    f.invcount
                }
            }
            "getid" => f.id,
            "gettext" => return vm_string(f.text.as_deref()),
            "getopbase" => return vm_string(Some(f.opbase.as_deref().unwrap_or(&[]))),
            "getop" => {
                let id = pop(i)?.wrapping_sub(1);
                let s = if let Some(ops) = &c0.ops {
                    if id >= ops.len() as i32 {
                        None
                    } else {
                        ops.get(id as usize)
                            .ok_or_else(|| anyhow::anyhow!("negative operation index"))?
                            .as_deref()
                    }
                } else {
                    None
                };
                return vm_string(Some(s.unwrap_or(&[])));
            }
            "getnextsubid" => c0.children.as_ref().map_or(0, |a| {
                let a = a.borrow();
                a.iter().position(Option::is_none).unwrap_or(a.len()) as i32
            }),
            "param" => {
                let id = pop(i)?;
                let p = self.state.params.get(&id).cloned().unwrap_or_default();
                if p.string {
                    return vm_string(c0.param_string(id, p.text.as_deref())?.as_deref());
                }
                c0.param_int(id, p.integer)?
            }
            _ => anyhow::bail!("unimplemented component getter {command}"),
        };
        Ok(Value::Int(v))
    }
}

/// The option key is slot 10; operation keys
/// use slots 0..9. Initial allocation and the combined has-keybinds scan follow
/// the original client, including a negative/zero char write on a still-unallocated component.
fn key_binding(
    c: &mut crate::ui_components::Component,
    command: &str,
    i: &[i32],
) -> anyhow::Result<()> {
    let option = command.starts_with("setopt");
    let index = if option {
        10
    } else {
        let n = i[0].wrapping_sub(1);
        anyhow::ensure!((0..10).contains(&n), "operation key index");
        n as usize
    };
    let values = if option { i } else { &i[1..] };
    fn init(c: &mut crate::ui_components::Component) {
        c.keys = Some(vec![None; 11]);
        c.key_mods = Some(vec![None; 11]);
        c.key_delays = Some(vec![0; 11]);
        c.key_rates = Some(vec![0; 11]);
        c.key_chars = Some(vec![0; 11]);
    }
    fn bound(c: &crate::ui_components::Component) -> bool {
        c.keys
            .as_ref()
            .unwrap()
            .iter()
            .zip(c.key_chars.as_ref().unwrap())
            .any(|(key, ch)| key.is_some() || *ch > 0)
    }
    match command {
        "setopchar" | "setoptchar" => {
            let value = values[0];
            if c.key_chars.is_none() {
                if value <= 0 {
                    return Ok(());
                }
                init(c);
            }
            c.key_chars.as_mut().unwrap()[index] = value;
            c.f.hasKeybinds = if value > 0 { true } else { bound(c) };
        }
        "setopkey" | "setoptkey" => {
            // if_setopkey and both option-key forms always supply one pair,
            // even when its integer key value was negative before byte narrowing.
            let count = if values.len() == 2 {
                1
            } else {
                values
                    .chunks_exact(2)
                    .take_while(|pair| pair[0] >= 0)
                    .count()
            };
            let keys = (count > 0).then(|| {
                values[..count * 2]
                    .chunks_exact(2)
                    .map(|p| p[0] as i8)
                    .collect()
            });
            let mods = (count > 0).then(|| {
                values[..count * 2]
                    .chunks_exact(2)
                    .map(|p| p[1] as i8)
                    .collect()
            });
            if c.keys.is_none() {
                if keys.is_none() {
                    return Ok(());
                }
                init(c);
            }
            c.keys.as_mut().unwrap()[index] = keys;
            c.f.hasKeybinds = if count > 0 { true } else { bound(c) };
            c.key_mods.as_mut().unwrap()[index] = mods;
        }
        "setopkeyrate" | "setoptkeyrate" => {
            let delays = c
                .key_delays
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("unallocated key delays"))?;
            let (delay, rate) = if option {
                (values[1], values[0])
            } else {
                (values[0], values[1])
            };
            delays[index] = delay;
            c.key_rates
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("unallocated key rates"))?[index] = rate;
        }
        "setopkeyignoreheld" | "setoptkeyignoreheld" => {
            let n = c
                .keys
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("unallocated keys"))?
                .len();
            c.key_next_fire.get_or_insert_with(|| vec![0; n])[index] = i32::MAX;
        }
        _ => unreachable!(),
    }
    Ok(())
}

// Portable retained hooks share this property owner with ordinary setters.
pub(crate) const RETAINED_PLAYER_HOOK: &str = "onvartransmit";
const PLAYER_LIST: &str = "onvartransmitlist";
const TRIGGER_SUFFIX: u8 = b'Y';
const STRING_ARGUMENT: u8 = b's';
const LONG_ARGUMENT: u8 = b'l';
const CLEAR_HOOK: i32 = -1;
const HEAD_VALUES: usize = 1;
const RING_CAPACITY: u32 = 64;
const RING_MASK: u32 = RING_CAPACITY - 1;

/// Trigger replacement precedes argument construction. Nonpositive counts and
/// descriptors without a suffix retain the existing vector. A nonempty vector
/// cuts the final UTF-8 byte, even when that byte is part of another character.
fn retained_descriptor(
    ints: &mut Vec<i32>,
    objects: &mut Vec<Option<String>>,
    triggers: &mut Vec<i32>,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = pop(objects)?
        .context("null retained hook descriptor")?
        .into_bytes();
    if bytes.last() == Some(&TRIGGER_SUFFIX) {
        let count = pop(ints)?;
        if count > i32::default() {
            let count = usize::try_from(count)?;
            let start = ints
                .len()
                .checked_sub(count)
                .context("retained trigger underflow")?;
            *triggers = ints.split_off(start);
        }
    }
    if !triggers.is_empty() {
        bytes.pop();
    }
    Ok(bytes)
}

pub(crate) fn install_retained_player(
    active: &mut [Active; 2],
    context: &native910::vm::InstructionContext<'_>,
    event_tokens: native910::execution::VariableEventTokens,
    ints: &mut Vec<i32>,
    objects: &mut Vec<Option<String>>,
    longs: &mut Vec<i64>,
) -> anyhow::Result<()> {
    let component = active[usize::from(context.secondary)]
        .component
        .clone()
        .context("no active component")?;
    let bytes = retained_descriptor(
        ints,
        objects,
        component
            .borrow_mut()
            .transmits
            .entry(PLAYER_LIST)
            .or_default(),
    )?;
    let mut args = vec![Arg::Null; bytes.len() + HEAD_VALUES];
    for (slot, unit) in bytes.into_iter().enumerate().rev() {
        args[slot + HEAD_VALUES] = match unit {
            STRING_ARGUMENT => pop(objects)?.map_or(Arg::Null, |value| {
                Arg::String(native910::jstr::units(&value))
            }),
            LONG_ARGUMENT => Arg::Long(pop(longs)?),
            _ => Arg::Int(pop(ints)?),
        };
    }
    let head = pop(ints)?;
    args[usize::default()] = Arg::Int(head);
    let mut component = component.borrow_mut();
    if head == CLEAR_HOOK {
        component.hooks.remove(RETAINED_PLAYER_HOOK);
    } else {
        component.hooks.insert(RETAINED_PLAYER_HOOK, args);
    }
    component.retained_player_transmit = Some(event_tokens);
    component.f.hashook = true;
    Ok(())
}

/// None leaves the cursor unchanged. Some(false) still advances the cursor;
/// Some(true) queues once, regardless of repeated matching changes in the ring.
pub(crate) fn retained_player_event(
    counter: &crate::ui_loop::Counter,
    seen: i32,
    triggers: Option<&[i32]>,
) -> Option<bool> {
    let current = counter.num as u32;
    let seen = seen as u32;
    if current <= seen {
        return None;
    }
    let triggers = triggers.unwrap_or_default();
    let delta = current.wrapping_sub(seen);
    Some(
        triggers.is_empty()
            || delta > RING_CAPACITY
            || (0..delta).any(|offset| {
                let index = (seen.wrapping_add(offset) & RING_MASK) as usize;
                triggers.contains(&counter.ids[index])
            }),
    )
}

#[cfg(test)]
pub(crate) fn verify_retained_player_recordings() {
    use crate::ui_components::Component;
    use native910::{execution::HostOperation, script::Operand};
    use serde_json::Value;
    use std::{cell::RefCell, rc::Rc};
    const INTEGER_PAYLOAD: i32 = 111;
    const LONG_PAYLOAD: i64 = i64::MAX;
    const OBJECT_PAYLOAD: &str = "captured object";
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/retained-player-transmit.json")).unwrap();
    let policy: Value = serde_json::from_str(include_str!(
        "../../../../../revisions/950/cs2/950-1-to-910.json"
    ))
    .unwrap();
    let tokens = policy["callback_policy"]["int_event_tokens"]
        .as_array()
        .unwrap();
    let event_tokens = native910::execution::VariableEventTokens::new(
        i32::try_from(tokens[0].as_i64().unwrap()).unwrap(),
        u8::try_from(tokens.len()).unwrap(),
    )
    .unwrap();
    let integers = |value: &Value| {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|value| i32::try_from(value.as_i64().unwrap()).unwrap())
            .collect::<Vec<_>>()
    };
    for case in fixture["descriptor_preprocessing"]["cases"]
        .as_array()
        .unwrap()
    {
        let input = &case["input"];
        let result = &case["result"];
        let mut ints = integers(&result["ints"]);
        ints.extend(integers(&input["new"]));
        if let Some(count) = input["count"].as_i64() {
            ints.push(i32::try_from(count).unwrap());
        }
        let mut objects = vec![Some(input["descriptor"].as_str().unwrap().into())];
        let mut triggers = integers(&input["prior"]);
        let bytes = retained_descriptor(&mut ints, &mut objects, &mut triggers).unwrap();
        assert_eq!(
            bytes
                .iter()
                .map(|byte| i32::from(*byte))
                .collect::<Vec<_>>(),
            integers(&result["descriptor"]),
            "{}",
            case["name"]
        );
        assert_eq!(ints, integers(&result["ints"]));
        assert_eq!(triggers, integers(&result["triggers"]));
        assert_eq!(objects.len() as u64, result["objects"].as_u64().unwrap());

        // The recording stops at the builder. Exercise its independently
        // modeled typed argument order and retention through the live consumer.
        let head = integers(&result["ints"])[usize::default()];
        let mut ints = vec![head];
        let mut objects = Vec::new();
        let mut longs = Vec::new();
        let mut expected = vec![Arg::Int(head)];
        for unit in &bytes {
            expected.push(match *unit {
                STRING_ARGUMENT => {
                    objects.push(Some(OBJECT_PAYLOAD.into()));
                    Arg::String(native910::jstr::units(OBJECT_PAYLOAD))
                }
                LONG_ARGUMENT => {
                    longs.push(LONG_PAYLOAD);
                    Arg::Long(LONG_PAYLOAD)
                }
                _ => {
                    ints.push(INTEGER_PAYLOAD);
                    Arg::Int(INTEGER_PAYLOAD)
                }
            });
        }
        ints.extend(integers(&input["new"]));
        if let Some(count) = input["count"].as_i64() {
            ints.push(i32::try_from(count).unwrap());
        }
        objects.push(Some(input["descriptor"].as_str().unwrap().into()));
        let operation = HostOperation::RetainedPlayerTransmit {
            event_tokens,
            pops: [
                u16::try_from(ints.len()).unwrap(),
                u16::try_from(objects.len()).unwrap(),
                u16::try_from(longs.len()).unwrap(),
            ],
        };
        assert_eq!(
            HostOperation::parse(&operation.spelling()).unwrap(),
            operation
        );
        let component = Rc::new(RefCell::new(Component::default()));
        component
            .borrow_mut()
            .transmits
            .insert(PLAYER_LIST, integers(&input["prior"]));
        let mut active = [
            Active::default(),
            Active {
                component: Some(component.clone()),
                ..Active::default()
            },
        ];
        let operand = Operand::Byte(u8::from(true));
        let context = native910::vm::InstructionContext {
            script_name: None,
            script_id: None,
            event: None,
            pc: usize::default(),
            command: operation.command(),
            operand: &operand,
            secondary: true,
            int_locals: &[],
        };
        install_retained_player(
            &mut active,
            &context,
            event_tokens,
            &mut ints,
            &mut objects,
            &mut longs,
        )
        .unwrap();
        assert_eq!(component.borrow().hooks[RETAINED_PLAYER_HOOK], expected);
        assert_eq!(
            component.borrow().transmits[PLAYER_LIST],
            integers(&result["triggers"])
        );
        assert!(
            component.borrow().retained_player_transmit.is_some() && component.borrow().f.hashook
        );
        assert!(ints.is_empty() && objects.is_empty() && longs.is_empty());
        // A clear head removes the hook while retaining its vector.
        install_retained_player(
            &mut active,
            &context,
            event_tokens,
            &mut vec![CLEAR_HOOK],
            &mut vec![Some(String::new())],
            &mut Vec::new(),
        )
        .unwrap();
        assert!(!component.borrow().hooks.contains_key(RETAINED_PLAYER_HOOK));
        assert_eq!(
            component.borrow().transmits[PLAYER_LIST],
            integers(&result["triggers"])
        );
    }
    for case in fixture["player_event_walk"]["cases"].as_array().unwrap() {
        let input = &case["input"];
        let result = &case["result"];
        let seen = input["seen"].as_u64().unwrap() as u32 as i32;
        let current = input["current"].as_u64().unwrap() as u32 as i32;
        let mut counter = crate::ui_loop::Counter {
            num: current,
            ..crate::ui_loop::Counter::default()
        };
        for (offset, value) in integers(&input["changes"]).into_iter().enumerate() {
            counter.ids[((seen as u32).wrapping_add(offset as u32) & RING_MASK) as usize] = value;
        }
        let triggers = integers(&input["triggers"]);
        let decision = if input["installed"] == false || input["suppress"] == true {
            None
        } else {
            retained_player_event(&counter, seen, Some(&triggers))
        };
        // The fixture also records other source families. This consumer owns
        // only the recorded player family.
        let queued = result["queued"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["hook"] == fixture["player_hook"]);
        assert_eq!(decision == Some(true), queued, "{}", case["name"]);
        assert_eq!(
            u64::from(if decision.is_some() { current } else { seen } as u32),
            result["seen"].as_u64().unwrap()
        );
    }
}
