//! The scalar preference queries and no-op setters of the CS2 `detail*` and
//! `autosetup_*` command families: a command pops its arguments and answers
//! from the saved options, or answers a fixed 0 for the newer detail controls
//! this client has no backend for. The commands are listed as data tables.
use super::*;

/// How a saved option is reported to scripts.
#[derive(Clone, Copy)]
enum Read {
    /// The stored value.
    Raw,
    /// 1 when the stored value equals the number, else 0.
    Equals(i32),
}

/// Commands answering a saved option: `(command, option, how)`.
const READS: &[(&str, &str, Read)] = &[
    ("getdefaultwindowmode", "windowMode", Read::Raw),
    ("detailget_brightness", "brightness", Read::Raw),
    ("detailget_removeroofs_option", "removeRoofs", Read::Raw),
    (
        "detailget_grounddecor_on",
        "groundDecoration",
        Read::Equals(1),
    ),
    ("detailget_idleanims_many", "idleAnimations", Read::Raw),
    (
        "detailget_flickering_on",
        "flickeringEffects",
        Read::Equals(1),
    ),
    (
        "detailget_spotshadows_on",
        "characterShadows",
        Read::Equals(1),
    ),
    ("detailget_hardshadows", "sceneryShadows", Read::Raw),
    ("detailget_shadowquality", "shadowQuality", Read::Raw),
    (
        "detailget_lightdetail_high",
        "lightingDetail",
        Read::Equals(1),
    ),
    ("detailget_waterdetail_high", "waterDetail", Read::Equals(2)),
    ("detailget_fog_on", "fog", Read::Equals(1)),
    ("detailget_antialiasing", "antiAliasing2", Read::Raw),
    ("detailget_stereo", "stereo", Read::Equals(1)),
    ("detailget_soundvol", "soundVolume", Read::Raw),
    ("detailget_musicvol", "unknownVolume1", Read::Raw),
    ("detailget_bgsoundvol", "backgroundSoundVolume", Read::Raw),
    ("detailget_antialiasing_default", "antiAliasing", Read::Raw),
    ("detailget_buildarea", "buildArea", Read::Raw),
    ("detailget_bloom", "bloom", Read::Equals(1)),
    ("detailget_customcursors", "customCursors", Read::Equals(1)),
    ("detailget_idleanims", "idleAnimations", Read::Raw),
    (
        "detailget_groundblending",
        "groundBlending",
        Read::Equals(1),
    ),
    ("detailget_toolkit", "displayMode", Read::Raw),
    ("detailget_toolkit_default", "toolkit", Read::Raw),
    ("detailget_cpuusage", "cpuUsage", Read::Raw),
    ("detailget_texturing", "textures", Read::Equals(1)),
    ("detailget_maxscreensize", "screenSize", Read::Raw),
    ("detailget_speechvol", "speechVolume", Read::Raw),
    ("detailget_loginvol", "unknownVolume2", Read::Raw),
    ("detailget_loadingscreentype", "loadingScreen", Read::Raw),
    ("detailget_orthographic", "orthographic", Read::Raw),
    ("detailget_drawdistance", "drawDistance", Read::Raw),
    ("detailget_skydetail", "skyboxes", Read::Raw),
    ("detailget_animdetail", "animDetail", Read::Raw),
    ("autosetup_getlevel", "preset", Read::Raw),
];

/// `detailcanmod_*` commands: whether the option can be modified.
const CAN_MOD: &[(&str, &str)] = &[
    ("detailcanmod_grounddecor", "groundDecoration"),
    ("detailcanmod_spotshadows", "characterShadows"),
    ("detailcanmod_hardshadows", "sceneryShadows"),
    ("detailcanmod_shadowquality", "shadowQuality"),
    ("detailcanmod_waterdetail", "waterDetail"),
    ("detailcanmod_particles", "particles"),
    ("detailcanmod_buildarea", "buildArea"),
    ("detailcanmod_groundblending", "groundBlending"),
    ("detailcanmod_texturing", "textures"),
    ("detailcanmod_maxscreensize", "screenSize"),
    ("detailcanmod_fog", "fog"),
    ("detailcanmod_orthographic", "orthographic"),
    ("detailcanmod_toolkit_default", "toolkit"),
    ("detailcanmod_skydetail", "skyboxes"),
    ("detailcanmod_animdetail", "animDetail"),
];

