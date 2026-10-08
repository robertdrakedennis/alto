//! Exact-client text defaults from donor frames. A decoded prefix is not a
//! complete component codec; runtime style parent links and remaining fields stay gaps.
mod styles;
use crate::{
    corpus,
    profile::{Book, Build, digest},
};
use anyhow::{Context, Result, ensure};
use native910::js5::{ArchiveIndex, decompress};
use rs910_config::ui_bytes::Cursor;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, str::FromStr};

const FORMAT: u32 = 1;
const INDEX_ARCHIVE: u32 = 255;
const TERMINATOR: u8 = 0;
const TRUE_BYTE: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRef {
    pub interface: u32,
    pub file: u32,
}
impl FromStr for FrameRef {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let (interface, file) = value.split_once(':').context("use interface:file")?;
        Ok(Self {
            interface: interface.parse()?,
            file: file.parse()?,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    format: u32,
    build: Build,
    client_md5: String,
    script_index_sha256: String,
    interface_archive: u32,
    interface_index_sha256: String,
    text_type: u8,
    type_mask: u8,
    name_mask: u8,
    absent_version: u8,
    aspect_mode: i8,
    monospaced_version: i32,
    max_lines_version: i32,
    style_version: i32,
    absent_style: i16,
    kernels: BTreeMap<String, String>,
    styles: styles::Profile,
}
impl Profile {
    fn parse(bytes: &[u8], book: &Book) -> Result<Self> {
        let profile: Self = serde_json::from_slice(bytes)?;
        ensure!(profile.format == FORMAT, "unsupported frame profile format");
        ensure!(
            profile.build == book.profile.build
                && profile.client_md5 == book.profile.client_md5
                && profile.script_index_sha256 == book.profile.script_index_sha256,
            "frame profile does not match the exact script client and index"
        );
        ensure!(
            !profile.kernels.is_empty() && profile.kernels.values().all(|hash| !hash.is_empty()),
            "missing frame decoder recording identity"
        );
        ensure!(
            !profile.styles.kernels.is_empty(),
            "missing style recording identity"
        );
        Ok(profile)
    }
    fn version(&self, value: u8) -> i32 {
        if value == self.absent_version {
            -i32::from(true)
        } else {
            i32::from(value)
        }
    }
    fn text(&self, cursor: &mut Cursor<'_>, version: i32) -> Result<TextState> {
        Ok(TextState {
            font: cursor.gsmart2or4s()?,
            monospaced: version >= self.monospaced_version && cursor.g1()? == TRUE_BYTE,
            text_bytes: skip_string(cursor)?,
            line_height: cursor.g1()?,
            horizontal_align: cursor.g1()?,
            vertical_align: cursor.g1()?,
            shadow: cursor.g1()? == TRUE_BYTE,
            colour: cursor.g4s()? as u32,
            alpha: !cursor.g1()?,
            max_lines: if version >= self.max_lines_version {
                cursor.g1()?
            } else {
                u8::default()
            },
        })
    }
    fn component(&self, bytes: &[u8]) -> Result<Option<TextFrame>> {
        let mut cursor = Cursor::new(bytes);
        let version = self.version(cursor.g1()?);
        let kind = cursor.g1()?;
        if kind & self.type_mask != self.text_type {
            return Ok(None);
        }
        let name_bytes = if kind & self.name_mask != u8::default() {
            Some(skip_string(&mut cursor)?)
        } else {
            None
        };
        let client_code = cursor.g2()?;
        let x = cursor.g2()? as i16;
        let y = cursor.g2()? as i16;
        let width = cursor.g2()?;
        let height = cursor.g2()?;
        let width_mode = cursor.g1b()?;
        let height_mode = cursor.g1b()?;
        let x_mode = cursor.g1b()?;
        let y_mode = cursor.g1b()?;
        let aspect = if width_mode == self.aspect_mode || height_mode == self.aspect_mode {
            Some((cursor.g2()?, cursor.g2()?))
        } else {
            None
        };
        let parent = cursor.g2()?;
        let flags = cursor.g1()?;
        let body_start = cursor.pos();
        let text = self.text(&mut cursor, version)?;
        let style = if version >= self.style_version {
            let style = cursor.g4s()? as i16;
            (style != self.absent_style).then_some(style)
        } else {
            None
        };
        Ok(Some(TextFrame {
            version,
            name_bytes,
            client_code,
            geometry: Geometry {
                x,
                y,
                width,
                height,
                width_mode,
                height_mode,
                x_mode,
                y_mode,
                aspect,
            },
            parent,
            flags,
            body_start,
            text,
            style,
            consumed: cursor.pos(),
            remaining: cursor.remaining(),
        }))
    }
}
fn skip_string(cursor: &mut Cursor<'_>) -> Result<usize> {
    let start = cursor.pos();
    while cursor.g1()? != TERMINATOR {}
    Ok(cursor.pos() - start - usize::from(true))
}
#[derive(Serialize)]
struct Geometry {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    width_mode: i8,
    height_mode: i8,
    x_mode: i8,
    y_mode: i8,
    aspect: Option<(u16, u16)>,
}
#[derive(Serialize)]
struct TextState {
    font: i32,
    monospaced: bool,
    text_bytes: usize,
    line_height: u8,
    horizontal_align: u8,
    vertical_align: u8,
    shadow: bool,
    colour: u32,
    alpha: u8,
    max_lines: u8,
}
#[derive(Serialize)]
struct TextFrame {
    version: i32,
    name_bytes: Option<usize>,
    client_code: u16,
    geometry: Geometry,
    parent: u16,
    flags: u8,
    body_start: usize,
    text: TextState,
    style: Option<i16>,
    consumed: usize,
    remaining: usize,
}
struct Frame {
    sha256: String,
    value: std::result::Result<TextFrame, String>,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub build: Build,
    pub client_md5: String,
    pub schema_sha256: String,
    pub interface_index_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_index_sha256: Option<String>,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StyleFont {
    pub style: i16,
    pub source_sha256: String,
    pub font: Option<i32>,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FontContract {
    pub frame: FrameRef,
    pub source_sha256: String,
    pub font: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_style: Option<StyleFont>,
}
pub struct Definitions {
    identity: Identity,
    frames: BTreeMap<FrameRef, Frame>,
    other_components: usize,
    styles: styles::Definitions,
}
impl Definitions {
    pub fn load(root: &Path, book: &Book, schema: &[u8]) -> Result<Self> {
        let profile = Profile::parse(schema, book)?;
        corpus::load_index(root, book)?;
        let container = std::fs::read(
            root.join(INDEX_ARCHIVE.to_string())
                .join(format!("{}.dat", profile.interface_archive)),
        )?;
        ensure!(
            digest(&container) == profile.interface_index_sha256,
            "interface index does not match the frame profile"
        );
        let index = ArchiveIndex::decode(&decompress(&container)?)?;
        let mut result = Self::empty(
            book,
            schema,
            Some(profile.interface_index_sha256.clone()),
            &profile,
        );
        result.styles = styles::Definitions::load(root, &profile.styles)?;
        result.identity.style_index_sha256 = Some(profile.styles.index_sha256.clone());
        for interface in &index.group_id {
            for (file, bytes) in
                corpus::load_group(root, profile.interface_archive, &index, *interface)?
            {
                result.insert(
                    &profile,
                    FrameRef {
                        interface: *interface,
                        file,
                    },
                    &bytes,
                )?;
            }
        }
        Ok(result)
    }
    fn empty(book: &Book, schema: &[u8], index: Option<String>, profile: &Profile) -> Self {
        Self {
            identity: Identity {
                build: book.profile.build,
                client_md5: book.profile.client_md5.clone(),
                schema_sha256: digest(schema),
                interface_index_sha256: index,
                style_index_sha256: None,
            },
            frames: BTreeMap::new(),
            other_components: usize::default(),
            styles: styles::Definitions::empty(&profile.styles),
        }
    }
    fn insert(&mut self, profile: &Profile, frame: FrameRef, bytes: &[u8]) -> Result<()> {
        let value = match profile.component(bytes) {
            Ok(None) => {
                self.other_components += usize::from(true);
                return Ok(());
            }
            Ok(Some(value)) => Ok(value),
            Err(error) => Err(error.to_string()),
        };
        ensure!(
            self.frames
                .insert(
                    frame,
                    Frame {
                        sha256: digest(bytes),
                        value
                    }
                )
                .is_none(),
            "duplicate donor frame"
        );
        Ok(())
    }
    pub fn identity(&self) -> Identity {
        self.identity.clone()
    }
    pub fn font_contract(&self, frame: FrameRef) -> Result<FontContract> {
        let source = self
            .frames
            .get(&frame)
            .context("source text frame is absent")?;
        let value = source
            .value
            .as_ref()
            .map_err(|error| anyhow::anyhow!("source text frame decode failed: {error}"))?;
        let source_style = value
            .style
            .map(|style| self.styles.font(style))
            .transpose()?;
        Ok(FontContract {
            frame,
            source_sha256: source.sha256.clone(),
            font: source_style
                .as_ref()
                .and_then(|style| style.font)
                .unwrap_or(value.text.font),
            source_style,
        })
    }
    pub fn report(&self, frame: Option<FrameRef>) -> Result<serde_json::Value> {
        if let Some(frame) = frame {
            ensure!(
                self.frames.contains_key(&frame),
                "source text frame is absent"
            );
        }
        let rows:Vec<_> = self.frames.iter().filter(|(key,_)|frame.is_none_or(|frame|frame == **key)).map(|(frame,source)| {
            let contract = self.font_contract(*frame);
            serde_json::json!({"frame":frame,"source_sha256":source.sha256,"prefix":source.value.as_ref().ok(),
                "source_style":source.value.as_ref().ok().and_then(|frame|frame.style).map(|style|self.styles.inspect(style)),"initial_font":contract.as_ref().ok(),"gap":contract.err().map(|error|error.to_string())})
        }).collect();
        Ok(
            serde_json::json!({"format":FORMAT,"identity":self.identity,"text_frames":self.frames.len(),
            "other_components":self.other_components,"initial_fonts":self.frames.keys().filter(|frame|self.font_contract(**frame).is_ok()).count(),
            "decode_failures":self.frames.values().filter(|source|source.value.is_err()).count(),
            "style_definitions":self.styles.summary(),
            "style_gaps":self.frames.iter().filter(|(frame,source)|source.value.as_ref().is_ok_and(|value|value.style.is_some()) && self.font_contract(**frame).is_err()).count(),
            "scope":"Primitive text prefix, numeric styles and direct initial fonts; remaining component fields and runtime style parent links are not sealed", "frames":rows}),
        )
    }
    #[cfg(feature = "test-hooks")]
    pub fn authored_fixture(
        book: &Book,
        schema: &[u8],
        files: &BTreeMap<FrameRef, Vec<u8>>,
        style_files: &BTreeMap<i32, Vec<u8>>,
    ) -> Result<Self> {
        let profile = Profile::parse(schema, book)?;
        let mut result = Self::empty(book, schema, None, &profile);
        result.styles = styles::Definitions::authored_fixture(&profile.styles, style_files)?;
        for (frame, bytes) in files {
            result.insert(&profile, *frame, bytes)?;
        }
        Ok(result)
    }
}
#[cfg(test)]
pub(crate) fn verify(book: &Book) {
    let schema = include_bytes!("../../../revisions/950/cs2/frames.json");
    let profile = Profile::parse(schema, book).unwrap();
    styles::verify(&profile.styles, &book.profile.client_md5);
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../fixtures/initial-frames.json")).unwrap();
    assert_eq!(fixture["client_md5"], book.profile.client_md5);
    for (address, hash) in &profile.kernels {
        assert_eq!(&fixture["kernels"][address], hash);
    }
    for case in fixture["text_bodies"].as_array().unwrap() {
        let bytes: Vec<u8> = serde_json::from_value(case["wire"].clone()).unwrap();
        let mut cursor = Cursor::new(&bytes);
        let state = profile
            .text(
                &mut cursor,
                i32::try_from(case["version"].as_i64().unwrap()).unwrap(),
            )
            .unwrap();
        let actual = serde_json::to_value(state).unwrap();
        for (name, value) in actual.as_object().unwrap() {
            let recorded = if name == "monospaced" { "mono" } else { name };
            if recorded != "text_bytes" {
                assert_eq!(value, &case["result"][recorded], "{recorded}");
            }
        }
        assert_eq!(serde_json::json!(cursor.pos()), case["result"]["consumed"]);
        for end in usize::default()..bytes.len() {
            assert!(
                profile
                    .text(
                        &mut Cursor::new(&bytes[..end]),
                        i32::try_from(case["version"].as_i64().unwrap()).unwrap()
                    )
                    .is_err()
            );
        }
    }
    let frame: FrameRef = serde_json::from_value(fixture["authored_frame"].clone()).unwrap();
    for case in fixture["common_headers"].as_array().unwrap() {
        let version = u8::try_from(case["version"].as_u64().unwrap()).unwrap();
        let mut bytes = vec![version, profile.text_type, u8::default(), u8::default()];
        bytes.extend(serde_json::from_value::<Vec<u8>>(case["wire"].clone()).unwrap());
        let decoded = profile.component(&bytes).unwrap().unwrap();
        let mut definitions = Definitions::empty(book, schema, None, &profile);
        definitions.insert(&profile, frame, &bytes).unwrap();
        assert_eq!(
            definitions.font_contract(frame).is_err(),
            decoded.style.is_some()
        );
        const FACTORY_PREFIX_BYTES: usize = 4;
        assert_eq!(
            serde_json::json!(decoded.body_start - FACTORY_PREFIX_BYTES),
            case["result"]["body_start"][0]
        );
        assert_eq!(
            serde_json::json!(decoded.consumed - FACTORY_PREFIX_BYTES),
            case["result"]["consumed"]
        );
        assert_eq!(serde_json::json!(decoded.text.font), case["result"]["font"]);
        assert_eq!(
            serde_json::json!(decoded.style.unwrap_or(profile.absent_style)),
            case["result"]["style"]
        );
        for end in usize::default()..bytes.len() {
            assert!(profile.component(&bytes[..end]).is_err());
        }
    }
    let wire: Vec<u8> = serde_json::from_value(fixture["authored_wire"].clone()).unwrap();
    let mut definitions = Definitions::empty(book, schema, None, &profile);
    definitions.insert(&profile, frame, &wire).unwrap();
    assert_eq!(
        definitions.font_contract(frame).unwrap().source_sha256,
        digest(&wire)
    );
    let styled: serde_json::Value =
        serde_json::from_slice(include_bytes!("../fixtures/style-fonts.json")).unwrap();
    let styled_wire: Vec<u8> = serde_json::from_value(styled["authored_wire"].clone()).unwrap();
    let mut styled_definitions = Definitions::empty(book, schema, None, &profile);
    styled_definitions.styles = styles::Definitions::authored_fixture(
        &profile.styles,
        &serde_json::from_value(styled["authored_styles"].clone()).unwrap(),
    )
    .unwrap();
    styled_definitions
        .insert(&profile, frame, &styled_wire)
        .unwrap();
    let styled_font = styled_definitions.font_contract(frame).unwrap();
    assert_eq!(
        serde_json::json!(styled_font.font),
        styled["authored_initial_font"]
    );
    assert_eq!(
        styled_font.source_style.unwrap().font,
        Some(styled_font.font)
    );
    let mut foreign: serde_json::Value = serde_json::from_slice(schema).unwrap();
    foreign["client_md5"] = book
        .profile
        .client_md5
        .chars()
        .rev()
        .collect::<String>()
        .into();
    assert!(Profile::parse(&serde_json::to_vec(&foreign).unwrap(), book).is_err());
}
