//! Compiler bookkeeping is separate from source instruction and call budgets.
//! Identities pin the exact emitted bytes; only bounded linear adapters and
//! entry wrappers can execute without advancing the source counter.
use crate::{
    error::{NativeError, Result},
    opcode::OpcodeBook,
    script::{CompiledScript, Operand, decode_script, encode_script},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    sync::Arc,
};

pub const PROJECT_FILENAME: &str = "execution.tsv";
pub const SCRIPT_FILE: u32 = 0;
pub const METADATA_FILE: u32 = 1;
const FORMAT: &str = "execution-format 1";
const SHA256_HEX_WIDTH: usize = 64;
const ONE_CALL: usize = 1;
pub(crate) const ENTRY_COMPONENT_CONSTANT: usize = 0;
pub(crate) const ENTRY_COMPONENT_SELECT: usize = 1;
pub(crate) const ENTRY_COMPONENT_DISCARD: usize = 2;
const HEX_BYTE_WIDTH: usize = 2;
const HEX_RADIX: u32 = 16;
pub type ResourceDigest = [u8; SHA256_HEX_WIDTH / HEX_BYTE_WIDTH];
const ENUM_ARGUMENTS: u16 = 4;
const FIELD_ARGUMENTS: u16 = 3;
const COUNT_ARGUMENTS: u16 = 2;
const IMPORT_NAME_PREFIX: &str = "proc,__alto_import_";
const IMPORT_NAME_SEPARATOR: &str = "__";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Accounting {
    identity: Option<String>,
    pub role: Role,
    pub transparent_calls: BTreeSet<usize>,
    pub host_operations: BTreeMap<usize, HostOperation>,
    pub resource: Option<Resource>,
}
impl Default for Accounting {
    fn default() -> Self {
        Self {
            identity: None,
            role: Role::Source,
            transparent_calls: BTreeSet::new(),
            host_operations: BTreeMap::new(),
            resource: None,
        }
    }
}

pub(crate) fn import_signature(script: &CompiledScript) -> Option<&str> {
    script
        .name
        .as_deref()?
        .strip_prefix(IMPORT_NAME_PREFIX)?
        .split_once(IMPORT_NAME_SEPARATOR)
        .map(|(signature, _)| signature)
}

impl Accounting {
    /// Imported bytecode carries the identity of the metadata it requires.
    pub fn validate_script(&self, script: &CompiledScript) -> Result<()> {
        if let Some(name) = script
            .name
            .as_deref()
            .and_then(|name| name.strip_prefix(IMPORT_NAME_PREFIX))
        {
            let signature = name
                .split_once(IMPORT_NAME_SEPARATOR)
                .map(|(signature, _)| signature);
            if signature.is_none() || signature != self.identity.as_deref() {
                return Err(invalid(
                    "imported script execution metadata missing or changed; rebuild the import",
                ));
            }
        }
        Ok(())
    }
}

/// Opaque immutable resource bound to an imported script's metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resource {
    bytes: Arc<[u8]>,
    digest: ResourceDigest,
}
impl Resource {
    fn new(bytes: Arc<[u8]>) -> Self {
        Self {
            digest: Sha256::digest(&bytes).into(),
            bytes,
        }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn digest(&self) -> ResourceDigest {
        self.digest
    }
    fn digest_hex(&self) -> String {
        self.digest
            .iter()
            .fold(String::with_capacity(SHA256_HEX_WIDTH), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            })
    }
}

/// Execution behavior bound to newly imported bytecode.
pub struct Specification {
    pub role: Role,
    pub adapter_calls: BTreeMap<usize, i32>,
    pub host_operations: BTreeMap<usize, HostOperation>,
    pub resource: Option<Arc<[u8]>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Source,
    Adapter,
    Entry,
}
impl Role {
    fn spelling(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Adapter => "adapter",
            Self::Entry => "entry",
        }
    }
}
/// Portable host behavior attached to a generated use, independent of wire opcodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostOperation {
    ComponentText {
        property: TextProperty,
        text_type: i32,
        explicit: bool,
    },
    ComponentPaint {
        property: PaintProperty,
        explicit: bool,
    },
    VariableBit {
        shift: u8,
        mask: i32,
        default: i32,
    },
    Enum {
        output_type: i32,
        string_type: u16,
    },
    /// Proven source stack traffic for a retained player-variable installer.
    RetainedPlayerTransmit {
        pops: [u16; 3],
        event_tokens: VariableEventTokens,
    },
    FindComponent,
    FindFlatChild {
        limit: u16,
        slot_argument: u16,
    },
    CreateFlatTextChild {
        limit: u16,
        kind_argument: u16,
        slot_argument: u16,
        source: Option<(i32, usize)>,
    },
    ClearRuntimeChildren,
    DatabaseField {
        field: i32,
        pushes: [u16; 3],
    },
    DatabaseFieldCount {
        field: i32,
    },
    NextRuntimeChildSlot {
        banks: RuntimeChildBanks,
        bank_argument: u16,
    },
    FindRuntimeChild {
        banks: RuntimeChildBanks,
        bank_argument: u16,
        slot_argument: u16,
    },
}

/// Contiguous integer tokens of a verified variable-event loader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VariableEventTokens {
    first: i32,
    count: u8,
}
impl VariableEventTokens {
    /// The consumer provides eleven component and default event fields.
    pub const CAPACITY: u8 = 11;
    pub fn new(first: i32, count: u8) -> Result<Self> {
        if count == u8::default()
            || count > Self::CAPACITY
            || first >= i32::default()
            || first
                .checked_add(i32::from(count - u8::from(true)))
                .is_none_or(|last| last >= i32::default())
        {
            return Err(invalid("invalid variable event token layout"));
        }
        Ok(Self { first, count })
    }
    pub fn first(self) -> i32 {
        self.first
    }
    pub fn count(self) -> u8 {
        self.count
    }
}

/// Property consumers retain the source revision's change and class policies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaintProperty {
    Colour { change_kind: u8 },
    Fill { rectangle_type: i32 },
    Transparency,
}