/// `detailcanset_*` commands: pops a value and answers whether the option
/// accepts it.
const CAN_SET: &[(&str, &str)] = &[
    ("detailcanset_grounddecor", "groundDecoration"),
    ("detailcanset_spotshadows", "characterShadows"),
    ("detailcanset_hardshadows", "sceneryShadows"),
    ("detailcanset_shadowquality", "shadowQuality"),
    ("detailcanset_waterdetail", "waterDetail"),
    ("detailcanset_particles", "particles"),
    ("detailcanset_buildarea", "buildArea"),
    ("detailcanset_groundblending", "groundBlending"),
    ("detailcanset_texturing", "textures"),
    ("detailcanset_maxscreensize", "screenSize"),
    ("detailcanset_fog", "fog"),
    ("detailcanset_orthographic", "orthographic"),
    ("detailcanset_toolkit_default", "toolkit"),
    ("detailcanset_skydetail", "skyboxes"),
    ("detailcanset_animdetail", "animDetail"),
];

/// Setters with no backend: they pop one int and change nothing.
const IGNORED_SETTERS: &[&str] = &[
    "detail_drawdistance",
    "detail_diskcachesize",
    "detail_shadows",
    "detail_lightingquality",
    "detail_antialiasingmode",
    "detail_ambientocclusion",
    "detail_reflections",
    "detail_vsync",
    "detail_anisotropicfiltering",
    "detail_volumetriclighting",
    "detail_maxforegroundfps",
    "detail_maxbackgroundfps",
    "detail_gamerenderscale",
    "detail_interfacescale",
    "detail_dof",
];

/// Queries of unsupported controls: answer 0 and pop nothing.
const ANSWER_ZERO: &[&str] = &[
    "detailget_diskcachesize",
    "detailget_mindiskcachesize",
    "detailget_maxdiskcachesize",
    "detailget_recommendeddiskcachesize",
    "detailget_shadows",
    "detailget_lightingquality",
    "detailget_antialiasingmode",
    "detailget_ambientocclusion",
    "detailget_reflections",
    "detailget_vsync",
    "detailget_anisotropicfiltering",
    "detailget_volumetriclighting",
    "detailget_maxforegroundfps",
    "detailget_maxbackgroundfps",
    "detailget_gamerenderscale",
    "detailget_interfacescale",
    "detailget_dof",
    "detailcanmod_shadows",
    "detailcanmod_lightingquality",
    "detailcanmod_antialiasingmode",
    "detailcanmod_ambientocclusion",
    "detailcanmod_reflections",
    "detailcanmod_vsync",
    "detailcanmod_anisotropicfiltering",
    "detailcanmod_volumetriclighting",
    "detailcanmod_maxforegroundfps",
    "detailcanmod_maxbackgroundfps",
    "detailcanmod_gamerenderscale",
    "detailcanmod_interfacescale",
    "detailcanmod_dof",
    "detailcanset_shadows",
    "detailcanset_lightingquality",
    "detailcanset_antialiasingmode",
    "detailcanset_ambientocclusion",
    "detailcanset_reflections",
    "detailcanset_vsync",
    "detailcanset_anisotropicfiltering",
    "detailcanset_volumetriclighting",
    "detailcanset_maxforegroundfps",
    "detailcanset_maxbackgroundfps",
];

/// Capability checks of unsupported controls: pop one int and answer 0.
const POP_AND_ANSWER_ZERO: &[&str] = &[
    "detailcanset_gamerenderscale",
    "detailcanset_interfacescale",
    "detailcanset_dof",
];

impl Preferences {
    pub(super) fn query(
        &self,
        command: &str,
        ints: &mut Vec<i32>,
    ) -> Option<VmResult<Option<Value>>> {
        let int = |value: i32| Ok(Some(Value::Int(value)));
        if command == "autosetup_setultra" {
            return Some(Ok(None));
        }
        if IGNORED_SETTERS.contains(&command) {
            return Some(pop(ints).map(|_| None));
        }
        if let Some(&(_, key, how)) = READS.iter().find(|(name, _, _)| *name == command) {
            let stored = self.options.get(key).unwrap();
            return Some(int(match how {
                Read::Raw => stored,
                Read::Equals(wanted) => i32::from(stored == wanted),
            }));
        }
        if ANSWER_ZERO.contains(&command) {
            return Some(int(0));
        }
        if POP_AND_ANSWER_ZERO.contains(&command) {
            return Some(pop(ints).and_then(|_| int(0)));
        }
        if let Some(&(_, key)) = CAN_MOD.iter().find(|(name, _)| *name == command) {
            return Some(int(i32::from(self.options.can_mod(key).unwrap())));
        }
        if let Some(&(_, key)) = CAN_SET.iter().find(|(name, _)| *name == command) {
            return Some(
                pop(ints).and_then(|value| int(self.options.can_set(key, value).unwrap())),
            );
        }
        None
    }
}
