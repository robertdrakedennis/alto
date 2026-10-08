//! Numeric style properties and direct font defaults. Parent IDs do not prove
//! that the original runtime installed a live inheritance link.
use super::{INDEX_ARCHIVE, StyleFont};
use crate::{corpus, profile::digest};
use anyhow::{Context, Result, ensure};
use native910::js5::{ArchiveIndex, decompress};
use rs910_config::ui_bytes::Cursor;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    pub archive: u32,
    pub index_sha256: String,
    numeric_tag: u8,
    absent_parent: i32,
    font_key: u32,
    pub kernels: BTreeMap<String, String>,
}
impl Profile {
    fn decode(&self, bytes: &[u8]) -> Result<Document> {
        let mut cursor = Cursor::new(bytes);
        let parent = i32::from(cursor.g2()? as i16);
        let count = cursor.g2()?;
        let mut values = BTreeMap::new();
        let mut published = true;
        for _ in u16::default()..count {
            let tag = cursor.g1()?;
            let key = cursor.g4s()? as u32;
            if tag != self.numeric_tag {
                published = false;
                break;
            }
            let value = cursor.g4s()?;
            values.entry(key).or_insert(value);
        }
        Ok(Document {
            parent,
            values,
            consumed: cursor.pos(),
            remaining: cursor.remaining(),
            published,
        })
    }
    fn property(&self, doc: &Document, key: u32, inherit: bool) -> Result<Option<i32>> {
        ensure!(
            doc.published,
            "style loader did not publish this definition"
        );
        if let Some(value) = doc.values.get(&key) {
            return Ok(Some(*value));
        }
        ensure!(
            !inherit || doc.parent == self.absent_parent,
            "style property needs an unsealed runtime parent link; a parent ID alone is insufficient"
        );
        Ok(None)
    }
}
#[derive(Serialize)]
struct Document {
    parent: i32,
    values: BTreeMap<u32, i32>,
    consumed: usize,
    remaining: usize,
    published: bool,
}
struct Definition {
    sha256: String,
    value: std::result::Result<Document, String>,
}
pub(super) struct Definitions {
    profile: Profile,
    rows: BTreeMap<i32, Definition>,
}
impl Definitions {
    pub fn empty(profile: &Profile) -> Self {
        Self {
            profile: profile.clone(),
            rows: BTreeMap::new(),
        }
    }
    pub fn load(root: &Path, profile: &Profile) -> Result<Self> {
        let bytes = std::fs::read(
            root.join(INDEX_ARCHIVE.to_string())
                .join(format!("{}.dat", profile.archive)),
        )?;
        ensure!(
            digest(&bytes) == profile.index_sha256,
            "style index does not match the frame profile"
        );
        let index = ArchiveIndex::decode(&decompress(&bytes)?)?;
        let mut result = Self::empty(profile);
        for group in &index.group_id {
            let files = corpus::load_group(root, profile.archive, &index, *group)?;
            ensure!(
                files.len() == usize::from(true) && files.contains_key(&u32::default()),
                "style group needs a single default file"
            );
            result.insert(i32::try_from(*group)?, &files[&u32::default()])?;
        }
        Ok(result)
    }
    fn insert(&mut self, id: i32, bytes: &[u8]) -> Result<()> {
        let value = self
            .profile
            .decode(bytes)
            .map_err(|error| error.to_string());
        ensure!(
            self.rows
                .insert(
                    id,
                    Definition {
                        sha256: digest(bytes),
                        value
                    }
                )
                .is_none(),
            "duplicate style definition"
        );
        Ok(())
    }
    pub fn font(&self, style: i16) -> Result<StyleFont> {
        let source = self
            .rows
            .get(&i32::from(style))
            .context("source frame style is absent")?;
        let document = source
            .value
            .as_ref()
            .map_err(|error| anyhow::anyhow!("style decode failed: {error}"))?;
        Ok(StyleFont {
            style,
            source_sha256: source.sha256.clone(),
            font: self
                .profile
                .property(document, self.profile.font_key, true)?,
        })
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"definitions":self.rows.len(),
            "decode_failures":self.rows.values().filter(|row| row.value.is_err()).count(),
            "unpublished":self.rows.values().filter(|row|row.value.as_ref().is_ok_and(|doc|!doc.published)).count(),
            "parent_ids":self.rows.values().filter(|row|row.value.as_ref().is_ok_and(|doc|doc.parent != self.profile.absent_parent)).count(),
            "direct_fonts":self.rows.values().filter(|row|row.value.as_ref().is_ok_and(|doc|doc.values.contains_key(&self.profile.font_key))).count(),
            "scope":"Numeric style wire and direct property lookup; runtime parent link construction is unsealed"})
    }
    pub fn inspect(&self, style: i16) -> serde_json::Value {
        self.rows.get(&i32::from(style)).map_or(serde_json::Value::Null, |source| {
            serde_json::json!({"style":style,"source_sha256":source.sha256,"definition":source.value.as_ref().ok(),"error":source.value.as_ref().err()})
        })
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn authored_fixture(profile: &Profile, files: &BTreeMap<i32, Vec<u8>>) -> Result<Self> {
        let mut result = Self::empty(profile);
        for (id, bytes) in files {
            result.insert(*id, bytes)?;
        }
        Ok(result)
    }
}
#[cfg(test)]
pub(super) fn verify(profile: &Profile, client_md5: &str) {
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../fixtures/style-fonts.json")).unwrap();
    assert_eq!(fixture["client_md5"], client_md5);
    assert_eq!(fixture["font_key"], profile.font_key);
    for (address, hash) in &profile.kernels {
        assert_eq!(fixture["kernels"][address], *hash);
    }
    for case in fixture["decoder_cases"].as_array().unwrap() {
        let wire: Vec<u8> = serde_json::from_value(case["wire"].clone()).unwrap();
        let doc = profile.decode(&wire).unwrap();
        assert_eq!(serde_json::json!(doc.parent), case["result"]["parent"]);
        assert_eq!(
            serde_json::json!(
                doc.values
                    .iter()
                    .map(|(key, value)| (*key, *value as u32))
                    .collect::<Vec<_>>()
            ),
            case["result"]["entries"]
        );
        assert_eq!(serde_json::json!(doc.consumed), case["result"]["consumed"]);
        assert_eq!(
            serde_json::json!(usize::from(doc.published)),
            case["result"]["published"]
        );
        if !doc.published {
            assert!(profile.property(&doc, profile.font_key, true).is_err());
        }
        for end in usize::default()..doc.consumed {
            assert!(profile.decode(&wire[..end]).is_err());
        }
    }
    for case in fixture["lookup_cases"].as_array().unwrap() {
        let source = &case["documents"][0];
        let doc = Document {
            parent: serde_json::from_value(source["parent"].clone()).unwrap(),
            values: serde_json::from_value::<Vec<(u32, i32)>>(source["entries"].clone())
                .unwrap()
                .into_iter()
                .collect(),
            consumed: usize::default(),
            remaining: usize::default(),
            published: true,
        };
        let mode = case["mode"].as_str().unwrap();
        let key = if mode == "font" {
            profile.font_key
        } else {
            serde_json::from_value(case["key"].clone()).unwrap()
        };
        let inherit = mode == "font" || case["inherit"] == serde_json::json!(true as u8);
        let value = profile.property(&doc, key, inherit);
        let requires_live_link =
            doc.parent != profile.absent_parent && inherit && !doc.values.contains_key(&key);
        // Supplied live links exercise the recorded lookup, but the cache loader
        // does not install them. Keep these imports unavailable until linking is sealed.
        if requires_live_link {
            assert!(value.is_err());
            continue;
        }
        let value = value.unwrap();
        if mode == "font" {
            assert_eq!(
                serde_json::json!(
                    value.unwrap_or(serde_json::from_value(case["initial_font"].clone()).unwrap())
                ),
                case["result"]["font"]
            );
        } else {
            assert_eq!(serde_json::json!(value), case["result"]["value"]);
        }
    }
}
