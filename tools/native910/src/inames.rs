//! Curated interface and component names: the Ghidra-naming workflow for UI.
//!
//! 910 interfaces carry no name tags — a component's identity IS its pack
//! address — so human names can only be curated. `inames.txt` (tracked,
//! hand-edited) holds those, in two tiers: interface (pack group) names,
//! which are global idents, and child (pack file) names, which live scoped
//! under their interface (`bank/main` and `shop/main` coexist; the address
//! display always pairs them, so nothing is ambiguous).
//!
//! File shape (line-oriented, `#` comments, blank lines ignored):
//!
//! ```text
//! interface 1253 bank
//! component 1253 4 main
//! ```
//!
//! Names render in dump identity comments (`// interface bank component
//! main`), the `inspect` tree, and summaries; the numeric address stays the
//! authority everywhere (filenames, validation, bytes). An empty registry
//! renders everything numeric — graceful degradation, mirroring script
//! `SymbolRegistry` behavior.
//!
//! Validation is exact or loud: names must be valid idents, interface names
//! globally unique, child names unique within their interface, and every id
//! must exist in the pack roster (unknown interface, unknown child, and
//! duplicate entries all fail with line numbers at parse or ids at build).
//!
//! Script source spells packed component ids symbolically through this
//! registry: `Bank/7` (interface name + `/` + child name-or-number) wherever
//! an int literal fits, lowering to the identical packed
//! `Operand::Int(iface << 16 | child)`. The syntax is NAME-led only —
//! `1253/4` stays division, never a component — and numeric children validate
//! against the roster carried here, so the registry built over real pack data
//! is what makes symbolic refs resolvable at all. An empty registry renders
//! everything numeric and rejects every symbolic ref loudly.

use crate::error::{NativeError, Result};
use crate::pack::PackArchive;
use std::collections::BTreeMap;
use std::path::Path;

/// Curated interface entries: `(interface id, name)`.
pub type IfaceCurated = Vec<(i32, String)>;
/// Curated child entries: `(interface id, child index, name)`.
pub type ChildCurated = Vec<(i32, u32, String)>;
/// Pack roster: interface id → sorted child (file) indexes present.
pub type InterfaceRoster = BTreeMap<i32, Vec<u32>>;

/// Parse a curated `inames.txt` into interface and child entries.
/// Line numbers on every error; anything but `interface <id> <name>` or
/// `component <iface> <child> <name>` is rejected, as are bad ids/names.
pub fn parse_inames_txt(text: &str) -> Result<(IfaceCurated, ChildCurated)> {
    let mut ifaces = Vec::new();
    let mut children = Vec::new();
    for (position, raw) in text.lines().enumerate() {
        let line_no = position + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let err = |what: &str| NativeError::Invalid(format!("inames.txt line {line_no}: {what}"));
        match parts.as_slice() {
            ["interface", id_text, name] => {
                let id: i32 = id_text.parse().map_err(|_| err("bad interface id"))?;
                if !crate::source::is_valid_name(name) {
                    return Err(err("bad interface name"));
                }
                ifaces.push((id, (*name).to_string()));
            }
            ["component", iface_text, child_text, name] => {
                let iface: i32 = iface_text.parse().map_err(|_| err("bad interface id"))?;
                let child: u32 = child_text.parse().map_err(|_| err("bad child index"))?;
                if !crate::source::is_valid_name(name) {
                    return Err(err("bad child name"));
                }
                children.push((iface, child, (*name).to_string()));
            }
            _ => {
                return Err(err(
                    "expected 'interface <id> <name>' or 'component <iface> <child> <name>'",
                ));
            }
        }
    }
    Ok((ifaces, children))
}

