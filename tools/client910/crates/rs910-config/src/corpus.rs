//! Corpus check of the config decoders: every config of every type in the
//! local cache is decoded, reduced to a name-independent canonical text and
//! hashed. `fixtures/config-corpus.tsv` holds the per-type digests, so a
//! change in what any decoder produces (values, defaults, errors) fails the
//! test with the type and the first differing config named.
//!
//! The canonical text is the `Debug` output with field and struct names
//! removed, so renaming a field does not move a digest; reordering fields or
//! changing a value does. The fixture was generated from the decoders as they
//! were before they were reshaped into opcode tables, and the reshaped
//! decoders were compared with them row by row over the whole cache (114672
//! locs, 49001 objs, 26769 npcs, and so on, see the fixture) before those
//! were removed. Regenerate it with `RS910_UPDATE_CORPUS=1 cargo test -p
//! rs910-config corpus` only when a change to a decoded value is intended.

use std::fmt::Debug;
use std::path::PathBuf;

use crate::cache::Pack;

/// One config type: its name and the `(config id, canonical value)` rows.
pub struct Section {
    pub name: &'static str,
    pub rows: Vec<(String, String)>,
}

/// `Debug` output without field names and struct names.
pub fn canonical<T: Debug>(value: &T) -> String {
    strip_names(&format!("{value:?}"))
}

/// The canonical text of a fallible decode: the value, or the error text.
pub fn outcome<T: Debug, E: Debug>(result: Result<T, E>) -> String {
    match result {
        Ok(value) => canonical(&value),
        Err(error) => format!("ERR {error:?}"),
    }
}

/// [`outcome`] for `anyhow` errors, whose alternate form carries the chain.
pub fn outcome_anyhow<T: Debug>(result: anyhow::Result<T>) -> String {
    match result {
        Ok(value) => canonical(&value),
        Err(error) => format!("ERR {error:#}"),
    }
}