/// Asset identities belong to an exact source client, even when target assets alias.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentDomain([u8; Self::BYTE_WIDTH]);
impl ContentDomain {
    const BYTE_WIDTH: usize = 16;
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() != Self::BYTE_WIDTH * HEX_BYTE_WIDTH {
            return Err(invalid("content domain needs a client MD5"));
        }
        let mut bytes = [u8::default(); Self::BYTE_WIDTH];
        for (destination, source) in bytes
            .iter_mut()
            .zip(text.as_bytes().chunks_exact(HEX_BYTE_WIDTH))
        {
            let source =
                std::str::from_utf8(source).map_err(|_| invalid("invalid content domain"))?;
            *destination = u8::from_str_radix(source, HEX_RADIX)
                .map_err(|_| invalid("invalid content domain"))?;
        }
        Ok(Self(bytes))
    }
    pub fn spelling(self) -> String {
        self.0.iter().fold(
            String::with_capacity(Self::BYTE_WIDTH * HEX_BYTE_WIDTH),
            |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            },
        )
    }
}

/// A font binding changes only one consumer, leaving donor values intact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FontMapping {
    pub source: i32,
    pub target: i32,
}

/// An imported font use is either awaiting authoring, constant, or resource-bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontBinding {
    Unbound,
    Constant(FontMapping),
    Resource,
}

/// Closed source-to-target asset choices attached to one imported consumer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontMappings {
    targets: BTreeMap<i32, i32>,
}
impl FontMappings {
    const HEADER: &'static str = "font-map 1\n";

