//! Client options: the persisted graphics, audio and behaviour settings.
//!
//! Each option is a numbered slot with a name in [`FIELDS`], a default, a
//! range clamp and an "is this value allowed here" rule that depends on the
//! machine [`Profile`] and on other slots. The byte format and the local file
//! lifecycle live in `client_options_codec.rs`, the quality presets in
//! `client_options_presets.rs`, the typed view the live consumers read in
//! `client_options_live.rs`. Slot numbers are fixed by the persisted
//! format, so the per-slot rule tables below are indexed by slot.
/// The machine facts the option rules read.
///
/// The DirectX toolkit (display mode 3) is unavailable by design: the client
/// draws through one wgpu device, and its GL toolkit is the hardware toolkit
/// on every operating system. The two facts that turn DirectX on in the
/// original client are therefore constant here: `windows` (which makes 3 the
/// default toolkit and adds the DirectX probe to the auto-setup) and `jagdx`
/// (the native library that makes toolkit 3 selectable). [`Profile::default`]
/// is the profile the client runs with on every operating system; the fields
/// stay inputs so the rules can be checked for a machine that has DirectX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub max_memory_mb: i32,
    pub cpu_count: i32,
    pub arm: bool,
    pub windows: bool,
    pub unused: bool,
    pub mode_game: i32,
    pub jagdx: bool,
    pub initial_display_mode: i32,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            max_memory_mb: 512,
            cpu_count: 2,
            arm: std::env::consts::ARCH.contains("arm"),
            windows: false,
            unused: false,
            mode_game: 0,
            jagdx: false,
            initial_display_mode: 0,
        }
    }
}
fn accelerated(mode: i32) -> bool {
    matches!(mode, 1 | 3 | 5)
}
fn software(mode: i32) -> bool {
    matches!(mode, 0 | 2)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientOptions {
    pub particle_level: i32,
    pub values: [i32; 55],
    pub profile: Profile,
    pub display_flags: [bool; 55],
    pub defaulted: [bool; 55],
    pub(crate) present: [bool; 55],
}
pub const FIELDS: [&str; 55] = [
    "animDetail",
    "antiAliasing",
    "antiAliasing2",
    "unused",
    "bloom",
    "brightness",
    "buildArea",
    "consoleKeyPress",
    "drawDistance",
    "flickeringEffects",
    "fog",
    "groundBlending",
    "groundDecoration",
    "idleAnimations",
    "lightingDetail",
    "sceneryShadows",
    "shadowQuality",
    "orthographic",
    "particles",
    "removeRoofs",
    "removeRoofs2",
    "screenSize",
    "skyboxes",
    "characterShadows",
    "textures",
    "toolkit",
    "displayMode",
    "waterDetail",
    "windowMode",
    "maxScreenSize2",
    "unused4",
    "unused1",
    "unused2",
    "unused3",
    "unused5",
    "unused6",
    "unused7",
    "unused8",
    "unused9",
    "unused10",
    "unused11",
    "unused12",
    "customCursors",
    "preset",
    "cpuUsage",
    "loadingScreen",
    "safeMode",
    "unknown7",
    "unused13",
    "soundVolume",
    "backgroundSoundVolume",
    "speechVolume",
    "unknownVolume1",
    "unknownVolume2",
    "stereo",
];
// The per-slot rule tables keep one arm per slot, with explicit `return`s,
// parenthesised conditions and arms with identical bodies, so each slot's rule
// can be read and changed on its own.
#[allow(
    unused_parens,
    unused_variables,
    clippy::if_same_then_else,
    clippy::needless_return,
    reason = "one self-contained rule per slot"
)]
impl ClientOptions {
    pub fn field_index(name: &str) -> Option<usize> {
        FIELDS.iter().position(|field| *field == name)
    }
    pub fn get(&self, name: &str) -> Option<i32> {
        Self::field_index(name).map(|i| self.values[i])
    }
    pub fn new(profile: Profile) -> Self {
        let mut out = Self {
            particle_level: 0,
            values: [0; 55],
            profile,
            display_flags: [true; 55],
            defaulted: [false; 55],
            present: [false; 55],
        };
        out.assign("displayMode", profile.initial_display_mode);
        out.set_defaults(true, true);
        out
    }
    pub(crate) fn assign(&mut self, name: &str, value: i32) {
        let slot = Self::field_index(name).expect("unknown option field");
        if name == "particles" {
            self.particle_level = if (0..=2).contains(&value) { value } else { 0 };
        }
        self.values[slot] = if name == "customCursors" { 1 } else { value };
        self.present[slot] = true;
        self.display_flags[slot] = true;
        self.defaulted[slot] = false;
    }
    pub fn set_field(&mut self, name: &str, value: i32) -> Option<bool> {
        let slot = Self::field_index(name)?;
        let accepted = self.can_set_slot(slot, value) != 3;
        if accepted {
            if name == "particles" {
                self.particle_level = if (0..=2).contains(&value) { value } else { 0 };
            }
            self.values[slot] = value;
            self.defaulted[slot] = false;
        }
        self.clamp();
        Some(accepted)
    }
    pub fn set_display_flag(&mut self, name: &str, value: bool) -> Option<()> {
        let slot = Self::field_index(name)?;
        if !matches!(name, "displayMode" | "toolkit") {
            return None;
        }
        self.display_flags[slot] = value;
        self.clamp();
        Some(())
    }
    pub fn default_value(&mut self, slot: usize) -> i32 {
        match slot {
            0 => {
                return 1;
            }
            1 => {
                return 0;
            }
            2 => {
                return 0;
            }
            3 => {
                return 1;
            }
            4 => {
                return 0;
            }
            5 => {
                return 3;
            }
            6 => {
                return if self.profile.unused { 5 } else { 0 };
            }
            7 => {
                return 1;
            }
            8 => {
                return if self.profile.unused { 1 } else { 0 };
            }
            9 => {
                return 1;
            }
            10 => {
                return 1;
            }
            11 => {
                return 1;
            }
            12 => {
                return 1;
            }
            13 => {
                return 1;
            }
            14 => {
                return 1;
            }
            15 => {
                return 2;
            }
            16 => {
                return 1;
            }
            17 => {
                return 0;
            }
            18 => {
                return if self.profile.max_memory_mb < 245 {
                    0
                } else {
                    2
                };
            }
            19 => {
                return 2;
            }
            20 => {
                return 2;
            }
            21 => {
                return if self.display_flags[26] && software(self.values[26]) {
                    1
                } else {
                    0
                };
            }
            22 => {
                return 1;
            }
            23 => {
                return 1;
            }
            24 => {
                return 1;
            }
            25 => {
                self.defaulted[slot] = true;
                return if self.profile.windows { 3 } else { 1 };
            }
            26 => {
                self.defaulted[slot] = true;
                return if self.profile.windows { 3 } else { 1 };
            }
            27 => {
                return 1;
            }
            28 => {
                return if self.profile.arm { 3 } else { 2 };
            }
            29 => {
                return if self.profile.arm { 3 } else { 2 };
            }
            30 => {
                return 0;
            }
            31 => {
                return 1;
            }
            32 => {
                return 2;
            }
            33 => {
                return 0;
            }
            34 => {
                return 1;
            }
            35 => {
                return if self.profile.unused { 0 } else { 1 };
            }
            36 => {
                return 0;
            }
            37 => {
                return 0;
            }
            38 => {
                return 70;
            }
            39 => {
                return 30;
            }
            40 => {
                return 100;
            }
            41 => {
                return 100;
            }
            42 => {
                return 1;
            }
            43 => {
                return 0;
            }
            44 => {
                return if self.profile.cpu_count > 1 { 4 } else { 2 };
            }
            45 => {
                return 0;
            }
            46 => {
                return 0;
            }
            47 => {
                let toolkit = self.values[25];
                return if toolkit == 3 || toolkit == 5 { 0 } else { 0 };
            }
            48 => {
                return -2;
            }
            49 => {
                return 127;
            }
            50 => {
                return 127;
            }
            51 => {
                return 127;
            }
            52 => {
                return 127;
            }
            53 => {
                return 127;
            }
            54 => {
                return 1;
            }
            _ => panic!("preference slot"),
        }
    }
    pub fn can_set(&self, name: &str, value: i32) -> Option<i32> {
        Self::field_index(name).map(|slot| self.can_set_slot(slot, value))
    }
    fn can_set_slot(&self, slot: usize, candidate: i32) -> i32 {
        match slot {
            0 => {
                return 1;
            }
            1 => {
                return if accelerated(self.values[26]) { 1 } else { 3 };
            }
            2 => {
                return if accelerated(self.values[26]) { 1 } else { 3 };
            }
            3 => {
                return if accelerated(self.values[26]) { 1 } else { 3 };
            }
            4 => {
                return if accelerated(self.values[26]) { 1 } else { 3 };
            }
            5 => {
                return 1;
            }
            6 => {
                if (self.profile.unused) {
                    return 3;
                }
                let max_memory = self.profile.max_memory_mb;
                if (max_memory < 245) {
                    return 3;
                } else if (3 == candidate && max_memory < 500) {
                    return 3;
                } else {
                    return 1;
                }
            }
            7 => {
                return 1;
            }
            8 => {
                return 1;
            }
            9 => {
                return 1;
            }
            10 => {
                if (candidate != 0 && self.values[17] == 2) {
                    return 3;
                } else if (candidate == 0 || self.values[11] == 1) {
                    return 1;
                } else {
                    return 2;
                }
            }
            11 => {
                if (self.profile.mode_game != 0) {
                    return 3;
                }
                if (candidate == 0) {
                    if (self.values[10] == 1) {
                        return 2;
                    }
                    if (self.values[24] == 1) {
                        return 2;
                    }
                    if (self.values[27] > 0) {
                        return 2;
                    }
                }
                return 1;
            }
            12 => {
                return if self.profile.mode_game == 0 { 1 } else { 3 };
            }
            13 => {
                return 1;
            }
            14 => {
                return 1;
            }
            15 => {
                return if self.values[24] == 0 { 3 } else { 1 };
            }
            16 => {
                return 1;
            }
            17 => {
                return if candidate == 2 { 2 } else { 1 };
            }
            18 => {
                return if self.profile.max_memory_mb < 245 {
                    3
                } else {
                    1
                };
            }
            19 => {
                return 1;
            }
            20 => {
                return 1;
            }
            21 => {
                return 1;
            }
            22 => {
                return if accelerated(self.values[26]) { 1 } else { 3 };
            }
            23 => {
                return 1;
            }
            24 => {
                if (self.profile.mode_game == 0) {
                    return if candidate == 0 || self.values[11] == 1 {
                        1
                    } else {
                        2
                    };
                } else {
                    return 3;
                }
            }
            25 => {
                return if candidate == 3 && !self.profile.jagdx {
                    3
                } else {
                    2
                };
            }
            26 => {
                return if candidate == 3 && !self.profile.jagdx {
                    3
                } else {
                    2
                };
            }
            27 => {
                return if candidate == 0 || self.values[11] == 1 {
                    1
                } else {
                    2
                };
            }
            28 => {
                return 1;
            }
            29 => {
                return 1;
            }
            30 => {
                return 3;
            }
            31 => {
                return 3;
            }
            32 => {
                return 3;
            }
            33 => {
                return 3;
            }
            34 => {
                return 3;
            }
            35 => {
                return 3;
            }
            36 => {
                return 3;
            }
            37 => {
                return 3;
            }
            38 => {
                return 3;
            }
            39 => {
                return 3;
            }
            40 => {
                return 3;
            }
            41 => {
                return 3;
            }
            42 => {
                return 1;
            }
            43 => {
                return 1;
            }
            44 => {
                return 1;
            }
            45 => {
                return 1;
            }
            46 => {
                return 1;
            }
            47 => {
                if (candidate == 0) {
                    return 1;
                } else if (self.profile.cpu_count < 2) {
                    return 3;
                } else {
                    let toolkit = self.values[25];
                    return if toolkit == 3 || toolkit == 5 { 1 } else { 3 };
                }
            }
            48 => {
                return 3;
            }
            49 => {
                return 1;
            }
            50 => {
                return 1;
            }
            51 => {
                return 1;
            }
            52 => {
                return 1;
            }
            53 => {
                return 1;
            }
            54 => {
                return 1;
            }
            _ => panic!("preference slot"),
        }
    }
    pub fn can_mod(&self, name: &str) -> Option<bool> {
        let slot = Self::field_index(name)?;
        Some(match slot {
            0 => self.can_mod_0(),
            1 => self.can_mod_1(),
            2 => self.can_mod_2(),
            4 => self.can_mod_4(),
            6 => self.can_mod_6(),
            10 => self.can_mod_10(),
            11 => self.can_mod_11(),
            12 => self.can_mod_12(),
            15 => self.can_mod_15(),
            16 => self.can_mod_16(),
            17 => self.can_mod_17(),
            18 => self.can_mod_18(),
            21 => self.can_mod_21(),
            22 => self.can_mod_22(),
            23 => self.can_mod_23(),
            24 => self.can_mod_24(),
            25 => self.can_mod_25(),
            26 => self.can_mod_26(),
            27 => self.can_mod_27(),
            _ => return None,
        })
    }
    fn can_mod_0(&self) -> bool {
        return true;
    }
    fn can_mod_1(&self) -> bool {
        return accelerated(self.values[26]);
    }
    fn can_mod_2(&self) -> bool {
        return accelerated(self.values[26]);
    }
    fn can_mod_4(&self) -> bool {
        return accelerated(self.values[26]);
    }
    fn can_mod_6(&self) -> bool {
        if (self.profile.unused) {
            return false;
        } else {
            let max_memory = self.profile.max_memory_mb;
            return max_memory >= 245;
        }
    }
    fn can_mod_10(&self) -> bool {
        return true;
    }
    fn can_mod_11(&self) -> bool {
        return self.profile.mode_game == 0;
    }
    fn can_mod_12(&self) -> bool {
        return self.profile.mode_game == 0;
    }
    fn can_mod_15(&self) -> bool {
        return self.values[24] != 0;
    }
    fn can_mod_16(&self) -> bool {
        return true;
    }
    fn can_mod_17(&self) -> bool {
        return true;
    }
    fn can_mod_18(&self) -> bool {
        return self.profile.max_memory_mb >= 245;
    }
    fn can_mod_21(&self) -> bool {
        return true;
    }
    fn can_mod_22(&self) -> bool {
        return accelerated(self.values[26]);
    }
    fn can_mod_23(&self) -> bool {
        return true;
    }
    fn can_mod_24(&self) -> bool {
        return self.profile.mode_game == 0;
    }
    fn can_mod_25(&self) -> bool {
        return true;
    }
    fn can_mod_26(&self) -> bool {
        return true;
    }
    fn can_mod_27(&self) -> bool {
        return true;
    }
    pub fn clamp(&mut self) {
        self.clamp_one(0);
        self.clamp_one(1);
        self.clamp_one(3);
        self.clamp_one(2);
        self.clamp_one(4);
        self.clamp_one(5);
        self.clamp_one(6);
        self.clamp_one(8);
        self.clamp_one(9);
        self.clamp_one(10);
        self.clamp_one(11);
        self.clamp_one(12);
        self.clamp_one(13);
        self.clamp_one(14);
        self.clamp_one(15);
        self.clamp_one(16);
        self.clamp_one(17);
        self.clamp_one(18);
        self.clamp_one(19);
        self.clamp_one(20);
        self.clamp_one(21);
        self.clamp_one(22);
        self.clamp_one(23);
        self.clamp_one(24);
        self.clamp_one(25);
        self.clamp_one(26);
        self.clamp_one(27);
        self.clamp_one(28);
        self.clamp_one(29);
        self.clamp_one(30);
        self.clamp_one(31);
        self.clamp_one(32);
        self.clamp_one(33);
        self.clamp_one(34);
        self.clamp_one(35);
        self.clamp_one(36);
        self.clamp_one(37);
        self.clamp_one(38);
        self.clamp_one(39);
        self.clamp_one(40);
        self.clamp_one(41);
        self.clamp_one(42);
        self.clamp_one(43);
        self.clamp_one(44);
        self.clamp_one(45);
        self.clamp_one(46);
        self.clamp_one(47);
        self.clamp_one(48);
        self.clamp_one(7);
        self.clamp_one(49);
        self.clamp_one(50);
        self.clamp_one(51);
        self.clamp_one(52);
        self.clamp_one(53);
        self.clamp_one(54);
    }
    fn clamp_one(&mut self, slot: usize) {
        match slot {
            0 => {
                if (self.values[slot] != 1 && self.values[slot] != 0) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            1 => {
                if (self.display_flags[26] && !accelerated(self.values[26])) {
                    self.values[slot] = 0;
                }
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            2 => {
                if (self.display_flags[26] && !accelerated(self.values[26])) {
                    self.values[slot] = 0;
                }
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            3 => {
                if (self.values[slot] < 0 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            4 => {
                if (self.display_flags[26] && !accelerated(self.values[26])) {
                    self.values[slot] = 0;
                }
                if (self.profile.unused) {
                    if (self.values[slot] < 0 || self.values[slot] > 3) {
                        self.values[slot] = self.default_value(slot);
                    }
                } else if (self.values[slot] < 0 || self.values[slot] > 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            5 => {
                if (self.values[slot] < 0 || self.values[slot] > 4) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            6 => {
                if (self.profile.unused) {
                    self.values[slot] = 5;
                    return;
                }
                let max_memory = self.profile.max_memory_mb;
                if (max_memory < 245) {
                    self.values[slot] = 0;
                }
                if (3 == self.values[slot] && max_memory < 500) {
                    self.values[slot] = 2;
                }
                if (self.values[slot] < 0 || self.values[slot] > 4) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            7 => {
                if (self.values[slot] != 0 && self.values[slot] != 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            8 => {
                if (self.profile.unused) {
                    if (self.values[slot] < 0 || self.values[slot] > 3) {
                        self.values[slot] = self.default_value(slot);
                    }
                } else if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            9 => {
                if (self.values[slot] != 1 && self.values[slot] != 0) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            10 => {
                if (self.values[slot] != 0 && self.values[11] != 1) {
                    self.values[slot] = 0;
                }
                if (self.values[slot] != 0 && self.values[17] == 2) {
                    self.values[slot] = 0;
                }
                if (self.values[slot] < 0 || self.values[slot] > 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            11 => {
                if (self.profile.mode_game != 0) {
                    self.values[slot] = 1;
                }
                if (self.values[slot] != 0 && self.values[slot] != 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            12 => {
                if (self.profile.mode_game != 0) {
                    self.values[slot] = 1;
                }
                if (self.values[slot] != 0 && self.values[slot] != 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            13 => {
                if (self.profile.mode_game == 1) {
                    self.values[slot] = 2;
                }
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            14 => {
                if (self.values[slot] != 1 && self.values[slot] != 0) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            15 => {
                if (self.values[24] == 0) {
                    self.values[slot] = 0;
                }
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            16 => {
                if (self.values[slot] < 0 || self.values[slot] > 4) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            17 => {
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            18 => {
                if (self.profile.max_memory_mb < 245) {
                    self.values[slot] = 0;
                }
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            19 => {
                if (self.values[17] == 2 && self.values[slot] == 2) {
                    self.values[slot] = 1;
                }
                if (self.values[slot] < 0 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            20 => {
                if (self.values[17] == 2 && self.values[slot] == 2) {
                    self.values[slot] = 1;
                }
                if (self.values[slot] < 0 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            21 => {
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            22 => {
                if (self.values[slot] < 0 || self.values[slot] > 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            23 => {
                if (self.values[slot] != 1 && self.values[slot] != 0) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            24 => {
                if (self.profile.mode_game != 0) {
                    self.values[slot] = 1;
                }
                if (self.profile.unused) {
                    if (self.values[slot] < 0 || self.values[slot] > 2) {
                        self.values[slot] = self.default_value(slot);
                    }
                } else if (self.values[slot] != 0 && self.values[slot] != 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            25 => {
                if (self.values[slot] < 0 || self.values[slot] > 5 || self.values[slot] == 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            26 => {
                if (self.values[slot] < 0 || self.values[slot] > 5 || self.values[slot] == 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            27 => {
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            28 => {
                if (self.values[slot] < 1 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            29 => {
                if (self.values[slot] < 1 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            30 => {
                if (self.values[slot] < 0 || self.values[slot] > 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            31 => {
                if (self.values[slot] < 0 || self.values[slot] > 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            32 => {
                if (self.values[slot] < 0 || self.values[slot] != 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            33 => {
                if (self.values[slot] < 0 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            34 => {
                if (self.values[slot] < 0 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            35 => {
                if (!self.profile.unused) {
                    self.values[slot] = self.default_value(slot);
                } else if (self.values[slot] < -1 || self.values[slot] > 3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            36 => {
                if (self.values[slot] < 0 || self.values[slot] > 4) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            37 => {
                if (self.values[slot] < 0 || self.values[slot] > 4) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            38 => {
                self.values[slot] = self.values[slot].clamp(5, 300);
            }
            39 => {
                self.values[slot] = self.values[slot].clamp(5, 300);
            }
            40 => {
                if (self.values[slot] < 33 || self.values[slot] > 200) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            41 => {
                if (self.values[slot] < 33 || self.values[slot] > 400) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            42 => {
                if (self.values[slot] != 1 && self.values[slot] != 0) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            43 => {
                if (self.values[slot] < 0 || self.values[slot] > 4) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            44 => {
                if (self.values[slot] < 0 || self.values[slot] > 4) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            45 => {}
            46 => {
                if (self.values[slot] < 0 || self.values[slot] > 5 || self.values[slot] == 2) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            47 => {
                let toolkit = self.values[25];
                if (toolkit != 3 && toolkit != 5) {
                    self.values[slot] = 0;
                }
                if (self.profile.cpu_count < 2) {
                    self.values[slot] = 0;
                }
                if (self.values[slot] != 0 && self.values[slot] != 1) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            48 => {
                if (self.values[slot] < -3) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            49 => {
                if (self.values[slot] < 0 || self.values[slot] > 255) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            50 => {
                if (self.values[slot] < 0 || self.values[slot] > 255) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            51 => {
                if (self.values[slot] < 0 || self.values[slot] > 255) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            52 => {
                if (self.values[slot] < 0 || self.values[slot] > 255) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            53 => {
                if (self.values[slot] < 0 || self.values[slot] > 255) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            54 => {
                if (self.values[slot] != 1 && self.values[slot] != 0) {
                    self.values[slot] = self.default_value(slot);
                }
            }
            _ => panic!("preference slot"),
        }
    }
    pub fn set_defaults(&mut self, reset: bool, audio: bool) {
        if reset || !self.present[0] {
            let value = self.default_value(0);
            self.assign("animDetail", value);
        }
        if reset || !self.present[1] {
            let value = self.default_value(1);
            self.assign("antiAliasing", value);
        }
        if reset || !self.present[3] {
            let value = self.default_value(3);
            self.assign("unused", value);
        }
        if reset || !self.present[2] {
            let value = self.values[1];
            self.assign("antiAliasing2", value);
        }
        if reset || !self.present[4] {
            let value = self.default_value(4);
            self.assign("bloom", value);
        }
        if reset || !self.present[5] {
            let value = self.default_value(5);
            self.assign("brightness", value);
        }
        if reset || !self.present[6] {
            let value = self.default_value(6);
            self.assign("buildArea", value);
        }
        if reset || !self.present[8] {
            let value = self.default_value(8);
            self.assign("drawDistance", value);
        }
        if reset || !self.present[9] {
            let value = self.default_value(9);
            self.assign("flickeringEffects", value);
        }
        if reset || !self.present[10] {
            let value = self.default_value(10);
            self.assign("fog", value);
        }
        if reset || !self.present[11] {
            let value = self.default_value(11);
            self.assign("groundBlending", value);
        }
        if reset || !self.present[12] {
            let value = self.default_value(12);
            self.assign("groundDecoration", value);
        }
        if reset || !self.present[13] {
            let value = self.default_value(13);
            self.assign("idleAnimations", value);
        }
        if reset || !self.present[14] {
            let value = self.default_value(14);
            self.assign("lightingDetail", value);
        }
        if reset || !self.present[15] {
            let value = self.default_value(15);
            self.assign("sceneryShadows", value);
        }
        if reset || !self.present[16] {
            let value = self.default_value(16);
            self.assign("shadowQuality", value);
        }
        if reset || !self.present[17] {
            let value = self.default_value(17);
            self.assign("orthographic", value);
        }
        if reset || !self.present[18] {
            let value = self.default_value(18);
            self.assign("particles", value);
        }
        if reset || !self.present[19] {
            let value = self.default_value(19);
            self.assign("removeRoofs", value);
        }
        if reset || !self.present[20] {
            let value = self.values[19];
            self.assign("removeRoofs2", value);
        }
        if reset || !self.present[21] {
            let value = self.default_value(21);
            self.assign("screenSize", value);
        }
        if reset || !self.present[22] {
            let value = self.default_value(22);
            self.assign("skyboxes", value);
        }
        if reset || !self.present[23] {
            let value = self.default_value(23);
            self.assign("characterShadows", value);
        }
        if reset || !self.present[24] {
            let value = self.default_value(24);
            self.assign("textures", value);
        }
        if reset || !self.present[25] {
            let value = self.default_value(25);
            self.assign("toolkit", value);
        }
        if reset || !self.present[26] {
            let value = self.values[25];
            self.assign("displayMode", value);
        }
        if reset || !self.present[27] {
            let value = self.default_value(27);
            self.assign("waterDetail", value);
        }
        if reset || !self.present[28] {
            let value = self.default_value(28);
            self.assign("windowMode", value);
        }
        if reset || !self.present[29] {
            let value = self.values[28];
            self.assign("maxScreenSize2", value);
        }
        if reset || !self.present[30] {
            let value = self.default_value(30);
            self.assign("unused4", value);
        }
        if reset || !self.present[31] {
            let value = self.default_value(31);
            self.assign("unused1", value);
        }
        if reset || !self.present[32] {
            let value = self.default_value(32);
            self.assign("unused2", value);
        }
        if reset || !self.present[33] {
            let value = self.default_value(33);
            self.assign("unused3", value);
        }
        if reset || !self.present[34] {
            let value = self.default_value(34);
            self.assign("unused5", value);
        }
        if reset || !self.present[35] {
            let value = self.default_value(35);
            self.assign("unused6", value);
        }
        if reset || !self.present[36] {
            let value = self.default_value(36);
            self.assign("unused7", value);
        }
        if reset || !self.present[37] {
            let value = self.default_value(37);
            self.assign("unused8", value);
        }
        if reset || !self.present[38] {
            let value = self.default_value(38);
            self.assign("unused9", value);
        }
        if reset || !self.present[39] {
            let value = self.default_value(39);
            self.assign("unused10", value);
        }
        if reset || !self.present[40] {
            let value = self.default_value(40);
            self.assign("unused11", value);
        }
        if reset || !self.present[41] {
            let value = self.default_value(41);
            self.assign("unused12", value);
        }
        if reset || !self.present[42] {
            let value = self.default_value(42);
            self.assign("customCursors", value);
        }
        if reset || !self.present[43] {
            let value = self.default_value(43);
            self.assign("preset", value);
        }
        if reset || !self.present[44] {
            let value = self.default_value(44);
            self.assign("cpuUsage", value);
        }
        if reset || !self.present[45] {
            let value = self.default_value(45);
            self.assign("loadingScreen", value);
        }
        if reset || !self.present[46] {
            let value = self.default_value(46);
            self.assign("safeMode", value);
        }
        if reset || !self.present[47] {
            let value = self.default_value(47);
            self.assign("unknown7", value);
        }
        if reset || !self.present[48] {
            let value = self.default_value(48);
            self.assign("unused13", value);
        }
        if reset || !self.present[7] {
            let value = self.default_value(7);
            self.assign("consoleKeyPress", value);
        }
        if reset || !self.present[49] || audio {
            let value = self.default_value(49);
            self.assign("soundVolume", value);
        }
        if reset || !self.present[50] || audio {
            let value = self.default_value(50);
            self.assign("backgroundSoundVolume", value);
        }
        if reset || !self.present[51] || audio {
            let value = self.default_value(51);
            self.assign("speechVolume", value);
        }
        if reset || !self.present[52] || audio {
            let value = self.default_value(52);
            self.assign("unknownVolume1", value);
        }
        if reset || !self.present[53] || audio {
            let value = self.default_value(53);
            self.assign("unknownVolume2", value);
        }
        if reset || !self.present[54] || audio {
            let value = self.default_value(54);
            self.assign("stereo", value);
        }
    }
}
// The per-slot rule tables keep one arm per slot, with explicit `return`s,
// parenthesised conditions and arms with identical bodies, so each slot's rule
// can be read and changed on its own.
#[allow(
    unused_parens,
    unused_variables,
    clippy::if_same_then_else,
    clippy::needless_return,
    reason = "one self-contained rule per slot"
)]
#[path = "client_options_codec.rs"]
mod codec;
#[path = "client_options_live.rs"]
mod live;
#[cfg(test)]
#[path = "client_options_oracle.rs"]
mod oracle;
#[path = "client_options_presets.rs"]
mod presets;
pub use live::LiveSettings;
#[cfg(test)]
#[path = "client_options_tests.rs"]
mod tests;