fn strip_names(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if chars[i - 1] == '"' {
                    break;
                }
            }
            continue;
        }
        let starts_word = (c.is_ascii_alphabetic() || c == '_')
            && (i == 0
                || !(chars[i - 1].is_ascii_alphanumeric()
                    || chars[i - 1] == '_'
                    || chars[i - 1] == '.'));
        if starts_word {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let field_name = chars.get(j) == Some(&':') && chars.get(j + 1) == Some(&' ');
            let struct_name = chars.get(j) == Some(&' ')
                && chars.get(j + 1) == Some(&'{')
                && c.is_ascii_uppercase();
            if field_name {
                i = j + 2;
                continue;
            }
            if struct_name {
                i = j + 1;
                continue;
            }
            out.push_str(&word);
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// FNV-1a over every row of a section.
pub fn digest(rows: &[(String, String)]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (key, value) in rows {
        for byte in key.bytes().chain([0]).chain(value.bytes()).chain([b'\n']) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn fixture_path() -> PathBuf {
    rs910_core::test_support::client_dir().join("crates/rs910-config/fixtures/config-corpus.tsv")
}

fn digest_table(sections: &[Section]) -> String {
    let mut out = String::from("# type\tconfigs\tdigest\n");
    for section in sections {
        out.push_str(&format!(
            "{}\t{}\t{:016x}\n",
            section.name,
            section.rows.len(),
            digest(&section.rows)
        ));
    }
    out
}

use crate::types910::{
    bas_types::Bas,
    combat_types::{Bar, Hit},
    config_types::{Item, Npc},
    defaults::{Graphics, Wear},
    effect_types::Effect,
    idk_types::IdentityKit,
    pack_defaults, pack_types,
    script_types::script_type,
    sequence_types::Group,
    titles,
    variable_types::{Domain, Variable},
};
use crate::{
    animation_sequences, avatar, billboard, config, flo, scenery_varbits, skybox_types, ui_configs,
    ui_quests,
};
use std::collections::BTreeMap;

/// Every file of a multi-group config archive, keyed by config id.
fn archive_files(pack: &Pack, archive: &str, bits: u32) -> Vec<(u32, Vec<u8>)> {
    let (_, records) = ui_configs::records(pack, archive, bits).unwrap();
    records
        .into_iter()
        .map(|(id, bytes)| (id as u32, bytes))
        .collect()
}

/// Every file of one group of the `config` archive.
fn group_files(pack: &Pack, group: u32) -> Vec<(u32, Vec<u8>)> {
    let index = pack.read_archive_index("config").unwrap();
    if !index.group_id.contains(&group) {
        return Vec::new();
    }
    pack.read_group("config", group)
        .unwrap()
        .into_iter()
        .collect()
}

fn section<F: FnMut(u32, &[u8]) -> String>(
    name: &'static str,
    files: Vec<(u32, Vec<u8>)>,
    mut decode: F,
) -> Section {
    Section {
        name,
        rows: files
            .iter()
            .map(|(id, bytes)| (id.to_string(), decode(*id, bytes)))
            .collect(),
    }
}

pub fn snapshot(pack: &Pack) -> Vec<Section> {
    let mut out = Vec::new();
    out.push(section(
        "loc",
        archive_files(pack, "loc.config", 8),
        |id, b| outcome_anyhow(config::decode_loc(id, b)),
    ));
    out.push(section(
        "obj",
        archive_files(pack, "obj.config", 8),
        |id, b| outcome_anyhow(config::decode_obj(id, b)),
    ));
    out.push(Section {
        name: "obj-resolved",
        rows: config::ObjStore::load(pack)
            .unwrap()
            .iter()
            .map(|(id, obj)| (id.to_string(), canonical(obj)))
            .collect(),
    });
    out.push(section(
        "npc",
        archive_files(pack, "npc.config", 7),
        |id, b| outcome_anyhow(config::decode_npc(id, b)),
    ));
    out.push(section(
        "seq",
        archive_files(pack, "seq.config", 7),
        |id, b| outcome_anyhow(config::decode_seq(id, b)),
    ));
    out.push(section("inv", group_files(pack, 5), |id, b| {
        outcome_anyhow(config::decode_inv(id, b))
    }));
    out.push(section(
        "item",
        archive_files(pack, "obj.config", 8),
        |id, b| outcome(Item::decode(id as i32, b)),
    ));
    for (name, members) in [("item-resolved", true), ("item-resolved-free", false)] {
        let types = pack_types::load(pack, members).unwrap();
        out.push(Section {
            name,
            rows: types
                .items
                .iter()
                .map(|(id, v)| (id.to_string(), canonical(v)))
                .collect(),
        });
        if members {
            out.push(Section {
                name: "npc-entity",
                rows: types
                    .npcs
                    .iter()
                    .map(|(id, v)| (id.to_string(), canonical(v)))
                    .collect(),
            });
        }
    }
    out.push(section(
        "npc-raw",
        archive_files(pack, "npc.config", 7),
        |id, b| outcome(Npc::decode(id as i32, b)),
    ));
    out.push(section("hitmark", group_files(pack, 46), |id, b| {
        outcome(Hit::decode(id as i32, b))
    }));
    out.push(section("headbar", group_files(pack, 72), |id, b| {
        outcome(Bar::decode(id as i32, b))
    }));
    out.push(section(
        "spotanim",
        archive_files(pack, "spot.config", 8),
        |id, b| outcome(Effect::decode(id as i32, b)),
    ));
    out.push(section("seqgroup", group_files(pack, 77), |_, b| {
        outcome(Group::decode(b))
    }));
    let sequences = animation_sequences::load(pack).unwrap();
    out.push(Section {
        name: "sequence-resolved",
        rows: sequences
            .sequences
            .iter()
            .map(|(id, v)| (id.to_string(), canonical(v)))
            .collect(),
    });
    out.push(Section {
        name: "seqgroup-resolved",
        rows: sequences
            .groups
            .iter()
            .map(|(id, v)| (id.to_string(), canonical(v)))
            .collect(),
    });
    for (name, group, domain) in [
        ("varp", 60, Domain::Player),
        ("varn", 61, Domain::Npc),
        ("var-domain2", 62, Domain::Npc),
        ("var-domain5", 65, Domain::Npc),
        ("var-domain6", 66, Domain::Npc),
        ("var-domain7", 67, Domain::Npc),
        ("var-domain9", 80, Domain::Npc),
    ] {
        out.push(section(name, group_files(pack, group), |id, b| {
            outcome(Variable::decode(domain, id as i32, b))
        }));
    }
    let varbit_inputs = scenery_varbits::load(pack).unwrap();
    for (name, allow_unbound) in [("varbit", true), ("varbit-strict", false)] {
        out.push(Section {
            name,
            rows: (0..varbit_inputs.raw.keys().last().map_or(0, |id| id + 1) as i32)
                .map(|id| {
                    (
                        id.to_string(),
                        outcome(varbit_inputs.get(id, allow_unbound)),
                    )
                })
                .collect(),
        });
    }
    out.push(Section {
        name: "defaults",
        rows: vec![
            ("all".into(), outcome_anyhow(pack_defaults::load(pack))),
            (
                "graphics".into(),
                outcome(Graphics::decode(
                    &pack_defaults::flat_file(pack, 3).unwrap(),
                )),
            ),
            (
                "wear".into(),
                outcome(Wear::decode(&pack_defaults::flat_file(pack, 6).unwrap())),
            ),
            (
                "titles".into(),
                outcome(titles::Defaults::decode(
                    &pack_defaults::flat_file(pack, 12).unwrap(),
                )),
            ),
        ],
    });
    out.push(section(
        "enum-title",
        archive_files(pack, "enum.config", 8),
        |_, b| outcome(titles::Enum::decode(b)),
    ));
    out.push(section("bas", group_files(pack, 32), |id, b| {
        outcome(Bas::decode(id as i32, b))
    }));
    out.push(section("idk", group_files(pack, 3), |id, b| {
        format!(
            "{} | {}",
            outcome_anyhow(avatar::decode_idk(id, b)),
            outcome(IdentityKit::decode(id as i32, b))
        )
    }));
    out.push(Section {
        name: "defaults-palette",
        rows: vec![
            (
                "load".into(),
                outcome_anyhow(avatar::GraphicsDefaults::load(pack)),
            ),
            (
                "file".into(),
                outcome_anyhow(avatar::GraphicsDefaults::decode(
                    &pack_defaults::flat_file(pack, 3).unwrap(),
                )),
            ),
        ],
    });
    let billboards = billboard::BillboardStore::load(pack).unwrap();
    out.push(Section {
        name: "billboard",
        rows: pack
            .read_group("billboards", 0)
            .unwrap()
            .keys()
            .map(|&id| (id.to_string(), canonical(&billboards.get(id as i32))))
            .collect(),
    });
    out.push(section("quest", group_files(pack, 35), |_, b| {
        outcome_anyhow(ui_quests::Quest::decode(b))
    }));
    out.push(section("floor-overlay", group_files(pack, 4), |id, b| {
        outcome_anyhow(flo::decode_overlay(id, b))
    }));
    out.push(section("floor-underlay", group_files(pack, 1), |id, b| {
        outcome_anyhow(flo::decode_underlay(id, b))
    }));
    out.push(section(
        "skybox",
        group_files(pack, skybox_types::SKYBOXTYPE_GROUP),
        |_, b| outcome_anyhow(skybox_types::SkyBoxType::decode(b)),
    ));
    out.push(section(
        "skydecor",
        group_files(pack, skybox_types::SKYDECORTYPE_GROUP),
        |_, b| outcome_anyhow(skybox_types::SkyDecorType::decode(b)),
    ));
    out.push(section(
        "enum",
        archive_files(pack, "enum.config", 8),
        |_, b| outcome_anyhow(ui_configs::Enum::decode(b)),
    ));
    out.push(section(
        "struct",
        archive_files(pack, "struct.config", 5),
        |_, b| outcome_anyhow(ui_configs::Struct::decode(b)),
    ));
    out.push(section(
        "param",
        group_files(pack, 11),
        |_, b| match ui_configs::Param::decode(b) {
            Ok(p) => format!("({}, {}, {:?})", p.string, p.integer, p.text),
            Err(e) => format!("ERR {e:#}"),
        },
    ));
    out.push(Section {
        name: "script-type",
        rows: (0..=255u8)
            .map(|id| (id.to_string(), canonical(&script_type(id))))
            .collect(),
    });
    let _: BTreeMap<u8, u8> = BTreeMap::new();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> Pack {
        crate::test_support::require_pack("client.config.js5")
    }

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn config_digests_match_the_committed_fixture() {
        let pack = pack();
        let table = digest_table(&snapshot(&pack));
        if std::env::var_os("RS910_UPDATE_CORPUS").is_some() {
            std::fs::create_dir_all(fixture_path().parent().unwrap()).unwrap();
            std::fs::write(fixture_path(), &table).unwrap();
        }
        let expected = std::fs::read_to_string(fixture_path()).unwrap();
        for (now, then) in table.lines().zip(expected.lines()) {
            assert_eq!(now, then, "config decoder output changed");
        }
        assert_eq!(table, expected, "config decoder output changed");
    }
}