    pub fn new(mappings: impl IntoIterator<Item = FontMapping>) -> Result<Self> {
        let mut targets = BTreeMap::new();
        for mapping in mappings {
            if targets.insert(mapping.source, mapping.target).is_some() {
                return Err(invalid("duplicate source font in a font map"));
            }
        }
        if targets.is_empty() {
            return Err(invalid("font map is empty"));
        }
        Ok(Self { targets })
    }
    pub fn get(&self, source: i32) -> Option<FontMapping> {
        self.targets.get(&source).map(|target| FontMapping {
            source,
            target: *target,
        })
    }
    pub fn resource(&self) -> Vec<u8> {
        let mut text = Self::HEADER.to_owned();
        for (source, target) in &self.targets {
            let _ = writeln!(text, "{source} {target}");
        }
        text.into_bytes()
    }
    pub fn decode_resource(bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes).map_err(|_| invalid("font map is not UTF-8"))?;
        let rows = text
            .strip_prefix(Self::HEADER)
            .ok_or_else(|| invalid("font map header missing"))?;
        let mappings = rows
            .lines()
            .map(|row| {
                let (source, target) = row
                    .split_once(' ')
                    .ok_or_else(|| invalid("invalid font map row"))?;
                Ok(FontMapping {
                    source: number(source)?,
                    target: number(target)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let map = Self::new(mappings)?;
        if map.resource() != bytes {
            return Err(invalid("font map is not canonical"));
        }
        Ok(map)
    }
}

/// A declaration of the donor font for one freshly loaded target frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InitialFontMapping {
    pub component: i32,
    pub mapping: FontMapping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontReader {
    Graphic,
    Metrics,
}
impl FontReader {
    fn family(self) -> &'static str {
        match self {
            Self::Graphic => "read-font",
            Self::Metrics => "read-font-metrics",
        }
    }
    fn parse(family: &str) -> Result<Self> {
        match family {
            "read-font" => Ok(Self::Graphic),
            "read-font-metrics" => Ok(Self::Metrics),
            _ => Err(invalid("unknown font reader")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextProperty {
    Font {
        domain: ContentDomain,
        change_kind: u8,
        mapping: FontBinding,
    },
    ReadFont {
        reader: FontReader,
        domain: ContentDomain,
        absent: i32,
        initial: Option<InitialFontMapping>,
    },
    Alignment,
    MaxLines,
}
impl TextProperty {
    pub fn domain(self) -> Option<ContentDomain> {
        match self {
            Self::Font { domain, .. } | Self::ReadFont { domain, .. } => Some(domain),
            _ => None,
        }
    }
    pub fn returns_integer(self) -> bool {
        matches!(self, Self::ReadFont { .. })
    }
    pub fn integer_arguments(self) -> u16 {
        const ALIGNMENT_ARGUMENTS: u16 = 3;
        match self {
            Self::ReadFont { .. } => u16::default(),
            Self::Alignment => ALIGNMENT_ARGUMENTS,
            _ => u16::from(true),
        }
    }
}

/// Bank widths and count come from the source revision's reviewed adapter.
/// The reserved all-ones component identity is never part of this namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeChildBanks {
    bank_count: u16,
    root_slots: u16,
    other_slots: u16,
}
impl RuntimeChildBanks {
    pub fn new(bank_count: u16, root_slots: u16, other_slots: u16) -> Result<Self> {
        if bank_count == u16::default()
            || root_slots == u16::default()
            || other_slots == u16::default()
            || u32::from(root_slots) + u32::from(bank_count - 1) * u32::from(other_slots)
                > u32::from(u16::MAX)
        {
            return Err(invalid("invalid runtime child bank layout"));
        }
        Ok(Self {
            bank_count,
            root_slots,
            other_slots,
        })
    }
    pub fn bank_count(self) -> u16 {
        self.bank_count
    }
    /// Encoded start and slot capacity for a valid bank.
    pub fn range(self, bank: i32) -> Result<(u16, u16)> {
        let bank = u16::try_from(bank).map_err(|_| invalid("invalid runtime child bank"))?;
        if bank >= self.bank_count {
            return Err(invalid("invalid runtime child bank"));
        }
        Ok(if bank == u16::default() {
            (u16::default(), self.root_slots)
        } else {
            (
                self.root_slots + (bank - 1) * self.other_slots,
                self.other_slots,
            )
        })
    }
    pub fn encode(self, bank: i32, slot: i32) -> Result<u16> {
        let (start, capacity) = self.range(bank)?;
        let slot = u16::try_from(slot).map_err(|_| invalid("invalid runtime child slot"))?;
        if slot >= capacity {
            return Err(invalid("invalid runtime child slot"));
        }
        Ok(start + slot)
    }
    pub fn decode(self, encoded: u16) -> Option<(u16, u16)> {
        if encoded < self.root_slots {
            return Some((u16::default(), encoded));
        }
        let offset = encoded - self.root_slots;
        let bank = offset / self.other_slots + 1;
        (bank < self.bank_count).then_some((bank, offset % self.other_slots))
    }
}

impl HostOperation {
    pub fn spelling(self) -> String {
        match self {
            Self::ComponentText {
                property,
                text_type,
                explicit,
            } => {
                let selector = if explicit { "explicit" } else { "active" };
                match property {
                    TextProperty::Font {
                        domain,
                        change_kind,
                        mapping,
                    } => {
                        let (source, target) = match mapping {
                            FontBinding::Unbound => ("unbound".into(), "unbound".into()),
                            FontBinding::Resource => ("resource".into(), "resource".into()),
                            FontBinding::Constant(mapping) => {
                                (mapping.source.to_string(), mapping.target.to_string())
                            }
                        };
                        format!(
                            "component-text/font/{text_type}/{change_kind}/{}/{source}/{target}/{selector}",
                            domain.spelling()
                        )
                    }
                    TextProperty::ReadFont {
                        reader,
                        domain,
                        absent,
                        initial,
                    } => {
                        let declaration = initial.map_or_else(String::new, |initial| {
                            format!(
                                "/{}/{}/{}",
                                initial.component, initial.mapping.source, initial.mapping.target
                            )
                        });
                        format!(
                            "component-text/{}/{text_type}/{}/{absent}{declaration}/{selector}",
                            reader.family(),
                            domain.spelling()
                        )
                    }
                    TextProperty::Alignment => {
                        format!("component-text/alignment/{text_type}/{selector}")
                    }
                    TextProperty::MaxLines => {
                        format!("component-text/max-lines/{text_type}/{selector}")
                    }
                }
            }
            Self::ComponentPaint { property, explicit } => {
                let selector = if explicit { "explicit" } else { "active" };
                match property {
                    PaintProperty::Colour { change_kind } => {
                        format!("component-paint/colour/{change_kind}/{selector}")
                    }
                    PaintProperty::Fill { rectangle_type } => {
                        format!("component-paint/fill/{rectangle_type}/{selector}")
                    }
                    PaintProperty::Transparency => {
                        format!("component-paint/transparency/{selector}")
                    }
                }
            }
            Self::VariableBit {
                shift,
                mask,
                default,
            } => format!("variable-bit/{shift}/{mask}/{default}"),
            Self::Enum {
                output_type,
                string_type,
            } => format!("enum/{output_type}/{string_type}"),
            Self::DatabaseField { field, pushes } => format!(
                "database-field/{field}/{}/{}/{}",
                pushes[0], pushes[1], pushes[2]
            ),
            Self::DatabaseFieldCount { field } => format!("database-field-count/{field}"),
            Self::RetainedPlayerTransmit { pops, event_tokens } => format!(
                "retained-player-transmit/{}/{}/{}/{}/{}",
                pops[0],
                pops[1],
                pops[2],
                event_tokens.first(),
                event_tokens.count()
            ),
            Self::FindComponent => "find-component".into(),
            Self::FindFlatChild {
                limit,
                slot_argument,
            } => {
                format!("find-flat-child/{limit}/{slot_argument}")
            }
            Self::CreateFlatTextChild {
                limit,
                kind_argument,
                slot_argument,
                source,
            } => {
                let (script, pc) = source.map_or_else(
                    || ("unbound".into(), usize::default()),
                    |(script, pc)| (script.to_string(), pc),
                );
                format!(
                    "create-flat-text-child/{limit}/{kind_argument}/{slot_argument}/{script}/{pc}"
                )
            }
            Self::ClearRuntimeChildren => "clear-runtime-children".into(),
            Self::FindRuntimeChild {
                banks,
                bank_argument,
                slot_argument,
            } => format!(
                "find-runtime-child/{}/{}/{}/{}/{}",
                banks.bank_count, banks.root_slots, banks.other_slots, bank_argument, slot_argument
            ),
            Self::NextRuntimeChildSlot {
                banks,
                bank_argument,
            } => format!(
                "next-runtime-child-slot/{}/{}/{}/{}",
                banks.bank_count, banks.root_slots, banks.other_slots, bank_argument
            ),
        }
    }
    pub fn parse(text: &str) -> Result<Self> {
        if text == "find-component" {
            return Ok(Self::FindComponent);
        }
        if text == "clear-runtime-children" {
            return Ok(Self::ClearRuntimeChildren);
        }
        let fields: Vec<_> = text.split('/').collect();
        match fields.as_slice() {
            [
                "component-text",
                "font",
                text_type,
                kind,
                domain,
                source,
                target,
                selector,
            ] => {
                let mapping = match (*source, *target) {
                    ("unbound", "unbound") => FontBinding::Unbound,
                    ("resource", "resource") => FontBinding::Resource,
                    _ => FontBinding::Constant(FontMapping {
                        source: number(source)?,
                        target: number(target)?,
                    }),
                };
                text_operation(
                    TextProperty::Font {
                        domain: ContentDomain::parse(domain)?,
                        change_kind: number(kind)?,
                        mapping,
                    },
                    number(text_type)?,
                    selector,
                    text,
                )
            }
            [
                "component-text",
                family @ ("read-font" | "read-font-metrics"),
                text_type,
                domain,
                absent,
                selector,
            ] => text_operation(
                TextProperty::ReadFont {
                    reader: FontReader::parse(family)?,
                    domain: ContentDomain::parse(domain)?,
                    absent: number(absent)?,
                    initial: None,
                },
                number(text_type)?,
                selector,
                text,
            ),
            [
                "component-text",
                family @ ("read-font" | "read-font-metrics"),
                text_type,
                domain,
                absent,
                component,
                source,
                target,
                selector,
            ] => text_operation(
                TextProperty::ReadFont {
                    reader: FontReader::parse(family)?,
                    domain: ContentDomain::parse(domain)?,
                    absent: number(absent)?,
                    initial: Some(InitialFontMapping {
                        component: number(component)?,
                        mapping: FontMapping {
                            source: number(source)?,
                            target: number(target)?,
                        },
                    }),
                },
                number(text_type)?,
                selector,
                text,
            ),
            ["component-text", "alignment", text_type, selector] => {
                text_operation(TextProperty::Alignment, number(text_type)?, selector, text)
            }
            ["component-text", "max-lines", text_type, selector] => {
                text_operation(TextProperty::MaxLines, number(text_type)?, selector, text)
            }
            ["component-paint", "colour", kind, selector] => paint_operation(
                PaintProperty::Colour {
                    change_kind: number(kind)?,
                },
                selector,
                text,
            ),
            ["component-paint", "fill", kind, selector] => paint_operation(
                PaintProperty::Fill {
                    rectangle_type: number(kind)?,
                },
                selector,
                text,
            ),
            ["component-paint", "transparency", selector] => {
                paint_operation(PaintProperty::Transparency, selector, text)
            }
            ["variable-bit", shift, mask, default] => {
                let operation = Self::VariableBit {
                    shift: number(shift)?,
                    mask: number(mask)?,
                    default: number(default)?,
                };
                if operation.spelling() != text || number::<u32>(shift)? >= i32::BITS {
                    return Err(invalid("invalid variable bit operation"));
                }
                Ok(operation)
            }
            ["enum", output_type, string_type] => {
                let operation = Self::Enum {
                    output_type: number(output_type)?,
                    string_type: number(string_type)?,
                };
                if operation.spelling() != text {
                    return Err(invalid("noncanonical enum operation"));
                }
                Ok(operation)
            }
            [
                "retained-player-transmit",
                ints,
                objects,
                longs,
                first,
                count,
            ] => {
                let operation = Self::RetainedPlayerTransmit {
                    pops: [number(ints)?, number(objects)?, number(longs)?],
                    event_tokens: VariableEventTokens::new(number(first)?, number(count)?)?,
                };
                if operation.spelling() != text
                    || number::<u16>(ints)? == u16::default()
                    || number::<u16>(objects)? == u16::default()
                {
                    return Err(invalid("invalid retained player transmit operation"));
                }
                Ok(operation)
            }
            ["database-field", field, ints, objects, longs] => {
                let operation = Self::DatabaseField {
                    field: number(field)?,
                    pushes: [number(ints)?, number(objects)?, number(longs)?],
                };
                if operation.spelling() != text {
                    return Err(invalid("noncanonical database operation"));
                }
                Ok(operation)
            }
            ["database-field-count", field] => {
                let operation = Self::DatabaseFieldCount {
                    field: number(field)?,
                };
                if operation.spelling() != text {
                    return Err(invalid("noncanonical database operation"));
                }
                Ok(operation)
            }
            ["create-flat-text-child", limit, kind, slot, script, pc] => {
                let operation = Self::CreateFlatTextChild {
                    limit: number(limit)?,
                    kind_argument: number(kind)?,
                    slot_argument: number(slot)?,
                    source: if *script == "unbound" {
                        None
                    } else {
                        Some((number(script)?, number(pc)?))
                    },
                };
                if operation.spelling() != text
                    || number::<u16>(limit)? == u16::default()
                    || operation
                        .creation_source()
                        .is_some_and(|(script, _)| script < i32::default())
                {
                    return Err(invalid("invalid text creation operation"));
                }
                Ok(operation)
            }
            ["find-flat-child", limit, argument] => {
                let operation = Self::FindFlatChild {
                    limit: number(limit)?,
                    slot_argument: number(argument)?,
                };
                if operation.spelling() != text || number::<u16>(limit)? == u16::default() {
                    return Err(invalid("invalid flat child operation"));
                }
                Ok(operation)
            }
            ["next-runtime-child-slot", count, root, other, argument] => {
                let operation = Self::NextRuntimeChildSlot {
                    banks: RuntimeChildBanks::new(number(count)?, number(root)?, number(other)?)?,
                    bank_argument: number(argument)?,
                };
                if operation.spelling() != text {
                    return Err(invalid("noncanonical host operation"));
                }
                Ok(operation)
            }
            [
                "find-runtime-child",
                count,
                root,
                other,
                bank_argument,
                slot_argument,
            ] => {
                let operation = Self::FindRuntimeChild {
                    banks: RuntimeChildBanks::new(number(count)?, number(root)?, number(other)?)?,
                    bank_argument: number(bank_argument)?,
                    slot_argument: number(slot_argument)?,
                };
                if operation.spelling() != text {
                    return Err(invalid("noncanonical host operation"));
                }
                Ok(operation)
            }
            _ => Err(invalid("unknown host operation")),
        }
    }
    pub fn creation_source(self) -> Option<(i32, usize)> {
        match self {
            Self::CreateFlatTextChild { source, .. } => source,
            _ => None,
        }
    }
    pub fn command(self) -> &'static str {
        match self {
            Self::ComponentText {
                property, explicit, ..
            } => match (property, explicit) {
                (TextProperty::Font { .. }, false) => "cc_settextfont",
                (TextProperty::Font { .. }, true) => "if_settextfont",
                (
                    TextProperty::ReadFont {
                        reader: FontReader::Graphic,
                        ..
                    },
                    false,
                ) => "cc_getfontgraphic",
                (
                    TextProperty::ReadFont {
                        reader: FontReader::Graphic,
                        ..
                    },
                    true,
                ) => "if_getfontgraphic",
                (
                    TextProperty::ReadFont {
                        reader: FontReader::Metrics,
                        ..
                    },
                    false,
                ) => "cc_getfontmetrics",
                (
                    TextProperty::ReadFont {
                        reader: FontReader::Metrics,
                        ..
                    },
                    true,
                ) => "if_getfontmetrics",
                (TextProperty::Alignment, false) => "cc_settextalign",
                (TextProperty::Alignment, true) => "if_settextalign",
                (TextProperty::MaxLines, false) => "cc_setmaxlines",
                (TextProperty::MaxLines, true) => "if_setmaxlines",
            },
            Self::ComponentPaint { property, explicit } => match (property, explicit) {
                (PaintProperty::Colour { .. }, false) => "cc_setcolour",
                (PaintProperty::Colour { .. }, true) => "if_setcolour",
                (PaintProperty::Fill { .. }, false) => "cc_setfill",
                (PaintProperty::Fill { .. }, true) => "if_setfill",
                (PaintProperty::Transparency, false) => "cc_settrans",
                (PaintProperty::Transparency, true) => "if_settrans",
            },
            Self::RetainedPlayerTransmit { .. } => "cc_setonvartransmit",
            Self::VariableBit { .. } => "push_var",
            Self::Enum { .. } => "_enum",
            Self::DatabaseField { .. } => "db_getfield",
            Self::DatabaseFieldCount { .. } => "db_getfieldcount",
            Self::CreateFlatTextChild { .. } => "cc_create",
            Self::ClearRuntimeChildren => "cc_deleteall",
            Self::NextRuntimeChildSlot { .. } => "if_getnextsubid",
            Self::FindComponent | Self::FindRuntimeChild { .. } | Self::FindFlatChild { .. } => {
                "if_find"
            }
        }
    }
    pub fn needs_resource(self) -> bool {
        matches!(
            self,
            Self::Enum { .. }
                | Self::DatabaseField { .. }
                | Self::DatabaseFieldCount { .. }
                | Self::CreateFlatTextChild { .. }
                | Self::ComponentText {
                    property: TextProperty::Font {
                        mapping: FontBinding::Resource,
                        ..
                    },
                    ..
                }
        )
    }
    pub fn effect(self, operand: &Operand) -> crate::semantics::Effect {
        match self {
            Self::ComponentText {
                property, explicit, ..
            } => crate::semantics::Effect::Fixed {
                pops: [property.integer_arguments() + u16::from(explicit), 0, 0],
                pushes: [u16::from(property.returns_integer()), 0, 0],
            },
            Self::CreateFlatTextChild { .. } => crate::semantics::Effect::Fixed {
                pops: [u16::from(true), 0, 0],
                pushes: [0, 0, 0],
            },
            Self::RetainedPlayerTransmit { pops, .. } => crate::semantics::Effect::Fixed {
                pops,
                pushes: [0, 0, 0],
            },
            Self::VariableBit { .. } => crate::semantics::Effect::Fixed {
                pops: [0, 0, 0],
                pushes: [1, 0, 0],
            },
            Self::Enum {
                output_type,
                string_type,
            } => crate::semantics::Effect::Fixed {
                pops: [ENUM_ARGUMENTS, 0, 0],
                pushes: if output_type == i32::from(string_type) {
                    [0, 1, 0]
                } else {
                    [1, 0, 0]
                },
            },
            Self::DatabaseField { pushes, .. } => crate::semantics::Effect::Fixed {
                pops: [FIELD_ARGUMENTS, 0, 0],
                pushes,
            },
            Self::DatabaseFieldCount { .. } => crate::semantics::Effect::Fixed {
                pops: [COUNT_ARGUMENTS, 0, 0],
                pushes: [1, 0, 0],
            },
            _ => crate::semantics::effect(self.command(), operand),
        }
    }
}

fn text_operation(
    property: TextProperty,
    text_type: i32,
    selector: &str,
    text: &str,
) -> Result<HostOperation> {
    let explicit = match selector {
        "active" => false,
        "explicit" => true,
        _ => return Err(invalid("invalid component text selector")),
    };
    let operation = HostOperation::ComponentText {
        property,
        text_type,
        explicit,
    };
    if operation.spelling() != text {
        return Err(invalid("noncanonical component text operation"));
    }
    Ok(operation)
}

fn paint_operation(property: PaintProperty, selector: &str, text: &str) -> Result<HostOperation> {
    let explicit = match selector {
        "active" => false,
        "explicit" => true,
        _ => return Err(invalid("invalid component paint selector")),
    };
    let operation = HostOperation::ComponentPaint { property, explicit };
    if operation.spelling() != text {
        return Err(invalid("noncanonical component paint operation"));
    }
    Ok(operation)
}

#[derive(Clone, Debug)]
struct Identity {
    sha256: String,
    role: Role,
    adapter_calls: BTreeMap<usize, i32>,
    host_operations: BTreeMap<usize, HostOperation>,
    resource: Option<Resource>,
}
impl Identity {
    fn signature(&self) -> String {
        let mut text = format!("role {}\n", self.role.spelling());
        for (pc, target) in &self.adapter_calls {
            let _ = writeln!(text, "adapter-call {pc} {target}");
        }
        for (pc, operation) in &self.host_operations {
            let _ = writeln!(text, "host-operation {pc} {}", operation.spelling());
        }
        if let Some(resource) = &self.resource {
            let _ = writeln!(text, "resource {}", resource.digest_hex());
        }
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    identities: BTreeMap<i32, Identity>,
}
fn invalid(message: impl Into<String>) -> NativeError {
    NativeError::Invalid(message.into())
}
fn number<T: std::str::FromStr>(text: &str) -> Result<T> {
    text.parse()
        .map_err(|_| invalid("invalid execution identity or instruction"))
}
fn decode_hex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(HEX_BYTE_WIDTH)
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("invalid execution resource bytes"));
    }
    text.as_bytes()
        .chunks_exact(HEX_BYTE_WIDTH)
        .map(|pair| {
            u8::from_str_radix(
                std::str::from_utf8(pair).map_err(|_| invalid("invalid resource encoding"))?,
                HEX_RADIX,
            )
            .map_err(|_| invalid("invalid resource encoding"))
        })
        .collect()
}

impl Table {
    pub fn parse(text: &str) -> Result<Self> {
        let mut rows = text.lines().filter(|row| !row.trim().is_empty());
        if rows.next() != Some(FORMAT) {
            return Err(invalid("unknown execution format"));
        }
        let mut table = Self::default();
        let mut calls = Vec::new();
        let mut host_operations = Vec::new();
        let mut resources = Vec::new();
        for row in rows {
            let columns: Vec<_> = row.split_whitespace().collect();
            match columns.as_slice() {
                ["script", id, sha256, role] => {
                    let role = match *role {
                        "source" => Role::Source,
                        "adapter" => Role::Adapter,
                        "entry" => Role::Entry,
                        _ => return Err(invalid("unknown execution role")),
                    };
                    let id: i32 = number(id)?;
                    if id < 0
                        || sha256.len() != SHA256_HEX_WIDTH
                        || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                        || table
                            .identities
                            .insert(
                                id,
                                Identity {
                                    sha256: sha256.to_ascii_lowercase(),
                                    role,
                                    adapter_calls: BTreeMap::new(),
                                    host_operations: BTreeMap::new(),
                                    resource: None,
                                },
                            )
                            .is_some()
                    {
                        return Err(invalid("invalid or duplicate execution identity"));
                    }
                }
                ["adapter-call", id, pc, target] => calls.push((
                    number::<i32>(id)?,
                    number::<usize>(pc)?,
                    number::<i32>(target)?,
                )),
                ["resource", id, bytes] => resources.push((number::<i32>(id)?, decode_hex(bytes)?)),
                ["host-operation", id, pc, operation] => host_operations.push((
                    number::<i32>(id)?,
                    number::<usize>(pc)?,
                    HostOperation::parse(operation)?,
                )),
                _ => return Err(invalid("invalid execution row")),
            }
        }
        for (id, pc, target) in calls {
            let identity = table
                .identities
                .get_mut(&id)
                .ok_or_else(|| invalid("execution caller missing"))?;
            if identity.role != Role::Source || identity.adapter_calls.insert(pc, target).is_some()
            {
                return Err(invalid("invalid or duplicate adapter call"));
            }
        }
        for (id, pc, operation) in host_operations {
            table.insert_host_operation(id, pc, operation)?;
        }
        for (id, bytes) in resources {
            table.insert_resource(id, bytes.into())?;
        }
        Ok(table)
    }
    pub fn insert(
        &mut self,
        id: i32,
        bytes: &[u8],
        role: Role,
        adapter_calls: BTreeMap<usize, i32>,
    ) -> Result<()> {
        if id < 0
            || (role != Role::Source && !adapter_calls.is_empty())
            || self.identities.contains_key(&id)
        {
            return Err(invalid("invalid or duplicate execution identity"));
        }
        self.identities.insert(
            id,
            Identity {
                sha256: format!("{:x}", Sha256::digest(bytes)),
                role,
                adapter_calls,
                host_operations: BTreeMap::new(),
                resource: None,
            },
        );
        Ok(())
    }
    /// Bind both emitted bytes and required execution behavior before publication.
    pub fn bind_import(
        &mut self,
        id: i32,
        script: &mut CompiledScript,
        book: &OpcodeBook,
        specification: Specification,
    ) -> Result<Vec<u8>> {
        if self.identities.contains_key(&id) {
            return Err(invalid("duplicate execution identity"));
        }
        let mut candidate = Self::default();
        candidate.insert(id, &[], specification.role, specification.adapter_calls)?;
        if let Some(resource) = specification.resource {
            candidate.insert_resource(id, resource)?;
        }
        for (pc, operation) in specification.host_operations {
            candidate.insert_host_operation(id, pc, operation)?;
        }
        let mut bound = script.clone();
        let original_name = script.name.as_deref().unwrap_or_default();
        let name = original_name.strip_prefix("proc,").unwrap_or(original_name);
        bound.name = Some(format!(
            "{IMPORT_NAME_PREFIX}{}{IMPORT_NAME_SEPARATOR}{name}",
            candidate.identities[&id].signature()
        ));
        let bytes = encode_script(&bound, book)?;
        candidate
            .identities
            .get_mut(&id)
            .ok_or_else(|| invalid("candidate identity missing"))?
            .sha256 = format!("{:x}", Sha256::digest(&bytes));
        candidate.validate(id, &bytes, &bound)?;
        self.extend(candidate)?;
        *script = bound;
        Ok(bytes)
    }
    fn insert_resource(&mut self, id: i32, bytes: Arc<[u8]>) -> Result<()> {
        let identity = self
            .identities
            .get_mut(&id)
            .ok_or_else(|| invalid("resource script missing"))?;
        if identity.role != Role::Adapter || identity.resource.is_some() || bytes.is_empty() {
            return Err(invalid("invalid or duplicate execution resource"));
        }
        identity.resource = Some(Resource::new(bytes));
        Ok(())
    }
    pub fn analysis_options(&self) -> crate::dataflow::AnalysisOptions {
        let mut options = crate::dataflow::AnalysisOptions::default();
        for identity in self.identities.values() {
            let effects = identity
                .host_operations
                .iter()
                .filter(|(_, operation)| {
                    operation.needs_resource()
                        || matches!(
                            operation,
                            HostOperation::VariableBit { .. }
                                | HostOperation::RetainedPlayerTransmit { .. }
                        )
                })
                .map(|(pc, operation)| {
                    let effect = operation.effect(&Operand::Byte(0));
                    let nonnull_strings = match operation {
                        HostOperation::Enum {
                            output_type,
                            string_type,
                        } => {
                            if *output_type == i32::from(*string_type) {
                                vec![true]
                            } else {
                                Vec::new()
                            }
                        }
                        HostOperation::DatabaseField { pushes, .. } => {
                            vec![true; usize::from(pushes[1])]
                        }
                        _ => Vec::new(),
                    };
                    (
                        *pc,
                        crate::dataflow::TypedTraffic {
                            effect,
                            nonnull_strings,
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            if !effects.is_empty() {
                options.bound_traffic.insert(identity.signature(), effects);
            }
        }
        options
    }
    pub fn insert_host_operation(
        &mut self,
        id: i32,
        pc: usize,
        operation: HostOperation,
    ) -> Result<()> {
        let identity = self
            .identities
            .get_mut(&id)
            .ok_or_else(|| invalid("host operation script missing"))?;
        if !(identity.role == Role::Adapter
            || identity.role == Role::Entry && operation == HostOperation::FindComponent)
            || identity.host_operations.contains_key(&pc)
        {
            return Err(invalid("invalid or duplicate host operation"));
        }
        identity.host_operations.insert(pc, operation);
        Ok(())
    }
    pub fn extend(&mut self, other: Self) -> Result<()> {
        for (id, identity) in other.identities {
            if self.identities.insert(id, identity).is_some() {
                return Err(invalid("duplicate execution identity"));
            }
        }
        Ok(())
    }
    /// Remove metadata for a script whose source is being replaced in a dump.
    pub fn remove(&mut self, id: i32) {
        self.identities.remove(&id);
    }
    pub fn ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.identities.keys().copied()
    }
    pub fn is_empty(&self) -> bool {
        self.identities.is_empty()
    }
    pub fn emit(&self) -> String {
        let mut text = format!("{FORMAT}\n");
        for (id, identity) in &self.identities {
            let _ = writeln!(
                text,
                "script {id} {} {}",
                identity.sha256,
                identity.role.spelling()
            );
            if let Some(resource) = &identity.resource {
                let _ = write!(text, "resource {id} ");
                for byte in resource.bytes.iter() {
                    let _ = write!(text, "{byte:02x}");
                }
                text.push('\n');
            }
            for (pc, target) in &identity.adapter_calls {
                let _ = writeln!(text, "adapter-call {id} {pc} {target}");
            }
            for (pc, operation) in &identity.host_operations {
                let _ = writeln!(text, "host-operation {id} {pc} {}", operation.spelling());
            }
        }
        text
    }
    pub fn accounting(&self, id: i32) -> Accounting {
        let Some(identity) = self.identities.get(&id) else {
            return Accounting::default();
        };
        Accounting {
            identity: Some(identity.signature()),
            role: identity.role,
            transparent_calls: identity.adapter_calls.keys().copied().collect(),
            host_operations: identity.host_operations.clone(),
            resource: identity.resource.clone(),
        }
    }
    pub fn validate(&self, id: i32, bytes: &[u8], script: &CompiledScript) -> Result<()> {
        self.accounting(id).validate_script(script)?;
        let Some(identity) = self.identities.get(&id) else {
            return Ok(());
        };
        if format!("{:x}", Sha256::digest(bytes)) != identity.sha256 {
            return Err(invalid(format!(
                "script {id}: execution identity changed; rebuild the import"
            )));
        }
        for (pc, target) in &identity.adapter_calls {
            if script.code.get(*pc).is_none_or(|instruction| {
                instruction.command != "gosub_with_params"
                    || instruction.operand != Operand::Script(*target)
            }) {
                return Err(invalid("execution adapter call is not linked"));
            }
        }
        for (pc, operation) in &identity.host_operations {
            let declared = |slot: u16| slot < script.args.int && slot < script.locals.int;
            let arguments_declared = match operation {
                HostOperation::VariableBit { shift, .. } => {
                    u32::from(*shift) < i32::BITS
                        && script.code.get(*pc).is_some_and(|instruction| {
                            matches!(instruction.operand, Operand::VarRef(_))
                        })
                }
                HostOperation::RetainedPlayerTransmit { pops, .. } => {
                    pops[0] > u16::default() && pops[1] > u16::default()
                }
                HostOperation::ComponentText { property, .. } => !matches!(
                    property,
                    TextProperty::Font {
                        mapping: FontBinding::Unbound,
                        ..
                    }
                ),
                HostOperation::ComponentPaint { .. }
                | HostOperation::FindComponent
                | HostOperation::ClearRuntimeChildren
                | HostOperation::Enum { .. }
                | HostOperation::DatabaseField { .. }
                | HostOperation::DatabaseFieldCount { .. } => true,
                HostOperation::CreateFlatTextChild {
                    limit,
                    kind_argument,
                    slot_argument,
                    source,
                } => {
                    *limit != u16::default()
                        && declared(*kind_argument)
                        && declared(*slot_argument)
                        && source.is_some()
                }
                HostOperation::FindFlatChild {
                    limit,
                    slot_argument,
                } => *limit != u16::default() && declared(*slot_argument),
                HostOperation::NextRuntimeChildSlot { bank_argument, .. } => {
                    declared(*bank_argument)
                }
                HostOperation::FindRuntimeChild {
                    bank_argument,
                    slot_argument,
                    ..
                } => declared(*bank_argument) && declared(*slot_argument),
            };
            if operation.needs_resource() && identity.resource.is_none() {
                return Err(invalid("host operation resource missing"));
            }
            if !arguments_declared {
                return Err(invalid("host operation argument is not declared"));
            }
            if script
                .code
                .get(*pc)
                .is_none_or(|instruction| instruction.command != operation.command())
            {
                return Err(invalid(
                    "host operation consumer does not match its command",
                ));
            }
        }
        if identity.role == Role::Source {
            return Ok(());
        }
        if script
            .code
            .last()
            .is_none_or(|instruction| instruction.command != "return")
        {
            return Err(invalid("generated execution must end in return"));
        }
        let component_setup = identity.role == Role::Entry && !identity.host_operations.is_empty();
        if component_setup
            && !(identity.host_operations.len() == ONE_CALL
                && identity.host_operations.get(&ENTRY_COMPONENT_SELECT)
                    == Some(&HostOperation::FindComponent)
                && script
                    .code
                    .get(ENTRY_COMPONENT_CONSTANT)
                    .is_some_and(|instruction| {
                        instruction.command == "push_constant_string"
                            && matches!(instruction.operand, Operand::Int(_))
                    })
                && script
                    .code
                    .get(ENTRY_COMPONENT_SELECT)
                    .is_some_and(|instruction| {
                        instruction.command == "if_find"
                            && instruction.operand == Operand::Byte(u8::default())
                    })
                && script
                    .code
                    .get(ENTRY_COMPONENT_DISCARD)
                    .is_some_and(|instruction| {
                        instruction.command == "pop_int_discard"
                            && instruction.operand == Operand::Byte(u8::default())
                    }))
        {
            return Err(invalid(
                "entry component setup must select one frame before arguments",
            ));
        }
        let mut calls = usize::default();
        for (pc, instruction) in script.code.iter().enumerate() {
            let command = instruction.command.as_str();
            if component_setup && matches!(pc, ENTRY_COMPONENT_SELECT | ENTRY_COMPONENT_DISCARD) {
                continue;
            }
            if command == "return" && pc + ONE_CALL == script.code.len() {
                continue;
            }
            if matches!(
                command,
                "push_constant_string" | "push_int_local" | "push_string_local" | "push_long_local"
            ) {
                if calls != usize::default() {
                    return Err(invalid("generated setup must precede its consumer"));
                }
                continue;
            }
            match identity.role {
                Role::Entry if command == "gosub_with_params" => {
                    let Operand::Script(target) = instruction.operand else {
                        return Err(invalid("entry target is not a procedure"));
                    };
                    if target < 0 {
                        return Err(invalid("entry target is negative"));
                    }
                    calls += ONE_CALL;
                }
                Role::Adapter
                    if identity.host_operations.contains_key(&pc)
                        || crate::semantics::execution_family(command)
                            == crate::semantics::ExecutionFamily::HostRequired =>
                {
                    calls += ONE_CALL;
                }
                _ => return Err(invalid("generated execution must be linear")),
            }
        }
        if calls != ONE_CALL {
            return Err(invalid("generated execution must have one consumer"));
        }
        Ok(())
    }
    pub fn group_bytes(&self, id: i32) -> Option<Vec<u8>> {
        self.identities.get(&id).map(|identity| {
            Self {
                identities: BTreeMap::from([(id, identity.clone())]),
            }
            .emit()
            .into_bytes()
        })
    }
    pub fn from_group(id: i32, bytes: &[u8]) -> Result<Self> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| invalid("execution metadata is not UTF-8"))?;
        let table = Self::parse(text)?;
        if table.ids().collect::<Vec<_>>() != [id] {
            return Err(invalid("execution metadata belongs to another script"));
        }
        Ok(table)
    }
    pub fn validate_links(&self, scripts: &BTreeMap<i32, CompiledScript>) -> Result<()> {
        for (id, script) in scripts {
            self.accounting(*id).validate_script(script)?;
        }
        for (id, identity) in &self.identities {
            let script = scripts
                .get(id)
                .ok_or_else(|| invalid("execution script missing"))?;
            for target in identity.adapter_calls.values() {
                if self
                    .identities
                    .get(target)
                    .is_none_or(|callee| callee.role != Role::Adapter)
                {
                    return Err(invalid("execution adapter target missing"));
                }
            }
            for operation in identity.host_operations.values() {
                if let Some((source, pc)) = operation.creation_source()
                    && (self
                        .identities
                        .get(&source)
                        .is_none_or(|identity| identity.role != Role::Source)
                        || scripts
                            .get(&source)
                            .and_then(|script| script.code.get(pc))
                            .is_none_or(|instruction| {
                                instruction.command != "gosub_with_params"
                                    || instruction.operand != Operand::Script(*id)
                            }))
                {
                    return Err(invalid(
                        "creation source instruction is not linked to its adapter",
                    ));
                }
            }
            if identity.role == Role::Entry {
                for instruction in &script.code {
                    if let Operand::Script(target) = instruction.operand
                        && self
                            .identities
                            .get(&target)
                            .is_none_or(|callee| callee.role != Role::Source)
                    {
                        return Err(invalid("execution entry target missing"));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn resolved_accounting(&self, id: i32, script: &CompiledScript) -> Accounting {
        let mut accounting = self.accounting(id);
        if self
            .identities
            .get(&id)
            .is_some_and(|identity| identity.role == Role::Entry)
        {
            accounting
                .transparent_calls
                .extend(
                    script
                        .code
                        .iter()
                        .enumerate()
                        .filter_map(|(pc, instruction)| {
                            (instruction.command == "gosub_with_params").then_some(pc)
                        }),
                );
        }
        accounting
    }
}

/// Decode optional per-group metadata without changing ordinary cache scripts.
pub fn decode_accounting(
    id: i32,
    bytes: &[u8],
    metadata: Option<&[u8]>,
    script: &CompiledScript,
) -> Result<Accounting> {
    let Some(metadata) = metadata else {
        let accounting = Accounting::default();
        accounting.validate_script(script)?;
        return Ok(accounting);
    };
    let table = Table::from_group(id, metadata)?;
    table.validate(id, bytes, script)?;
    Ok(table.resolved_accounting(id, script))
}

/// Script groups contain bytecode at file zero and optional execution metadata.
pub fn decode_group(
    id: i32,
    files: &BTreeMap<u32, Vec<u8>>,
    book: &OpcodeBook,
) -> Result<(CompiledScript, Table)> {
    if files
        .keys()
        .any(|file| !matches!(*file, SCRIPT_FILE | METADATA_FILE))
    {
        return Err(invalid("unknown script group file"));
    }
    let bytes = files
        .get(&SCRIPT_FILE)
        .ok_or_else(|| invalid("script group lacks file zero"))?;
    let script = decode_script(bytes, book)?;
    let table = files
        .get(&METADATA_FILE)
        .map(|bytes| Table::from_group(id, bytes))
        .transpose()?
        .unwrap_or_default();
    table.validate(id, bytes, &script)?;
    Ok((script, table))
}
