//! Versioned options block and local file lifecycle.
//!
//! The block starts with a version byte. Versions below 23 use the legacy
//! layout (a truncated legacy block falls back to defaults), versions above 38
//! are unknown and take the full defaults, and versions 23 to 38 read the
//! gated fields in order and then fill the remaining slots with defaults
//! (refreshing the audio slots below version 32). Every decode ends with a
//! clamp. `load` returns the default options for a missing, empty, truncated
//! or corrupt file, so `decode` reports a truncated modern block as `Err`.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated;
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl Reader<'_> {
    fn g1(&mut self) -> Result<i32, Truncated> {
        let v = *self.bytes.get(self.pos).ok_or(Truncated)?;
        self.pos += 1;
        Ok(v as i32)
    }
    fn g1b(&mut self) -> Result<i32, Truncated> {
        Ok(self.g1()? as i8 as i32)
    }
    fn g2(&mut self) -> Result<i32, Truncated> {
        Ok((self.g1()? << 8) | self.g1()?)
    }
    fn g4(&mut self) -> Result<i32, Truncated> {
        Ok((self.g2()? << 16) | self.g2()?)
    }
}
impl ClientOptions {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(59);
        out.push(38);
        out.push((self.values[0]) as u8);
        out.push((self.values[1]) as u8);
        out.push((self.values[3]) as u8);
        out.push((self.values[4]) as u8);
        out.push((self.values[5]) as u8);
        out.push((self.values[6]) as u8);
        out.push((self.values[8]) as u8);
        out.push((self.values[9]) as u8);
        out.push((self.values[10]) as u8);
        out.push((self.values[11]) as u8);
        out.push((self.values[12]) as u8);
        out.push((self.values[13]) as u8);
        out.push((self.values[14]) as u8);
        out.push((self.values[15]) as u8);
        out.push((self.values[16]) as u8);
        out.push(0);
        out.push((self.values[17]) as u8);
        out.push((self.values[18]) as u8);
        out.push((self.values[19]) as u8);
        out.push((self.values[21]) as u8);
        out.push((self.values[22]) as u8);
        out.push((self.values[23]) as u8);
        out.push((self.values[24]) as u8);
        out.push((self.values[25]) as u8);
        out.push(0);
        out.push((self.values[27]) as u8);
        out.push((self.values[28]) as u8);
        out.push((self.values[31]) as u8);
        out.push((self.values[32]) as u8);
        out.push((self.values[33]) as u8);
        out.push((self.values[30]) as u8);
        out.push((self.values[34]) as u8);
        out.push((self.values[35]) as u8);
        out.push((self.values[36]) as u8);
        out.push((self.values[37]) as u8);
        out.extend_from_slice(&((self.values[38]) as u16).to_be_bytes());
        out.extend_from_slice(&((self.values[39]) as u16).to_be_bytes());
        out.extend_from_slice(&((self.values[40]) as u16).to_be_bytes());
        out.extend_from_slice(&((self.values[41]) as u16).to_be_bytes());
        out.push((self.values[42]) as u8);
        out.push((self.values[43]) as u8);
        out.push((self.values[44]) as u8);
        out.push((self.values[45]) as u8);
        out.push((self.values[46]) as u8);
        out.push((self.values[47]) as u8);
        out.push((self.values[48]) as u8);
        out.push((self.values[7]) as u8);
        out.push((self.values[49]) as u8);
        out.push((self.values[50]) as u8);
        out.push((self.values[51]) as u8);
        out.push((self.values[52]) as u8);
        out.push((self.values[53]) as u8);
        out.push((self.values[54]) as u8);
        out
    }
    pub fn decode(bytes: &[u8], profile: Profile) -> Result<Self, Truncated> {
        let mut p = Reader { bytes, pos: 0 };
        let version = p.g1()?;
        let mut out = Self {
            particle_level: 0,
            values: [0; 55],
            profile,
            display_flags: [true; 55],
            defaulted: [false; 55],
            present: [false; 55],
        };
        out.assign("displayMode", profile.initial_display_mode);
        // Legacy layout below 23, defaults for a version this client does not know.
        if version < 23 {
            if Self::read_legacy(&mut out, &mut p, version).is_err() {
                out.set_defaults(true, true);
            }
            out.set_defaults(false, true);
        } else if version > 38 {
            out.set_defaults(true, true);
        } else {
            if (version >= 29) {
                out.assign("animDetail", p.g1()?);
            }
            out.assign("antiAliasing", p.g1()?);
            if (version >= 31) {
                out.assign("unused", p.g1()?);
            }
            out.assign("antiAliasing2", out.values[1]);
            out.assign("bloom", p.g1()?);
            out.assign("brightness", p.g1()?);
            out.assign("buildArea", p.g1()?);
            if (version >= 27) {
                out.assign("drawDistance", p.g1()?);
            }
            out.assign("flickeringEffects", p.g1()?);
            out.assign("fog", p.g1()?);
            out.assign("groundBlending", p.g1()?);
            out.assign("groundDecoration", p.g1()?);
            out.assign("idleAnimations", p.g1()?);
            out.assign("lightingDetail", p.g1()?);
            out.assign("sceneryShadows", p.g1()?);
            if (version >= 33) {
                out.assign("shadowQuality", p.g1()?);
            }
            if (version >= 34) {
                p.g1()?;
            }
            if (version >= 24) {
                out.assign("orthographic", p.g1()?);
            }
            out.assign("particles", p.g1()?);
            out.assign("removeRoofs", p.g1()?);
            out.assign("removeRoofs2", out.values[19]);
            out.assign("screenSize", p.g1()?);
            if (version >= 25) {
                out.assign("skyboxes", p.g1()?);
            }
            out.assign("characterShadows", p.g1()?);
            if (version <= 25) {
                p.pos += 1;
            }
            out.assign("textures", p.g1()?);
            out.assign("toolkit", p.g1()?);
            out.assign("displayMode", out.values[25]);
            p.g1()?;
            out.assign("waterDetail", p.g1()?);
            out.assign("windowMode", p.g1()?);
            if (version >= 35) {
                out.assign("unused1", p.g1()?);
                out.assign("unused2", p.g1()?);
                out.assign("unused3", p.g1()?);
                out.assign("unused4", p.g1()?);
                out.assign("unused5", p.g1()?);
                out.assign("unused6", p.g1b()?);
            }
            if (version >= 36) {
                out.assign("unused7", p.g1()?);
                out.assign("unused8", p.g1()?);
            }
            if (version >= 37) {
                out.assign("unused9", p.g2()?);
                out.assign("unused10", p.g2()?);
            }
            if (version >= 38) {
                out.assign("unused11", p.g2()?);
                out.assign("unused12", p.g2()?);
            }
            out.assign("maxScreenSize2", out.values[28]);
            out.assign("customCursors", p.g1()?);
            out.assign("preset", p.g1()?);
            out.assign("cpuUsage", p.g1()?);
            out.assign("loadingScreen", p.g1()?);
            out.assign("safeMode", p.g1()?);
            if (version >= 26) {
                out.assign("unknown7", p.g1()?);
            }
            if (version >= 28) {
                out.assign("unused13", p.g1()?);
            }
            if (version >= 30) {
                out.assign("consoleKeyPress", p.g1()?);
            }
            out.assign("soundVolume", p.g1()?);
            out.assign("backgroundSoundVolume", p.g1()?);
            out.assign("speechVolume", p.g1()?);
            out.assign("unknownVolume1", p.g1()?);
            out.assign("unknownVolume2", p.g1()?);
            out.assign("stereo", p.g1()?);
            out.set_defaults(false, version < 32);
        }
        out.clamp();
        Ok(out)
    }
    fn read_legacy(out: &mut Self, p: &mut Reader<'_>, version: i32) -> Result<(), Truncated> {
        out.assign("brightness", p.g1()?);
        p.pos += 1;
        out.assign("removeRoofs", p.g1()? + 1);
        out.assign("groundDecoration", p.g1()?);
        p.pos += 1;
        out.assign("idleAnimations", p.g1()?);
        out.assign("flickeringEffects", p.g1()?);
        p.g1()?;
        out.assign("characterShadows", p.g1()?);
        let first_shadow = p.g1()?;
        let mut second_shadow = 0;
        if (version >= 17) {
            second_shadow = p.g1()?;
        }
        out.assign(
            "sceneryShadows",
            if first_shadow > second_shadow {
                first_shadow
            } else {
                second_shadow
            },
        );
        let mut second_light = true;
        let first_light: bool;
        if (version >= 2) {
            first_light = p.g1()? == 1;
            if (version >= 17) {
                second_light = p.g1()? == 1;
            }
        } else {
            first_light = p.g1()? == 1;
            p.g1()?;
        }
        out.assign(
            "lightingDetail",
            if first_light | second_light { 1 } else { 0 },
        );
        out.assign("waterDetail", p.g1()?);
        out.assign("fog", p.g1()?);
        out.assign("antiAliasing", p.g1()?);
        out.assign("stereo", p.g1()?);
        out.assign("soundVolume", p.g1()?);
        if (version >= 20) {
            out.assign("speechVolume", p.g1()?);
        } else {
            out.assign("speechVolume", out.values[49]);
        }
        out.assign("unknownVolume1", p.g1()?);
        out.assign("backgroundSoundVolume", p.g1()?);
        if (version >= 21) {
            out.assign("unknownVolume2", p.g1()?);
        } else {
            out.assign("unknownVolume2", out.values[52]);
        }
        if (version >= 1) {
            p.g2()?;
            p.g2()?;
        }
        if (3..6).contains(&version) {
            p.g1()?;
        }
        if (version >= 4) {
            out.assign("particles", p.g1()?);
        }
        p.g4()?;
        if (version >= 6) {
            out.assign("windowMode", p.g1()?);
        }
        if (version >= 7) {
            out.assign("safeMode", p.g1()?);
        }
        if (version >= 8) {
            p.g1()?;
        }
        if (version >= 9) {
            out.assign("buildArea", p.g1()?);
        }
        if (version >= 10) {
            out.assign("bloom", p.g1()?);
        }
        if (version >= 11) {
            out.assign("customCursors", p.g1()?);
        }
        if (version >= 12) {
            out.assign("idleAnimations", p.g1()?);
        }
        if (version >= 13) {
            out.assign("groundBlending", p.g1()?);
        }
        if (version >= 14) {
            out.assign("toolkit", p.g1()?);
        }
        if (version >= 15) {
            out.assign("cpuUsage", p.g1()?);
        }
        if (version >= 16) {
            out.assign("textures", p.g1()?);
        }
        if (version >= 18) {
            out.assign("preset", p.g1()?);
        }
        if (version >= 19) {
            out.assign("screenSize", p.g1()?);
        }
        if (version >= 22) {
            out.assign("loadingScreen", p.g1()?);
        }
        Ok(())
    }
    /// Reads the options file at `path`. A missing, empty, truncated or corrupt
    /// file gives the default options for `profile`.
    pub fn load(path: &std::path::Path, profile: Profile) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| Self::decode(&bytes, profile).ok())
            .unwrap_or_else(|| Self::new(profile))
    }
    /// Writes the encoded options to `path`. The write goes to a temporary file
    /// next to the destination and is renamed over it, so a concurrent `load`
    /// never sees a torn prefix.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&temporary, self.encode())?;
        std::fs::rename(temporary, path)
    }
}