/// Load the pack roster: every interface id with its sorted child indexes.
pub fn load_interface_roster(pack_root: &Path) -> Result<InterfaceRoster> {
    let archive = PackArchive::open(&pack_root.join("client.interfaces.js5"))?;
    let mut roster = BTreeMap::new();
    for group in archive.group_ids() {
        let Some(files) = archive.group_files(group)? else {
            continue;
        };
        let mut children: Vec<u32> = files.keys().copied().collect();
        children.sort_unstable();
        roster.insert(i32::try_from(group).unwrap_or(i32::MAX), children);
    }
    Ok(roster)
}

/// Curated names over the pack roster. Empty registries render numeric.
/// The roster is carried along (cloned at [`build`](Self::build)) so script
/// source parsing can validate symbolic component refs (`Bank/7`) and numeric
/// children against real pack membership — never guessed.
#[derive(Clone, Debug, Default)]
pub struct InterfaceRegistry {
    iface_name: BTreeMap<i32, String>,
    iface_id: BTreeMap<String, i32>,
    child_name: BTreeMap<(i32, u32), String>,
    child_id: BTreeMap<(i32, String), u32>,
    roster: InterfaceRoster,
}

impl InterfaceRegistry {
    /// Empty registry: every address renders numeric.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether any names are curated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.iface_name.is_empty() && self.child_name.is_empty()
    }

    /// Build over the roster. Interface names must be valid unique idents
    /// pointing at known interfaces; child names must be valid idents unique
    /// within their interface and pointing at known children. Anything else
    /// fails loudly — a typo'd id is never silently ignored.
    pub fn build(
        ifaces: &IfaceCurated,
        children: &ChildCurated,
        roster: &InterfaceRoster,
    ) -> Result<Self> {
        let mut registry = Self {
            roster: roster.clone(),
            ..Self::default()
        };
        for (id, name) in ifaces {
            if !roster.contains_key(id) {
                return Err(NativeError::Invalid(format!(
                    "named interface id {id} ('{name}') is not a known interface"
                )));
            }
            if registry.iface_id.contains_key(name.as_str()) {
                return Err(NativeError::Invalid(format!(
                    "interface name '{name}' already names id {}",
                    registry.iface_id[name.as_str()]
                )));
            }
            if registry.iface_name.contains_key(id) {
                return Err(NativeError::Invalid(format!(
                    "duplicate name entry for interface id {id}"
                )));
            }
            registry.iface_name.insert(*id, name.clone());
            registry.iface_id.insert(name.clone(), *id);
        }
        for (iface, child, name) in children {
            let known = roster.get(iface).ok_or_else(|| {
                NativeError::Invalid(format!(
                    "named component {iface}/{child} ('{name}'): unknown interface {iface}"
                ))
            })?;
            if !known.contains(child) {
                return Err(NativeError::Invalid(format!(
                    "named component {iface}/{child} ('{name}'): no such child"
                )));
            }
            if registry.child_id.contains_key(&(*iface, name.clone())) {
                return Err(NativeError::Invalid(format!(
                    "component name '{name}' already names a child of interface {iface}"
                )));
            }
            if registry.child_name.contains_key(&(*iface, *child)) {
                return Err(NativeError::Invalid(format!(
                    "duplicate name entry for component {iface}/{child}"
                )));
            }
            registry.child_name.insert((*iface, *child), name.clone());
            registry.child_id.insert((*iface, name.clone()), *child);
        }
        Ok(registry)
    }

    /// Curated interface name, if anyone has named it.
    #[must_use]
    pub fn interface_name(&self, iface: i32) -> Option<&str> {
        self.iface_name.get(&iface).map(String::as_str)
    }

    /// Curated child name, if anyone has named it.
    #[must_use]
    pub fn child_name(&self, iface: i32, child: u32) -> Option<&str> {
        self.child_name.get(&(iface, child)).map(String::as_str)
    }

    /// Display for an interface id: the name, else the number.
    #[must_use]
    pub fn display_iface(&self, iface: i32) -> String {
        match self.interface_name(iface) {
            Some(name) => name.to_string(),
            None => iface.to_string(),
        }
    }

    /// Display for a child index under an interface: the name, else the number.
    #[must_use]
    pub fn display_child(&self, iface: i32, child: u32) -> String {
        match self.child_name(iface, child) {
            Some(name) => name.to_string(),
            None => child.to_string(),
        }
    }

    /// Display for an address: `bank/main`, `bank/7`, or `1253/4` — names
    /// where curated, numbers elsewhere, always paired so scoped child names
    /// stay unambiguous.
    #[must_use]
    pub fn display(&self, iface: i32, child: u32) -> String {
        format!(
            "{}/{}",
            self.display_iface(iface),
            self.display_child(iface, child)
        )
    }

    /// Resolve an interface display back to its id: curated name first, else
    /// a plain number. `None` resolves nothing (never guessed).
    #[must_use]
    pub fn resolve_iface(&self, text: &str) -> Option<i32> {
        self.iface_id
            .get(text)
            .copied()
            .or_else(|| text.parse().ok())
    }

    /// Resolve a child display under an interface: curated name first, else
    /// a plain number.
    #[must_use]
    pub fn resolve_child(&self, iface: i32, text: &str) -> Option<u32> {
        self.child_id
            .get(&(iface, text.to_string()))
            .copied()
            .or_else(|| text.parse().ok())
    }

    /// The pack roster this registry validates against.
    #[must_use]
    pub fn roster(&self) -> &InterfaceRoster {
        &self.roster
    }

    /// Whether `(iface, child)` names a real packed pair in the roster.
    #[must_use]
    pub fn contains(&self, iface: i32, child: u32) -> bool {
        self.roster
            .get(&iface)
            .is_some_and(|children| children.contains(&child))
    }

    /// Resolve an `Iface/child` script-source spelling to its addressed pair.
    /// The interface resolves by curated NAME only — never numerically, so
    /// `1253/4` can never arrive here as a component (it stays division or a
    /// bad int). The child resolves by curated name, else as a plain number
    /// validated against roster membership. Anything unresolvable yields
    /// `None` (the caller fails loudly, distinguishing unknown interfaces
    /// from unknown children); an empty registry resolves nothing.
    #[must_use]
    pub fn resolve_component(&self, iface_name: &str, child_text: &str) -> Option<(i32, u32)> {
        let iface = self.iface_id.get(iface_name).copied()?;
        let child = self
            .child_id
            .get(&(iface, child_text.to_string()))
            .copied()
            .or_else(|| {
                child_text
                    .parse::<u32>()
                    .ok()
                    .filter(|child| self.contains(iface, *child))
            })?;
        Some((iface, child))
    }

    /// Unpack a packed int (`iface << 16 | child`) the roster actually holds.
    /// Negative values are not references and unknown pairs are not guessed —
    /// both yield `None`. Dump rendering uses this to find symbolic spellings;
    /// empty registries (no roster) find nothing.
    #[must_use]
    pub fn unpack_known(&self, value: i32) -> Option<(i32, u32)> {
        if value < 0 {
            return None;
        }
        let child = (value & 0xFFFF) as u32;
        let iface = value >> 16;
        self.contains(iface, child).then_some((iface, child))
    }

    /// Display for a resolved pair in SCRIPT source: `Bank/7` or `Bank/main`
    /// when the interface is named (child by curated name else number —
    /// [`display`]), else the packed decimal (never numeric/numeric, which
    /// would reparse as division). Total: out-of-range programmatic pairs
    /// render a wide decimal the int parser rejects loudly on reparse, never
    /// a silently reshaped address.
    #[must_use]
    pub fn display_component_ref(&self, iface: i32, child: u32) -> String {
        if self.interface_name(iface).is_some() {
            return self.display(iface, child);
        }
        ((i64::from(iface) << 16) | i64::from(child)).to_string()
    }
}

