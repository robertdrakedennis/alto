//! CS2 variable domains. Revision 910 defines exactly 11 domains (`0..=10`); a
//! byte outside that range is malformed input, surfaced loudly instead of being
//! silently remapped.

use crate::error::{NativeError, Result};

/// A CS2 variable domain (`push_var`/`pop_var` namespace).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VarScope {
    /// Player vars.
    Player = 0,
    /// Npc vars.
    Npc = 1,
    /// Client vars.
    Client = 2,
    /// World vars.
    World = 3,
    /// Region vars.
    Region = 4,
    /// Object vars.
    Object = 5,
    /// Clan vars.
    Clan = 6,
    /// Clan-setting vars.
    ClanSetting = 7,
    /// Controller vars.
    Controller = 8,
    /// Player-group vars.
    Group = 9,
    /// Global vars.
    Global = 10,
}

impl VarScope {
    /// Decode a domain id byte.
    pub fn from_id(id: u8) -> Result<Self> {
        match id {
            0 => Ok(Self::Player),
            1 => Ok(Self::Npc),
            2 => Ok(Self::Client),
            3 => Ok(Self::World),
            4 => Ok(Self::Region),
            5 => Ok(Self::Object),
            6 => Ok(Self::Clan),
            7 => Ok(Self::ClanSetting),
            8 => Ok(Self::Controller),
            9 => Ok(Self::Group),
            10 => Ok(Self::Global),
            other => Err(NativeError::Invalid(format!(
                "unknown var domain id {other} (valid range 0..=10)"
            ))),
        }
    }

    /// Short label used in diagnostics and source text.
    #[must_use]
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Npc => "npc",
            Self::Client => "client",
            Self::World => "world",
            Self::Region => "region",
            Self::Object => "object",
            Self::Clan => "clan",
            Self::ClanSetting => "clan_setting",
            Self::Controller => "controller",
            Self::Group => "player_group",
            Self::Global => "global",
        }
    }
    /// Parse a source-text label back to its domain.
    pub fn from_label(label: &str) -> Result<Self> {
        match label {
            "player" => Ok(Self::Player),
            "npc" => Ok(Self::Npc),
            "client" => Ok(Self::Client),
            "world" => Ok(Self::World),
            "region" => Ok(Self::Region),
            "object" => Ok(Self::Object),
            "clan" => Ok(Self::Clan),
            "clan_setting" => Ok(Self::ClanSetting),
            "controller" => Ok(Self::Controller),
            "player_group" => Ok(Self::Group),
            "global" => Ok(Self::Global),
            _ => Err(NativeError::Invalid(format!(
                "unknown var domain '{label}'"
            ))),
        }
    }
}

impl From<VarScope> for u8 {
    fn from(domain: VarScope) -> Self {
        domain as Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scope against the recorded declarations in `data/var-domains.tsv`:
    /// wire id, source label (the original name lowercased) and config group,
    /// both directions.
    #[test]
    fn scopes_match_the_recorded_declarations() {
        let rows = include_str!("../data/var-domains.tsv");
        let mut seen = 0;
        for line in rows.lines() {
            let mut fields = line.split('\t');
            let (Some(name), Some(id), Some(group), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                panic!("malformed row {line:?}");
            };
            let id: u8 = id.parse().unwrap();
            let group: u32 = group.parse().unwrap();
            let label = name.to_ascii_lowercase();
            let domain = VarScope::from_id(id).unwrap();
            assert_eq!(domain.as_label(), label, "{name} id {id}");
            assert_eq!(VarScope::from_label(&label).unwrap(), domain, "{name}");
            assert_eq!(u8::from(domain), id, "{name}");
            assert_eq!(crate::config::var_group_id(domain), group, "{name}");
            assert_eq!(
                crate::config::var_domain_from_group(group).unwrap(),
                domain,
                "{name}"
            );
            seen += 1;
        }
        assert_eq!(seen, 11, "recorded var scope declarations");
        assert!(VarScope::from_id(11).is_err());
        assert!(VarScope::from_id(u8::MAX).is_err());
    }
}