/// Format a dump identity comment: `// interface <iface> component <child>`
/// with names where curated (`// interface bank component main`). The parser
/// ignores `//` lines, so this is display metadata — but [`check_identity`]
/// validates it back on assemble, so a stale or moved name fails loudly
/// instead of lying.
#[must_use]
pub fn format_identity(iface: i32, child: u32, names: &InterfaceRegistry) -> String {
    format!(
        "// interface {} component {}\n",
        names.display_iface(iface),
        names.display_child(iface, child)
    )
}

/// Validate a source text's identity comment against the assembling group:
/// when the first line is `// interface <X> component <Y>`, `X` must resolve
/// (name or number) to `group` and `Y` must resolve under that interface —
/// anything else is a moved or stale file failing loudly. Texts without the
/// comment (hand-written, or the bare `// interface component` marker) pass
/// untouched: the rule only guards what claims an address.
pub fn check_identity(text: &str, group: i32, names: &InterfaceRegistry) -> Result<()> {
    let Some(first) = text.lines().next() else {
        return Ok(());
    };
    let parts: Vec<&str> = first.split_whitespace().collect();
    if parts.len() != 5 || parts[0] != "//" || parts[1] != "interface" || parts[3] != "component" {
        return Ok(());
    }
    match names.resolve_iface(parts[2]) {
        Some(id) if id == group => {}
        Some(id) => {
            return Err(NativeError::Invalid(format!(
                "identity comment names interface {id} but assembling group {group}"
            )));
        }
        None => {
            return Err(NativeError::Invalid(format!(
                "identity comment names unknown interface '{}'",
                parts[2]
            )));
        }
    }
    if names.resolve_child(group, parts[4]).is_none() {
        return Err(NativeError::Invalid(format!(
            "identity comment names unknown component '{}'",
            parts[4]
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> InterfaceRoster {
        BTreeMap::from([(1253, vec![0, 1, 2, 3, 4]), (7, vec![0, 1])])
    }

    fn registry() -> InterfaceRegistry {
        let (ifaces, children) = parse_inames_txt(
            "# comment\n\ninterface 1253 bank\ncomponent 1253 4 main\ncomponent 7 0 other_main\n",
        )
        .unwrap();
        InterfaceRegistry::build(&ifaces, &children, &roster()).unwrap()
    }

    #[test]
    fn curated_names_display_and_resolve() {
        let names = registry();
        assert_eq!(names.display(1253, 4), "bank/main");
        assert_eq!(names.display(1253, 3), "bank/3");
        assert_eq!(names.display(7, 0), "7/other_main");
        assert_eq!(names.display(9, 0), "9/0");
        assert_eq!(names.resolve_iface("bank"), Some(1253));
        assert_eq!(names.resolve_iface("1253"), Some(1253));
        assert_eq!(names.resolve_iface("ghost"), None);
        assert_eq!(names.resolve_child(1253, "main"), Some(4));
        assert_eq!(names.resolve_child(1253, "4"), Some(4));
        // Scoped: `main` under 7 is a different child.
        assert_eq!(names.resolve_child(7, "other_main"), Some(0));
        assert_eq!(names.resolve_child(7, "main"), None);
        assert!(!names.is_empty());
        assert!(InterfaceRegistry::empty().is_empty());
    }

    #[test]
    fn build_validates_ids_and_uniqueness_loudly() {
        let roster = roster();
        let build = |text: &str| {
            parse_inames_txt(text).and_then(|(ifaces, children)| {
                InterfaceRegistry::build(&ifaces, &children, &roster)
            })
        };
        // Unknown interface, unknown child, bad name, bad shape.
        assert!(build("interface 9999 ghost\n").is_err());
        assert!(build("component 1253 9 ghost\n").is_err());
        assert!(build("component 9999 0 ghost\n").is_err());
        assert!(build("interface 1253 has space\n").is_err());
        assert!(build("interface x bad\n").is_err());
        assert!(build("proc 1 x\n").is_err());
        // Duplicate interface name across ids; duplicate entry for one id.
        assert!(build("interface 1253 bank\ninterface 7 bank\n").is_err());
        assert!(build("interface 1253 bank\ninterface 1253 shop\n").is_err());
        // Duplicate child name within one interface fails; across interfaces is fine.
        assert!(build("component 1253 3 main\ncomponent 1253 4 main\n").is_err());
        assert!(build("component 1253 3 main\ncomponent 1253 3 other\n").is_err());
        assert!(build("component 1253 3 main\ncomponent 7 1 main\n").is_ok());
    }

    #[test]
    fn identity_comments_format_and_check() {
        let names = registry();
        assert_eq!(
            format_identity(1253, 4, &names),
            "// interface bank component main\n"
        );
        assert_eq!(
            format_identity(1253, 3, &names),
            "// interface bank component 3\n"
        );
        assert_eq!(
            format_identity(9, 0, &names),
            "// interface 9 component 0\n"
        );
        // Matching group passes (numeric or named); mismatched or unknown fails.
        assert!(check_identity("// interface bank component main\n", 1253, &names).is_ok());
        assert!(check_identity("// interface 1253 component 4\n", 1253, &names).is_ok());
        assert!(check_identity("// interface bank component main\n", 7, &names).is_err());
        assert!(check_identity("// interface ghost component 0\n", 1253, &names).is_err());
        assert!(check_identity("// interface bank component ghost\n", 1253, &names).is_err());
        // Non-identity first lines (bare marker, code, empty) pass untouched.
        assert!(check_identity("// interface component\n", 1253, &names).is_ok());
        assert!(check_identity("component text;\n", 1253, &names).is_ok());
        assert!(check_identity("", 1253, &names).is_ok());
        // Empty registry still validates numeric addresses.
        let empty = InterfaceRegistry::empty();
        assert!(check_identity("// interface 1253 component 4\n", 1253, &empty).is_ok());
        assert!(check_identity("// interface 1253 component 4\n", 7, &empty).is_err());
    }

    #[test]
    fn component_refs_resolve_name_led_only() {
        let names = registry();
        // Named interface plus curated or roster-checked numeric child.
        assert_eq!(names.resolve_component("bank", "main"), Some((1253, 4)));
        assert_eq!(names.resolve_component("bank", "3"), Some((1253, 3)));
        // Numeric children outside the roster never resolve (never guessed).
        assert_eq!(names.resolve_component("bank", "9"), None);
        assert_eq!(names.resolve_component("bank", "ghost"), None);
        assert_eq!(names.resolve_component("ghost", "0"), None);
        // The interface side is names-only: numeric text never resolves here,
        // so `1253/4` can never parse as a component.
        assert_eq!(names.resolve_component("1253", "4"), None);
        assert_eq!(names.resolve_component("1253", "main"), None);
        // Empty registries resolve nothing; numeric text is unaffected
        // elsewhere (it never routes through here).
        assert_eq!(
            InterfaceRegistry::empty().resolve_component("bank", "main"),
            None
        );
    }

    #[test]
    fn known_pairs_unpack_and_display_for_source() {
        let names = registry();
        // 1253 << 16 | 4 == the packed wire int dumps carry.
        assert_eq!(names.unpack_known(82_116_612), Some((1253, 4)));
        assert_eq!(names.unpack_known(82_116_611), Some((1253, 3)));
        // Not a reference (negative), or not a roster pair: no guess.
        assert_eq!(names.unpack_known(-1), None);
        assert_eq!(names.unpack_known(41), None);
        assert_eq!(names.unpack_known(9 << 16), None);
        assert_eq!(InterfaceRegistry::empty().unpack_known(82_116_612), None);
        // Named interfaces render `Name/child`; unnamed ones the packed
        // decimal — never numeric/numeric (which would reparse as division).
        assert_eq!(names.display_component_ref(1253, 4), "bank/main");
        assert_eq!(names.display_component_ref(1253, 3), "bank/3");
        assert_eq!(names.display_component_ref(9, 0), "589824");
        assert_eq!(
            InterfaceRegistry::empty().display_component_ref(1253, 4),
            "82116612"
        );
    }
}
