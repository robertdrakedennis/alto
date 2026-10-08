//! Refactor-gate helper for the client910 code-quality programme (Phase 0).
//!
//! Two subcommands, both driven by Python wrappers in `tools/refactor/`:
//!
//! * `items <label=dir>...` (used by `fn-hash.py`): parse every `.rs` file
//!   under each directory with `syn` and print one TSV row per item (fn,
//!   impl header, struct, enum, const, static, type, trait, macro_rules,
//!   item-level macro call) with three hashes:
//!   - `code`: the item's tokens with comments, doc attributes and
//!     visibility removed, `crate::`/`super::`/`self::`/`$crate::`/
//!     `client910::`/`rs910_*::` path prefixes stripped, single-name paths
//!     expanded through the module's (and fn body's) `use` declarations,
//!     and `use` statements inside fn bodies dropped. A pure move keeps it.
//!   - `shape`: the `code` tokens with every distinct non-keyword identifier
//!     replaced by its first-occurrence index (alpha-normalised), so a pure
//!     rename keeps it. Keywords, primitive and prelude names, macro names,
//!     attribute contents, literals and all punctuation stay exactly. The
//!     distinct identifiers in first-occurrence order are emitted too
//!     (`idents`), so callers can check which names a rename touched.
//!   - `doc`: the doc attributes (`///`, `//!`, `#[doc]`).
//!   - `comments`: the non-doc `//` and `/* */` comments inside the item and
//!     the comment lines directly above it (`File.java:line` cites).
//! * `deps <crate-root.rs> [--alias name]... [--crate-alias name=lib.rs]...`
//!   (used by `dag-check.py`): walk the module tree from the crate root like
//!   rustc (`mod x;`, `#[path]`, `mod.rs`), and print every module, every
//!   reference from one top-level module to another (`crate::`, `super::`,
//!   `$crate::`, crate aliases, root-level names), and every use of an
//!   external crate, each tagged with whether it only exists under
//!   `#[cfg(test)]`. `--crate-alias` names a workspace crate whose top-level
//!   modules count as modules of this graph (Phase 2 crates reached through
//!   `rs910_core::m` paths or `pub use` facades).
//!
//! * `magic-ids <dir|file>...` (used by `magic-id-ratchet.py`): integer
//!   literals standing where a content id goes (`magic_ids.rs`).
//!
//! The scanner works on token streams, so paths inside macro invocations
//! (`format!`, `matches!`, local `macro_rules!` calls) count too.

mod magic_ids;

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// ---------------------------------------------------------------------------
// Token flattening
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Ident,
    Punct,
    Lit,
    Open,
    Close,
}

#[derive(Clone, Debug)]
struct Tok {
    s: String,
    k: K,
    line: usize,
    col: usize,
    end_line: usize,
    end_col: usize,
}

fn flatten(ts: TokenStream, out: &mut Vec<Tok>) {
    let mut pending: Option<Tok> = None;
    for tt in ts {
        if let TokenTree::Punct(p) = &tt {
            let sp = p.span();
            let (s, e) = (sp.start(), sp.end());
            let entry = pending.get_or_insert_with(|| Tok {
                s: String::new(),
                k: K::Punct,
                line: s.line,
                col: s.column,
                end_line: e.line,
                end_col: e.column,
            });
            entry.s.push(p.as_char());
            entry.end_line = e.line;
            entry.end_col = e.column;
            if p.spacing() == Spacing::Alone {
                out.push(pending.take().unwrap());
            }
            continue;
        }
        if let Some(p) = pending.take() {
            out.push(p);
        }
        match tt {
            TokenTree::Ident(i) => {
                let sp = i.span();
                out.push(Tok {
                    s: i.to_string(),
                    k: K::Ident,
                    line: sp.start().line,
                    col: sp.start().column,
                    end_line: sp.end().line,
                    end_col: sp.end().column,
                });
            }
            TokenTree::Literal(l) => {
                let sp = l.span();
                out.push(Tok {
                    s: l.to_string(),
                    k: K::Lit,
                    line: sp.start().line,
                    col: sp.start().column,
                    end_line: sp.end().line,
                    end_col: sp.end().column,
                });
            }
            TokenTree::Group(g) => {
                let (o, c) = match g.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::None => {
                        flatten(g.stream(), out);
                        continue;
                    }
                };
                let so = g.span_open();
                out.push(Tok {
                    s: o.into(),
                    k: K::Open,
                    line: so.start().line,
                    col: so.start().column,
                    end_line: so.end().line,
                    end_col: so.end().column,
                });
                flatten(g.stream(), out);
                let sc = g.span_close();
                out.push(Tok {
                    s: c.into(),
                    k: K::Close,
                    line: sc.start().line,
                    col: sc.start().column,
                    end_line: sc.end().line,
                    end_col: sc.end().column,
                });
            }
            TokenTree::Punct(_) => unreachable!(),
        }
    }
    if let Some(p) = pending.take() {
        out.push(p);
    }
}

fn toks_of<T: ToTokens>(t: &T) -> Vec<Tok> {
    let mut out = Vec::new();
    flatten(t.to_token_stream(), &mut out);
    out
}

/// A path head may start here: an identifier that is not a field/method
/// name (`.x`), not inside a path (`a::x`) and not a lifetime (`'x`).
fn path_head(toks: &[Tok], i: usize) -> bool {
    if toks[i].k != K::Ident {
        return false;
    }
    if i == 0 {
        return true;
    }
    let p = &toks[i - 1];
    !(p.k == K::Punct && (p.s == "." || p.s.ends_with("::") || p.s.ends_with('\'') || p.s == "$"))
}

/// Collect `a :: b :: c` starting at `i`; returns (segments, index after).
fn path_run(toks: &[Tok], i: usize) -> (Vec<String>, usize) {
    let mut segs = vec![toks[i].s.clone()];
    let mut j = i + 1;
    while j + 1 < toks.len()
        && toks[j].k == K::Punct
        && toks[j].s == "::"
        && toks[j + 1].k == K::Ident
    {
        segs.push(toks[j + 1].s.clone());
        j += 2;
    }
    (segs, j)
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

fn fnv(parts: impl IntoIterator<Item = impl AsRef<str>>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_ref().bytes().chain(std::iter::once(0x1f)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn is_our_prefix(s: &str) -> bool {
    matches!(s, "crate" | "super" | "self" | "client910") || s.starts_with("rs910_")
}

type UseMap = HashMap<String, Vec<String>>;

fn strip_head(mut segs: Vec<String>) -> (Vec<String>, bool) {
    let mut stripped = false;
    while segs.len() > 1 && is_our_prefix(&segs[0]) {
        segs.remove(0);
        stripped = true;
    }
    (segs, stripped)
}

/// Normalise a flattened token list for the `code` hash.
fn normalize(toks: &[Tok], uses: &UseMap) -> Vec<String> {
    let mut out = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        // Visibility: `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`.
        if t.k == K::Ident && t.s == "pub" {
            i += 1;
            if i < toks.len() && toks[i].k == K::Open && toks[i].s == "(" {
                if let Some(n) = toks.get(i + 1) {
                    if n.k == K::Ident && matches!(n.s.as_str(), "crate" | "super" | "self" | "in")
                    {
                        let mut depth = 0;
                        while i < toks.len() {
                            match toks[i].k {
                                K::Open => depth += 1,
                                K::Close => {
                                    depth -= 1;
                                    if depth == 0 {
                                        i += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                }
            }
            continue;
        }
        // `$crate::x` in macro_rules bodies.
        if t.k == K::Punct
            && t.s == "$"
            && toks
                .get(i + 1)
                .is_some_and(|n| n.k == K::Ident && n.s == "crate")
        {
            let (segs, j) = path_run(toks, i + 1);
            let segs = if segs.len() > 1 {
                segs[1..].to_vec()
            } else {
                segs
            };
            push_path(&mut out, &segs);
            i = j;
            continue;
        }
        if path_head(toks, i) {
            let (segs, j) = path_run(toks, i);
            let (segs, stripped) = strip_head(segs);
            let segs = if !stripped {
                match uses.get(&segs[0]) {
                    Some(full) if !(segs.len() == 1 && next_is_colon_field(toks, j)) => {
                        let mut v = full.clone();
                        v.extend_from_slice(&segs[1..]);
                        v
                    }
                    _ => segs,
                }
            } else {
                segs
            };
            push_path(&mut out, &segs);
            i = j;
            continue;
        }
        out.push(t.s.clone());
        i += 1;
    }
    out
}

/// `Foo { name: value }` field names are not paths.
fn next_is_colon_field(toks: &[Tok], j: usize) -> bool {
    toks.get(j).is_some_and(|n| n.k == K::Punct && n.s == ":")
}

fn push_path(out: &mut Vec<String>, segs: &[String]) {
    for (n, s) in segs.iter().enumerate() {
        if n > 0 {
            out.push("::".into());
        }
        out.push(s.clone());
    }
}

// ---------------------------------------------------------------------------
// Rename-aware shape hash
// ---------------------------------------------------------------------------

/// Identifiers that are never alpha-normalised: keywords and the names whose
/// meaning is fixed by the language or its prelude (renaming `Some` to `Ok`
/// or `u32` to `u64` is not a rename).
const KEPT_IDENTS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "become",
    "box",
    "break",
    "const",
    "continue",
    "crate",
    "do",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "macro",
    "macro_rules",
    "match",
    "mod",
    "move",
    "mut",
    "override",
    "priv",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "union",
    "unsafe",
    "unsized",
    "use",
    "virtual",
    "where",
    "while",
    "yield",
    "_",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "f32",
    "f64",
    "bool",
    "char",
    "str",
    "Some",
    "None",
    "Ok",
    "Err",
    "Option",
    "Result",
    "Vec",
    "Box",
    "String",
];

/// Is this normalised token a literal (number, string, char, byte, raw)?
fn is_literal_tok(s: &str) -> bool {
    let b = s.as_bytes();
    match b.first() {
        Some(b'0'..=b'9' | b'"') => true,
        Some(b'\'') => s.len() > 1,
        Some(b'b') => match b.get(1) {
            Some(b'"' | b'\'') => true,
            Some(b'r') => raw_tail(&s[2..]),
            _ => false,
        },
        Some(b'r') => raw_tail(&s[1..]),
        _ => false,
    }
}

/// `#*"` after an `r` / `br` prefix (not the raw identifier `r#type`).
fn raw_tail(t: &str) -> bool {
    t.trim_start_matches('#').starts_with('"')
}

fn is_ident_tok(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_alphabetic() || c == '_')
        && !is_literal_tok(s)
        && !KEPT_IDENTS.contains(&s)
}

/// Alpha-normalise a normalised token list: hash (with `kind`) and the
/// distinct renamable identifiers in first-occurrence order.
///
/// An identifier is marked global (`name@` in the emitted list) when it
/// names something outside the item: it follows `.` or `::` or an item
/// keyword, starts uppercase, is called or a path head, or the item is a
/// type-level item (struct, enum, impl, ...). Bare bindings (params, locals)
/// stay unmarked; renames of those legitimately differ per function, renames
/// of global names must be consistent across the whole run.
fn shape_of(kind: &str, norm: &[String]) -> (u64, Vec<String>) {
    let type_level = !matches!(kind, "fn" | "macro_rules" | "macro-call");
    let mut idents: Vec<String> = Vec::new();
    let mut global: Vec<bool> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut parts: Vec<String> = Vec::with_capacity(norm.len() + 1);
    parts.push(kind.to_string());
    let mut i = 0;
    while i < norm.len() {
        let t = norm[i].as_str();
        // Attribute contents (`#[derive(..)]`, `#![allow(..)]`) stay as written.
        if (t == "#" || t == "#!") && norm.get(i + 1).is_some_and(|n| n == "[") {
            let mut depth = 0;
            while i < norm.len() {
                match norm[i].as_str() {
                    "[" => depth += 1,
                    "]" => depth -= 1,
                    _ => {}
                }
                parts.push(norm[i].clone());
                i += 1;
                if depth == 0 && parts.last().is_some_and(|p| p == "]") {
                    break;
                }
            }
            continue;
        }
        // Macro names (`vec!`, `format!`) stay as written.
        let is_macro_name =
            norm.get(i + 1).is_some_and(|n| n == "!") && !norm.get(i + 2).is_some_and(|n| n == "=");
        if is_ident_tok(t) && !is_macro_name {
            let prev = i.checked_sub(1).map(|p| norm[p].as_str()).unwrap_or("");
            let next = norm.get(i + 1).map(String::as_str).unwrap_or("");
            let is_global = type_level
                || matches!(
                    prev,
                    "." | "::"
                        | "fn"
                        | "struct"
                        | "enum"
                        | "union"
                        | "trait"
                        | "type"
                        | "mod"
                        | "const"
                        | "static"
                )
                || t.starts_with(char::is_uppercase)
                || matches!(next, "(" | "::");
            let n = match index.get(t) {
                Some(n) => *n,
                None => {
                    let n = idents.len();
                    idents.push(t.to_string());
                    global.push(false);
                    index.insert(t, n);
                    n
                }
            };
            global[n] |= is_global;
            parts.push(format!("\u{1}{n}"));
        } else {
            parts.push(t.to_string());
        }
        i += 1;
    }
    for (id, g) in idents.iter_mut().zip(&global) {
        if *g {
            id.push('@');
        }
    }
    (fnv(parts.iter()), idents)
}

// ---------------------------------------------------------------------------
// `use` trees
// ---------------------------------------------------------------------------

/// Expand a use tree to (visible name, full path) pairs. Globs are skipped.
fn expand_use(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<(String, Vec<String>)>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            expand_use(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last() {
                    out.push((last.clone(), prefix.clone()));
                }
            } else {
                let mut full = prefix.clone();
                full.push(name.clone());
                out.push((name, full));
            }
        }
        syn::UseTree::Rename(r) => {
            let mut full = prefix.clone();
            if r.ident != "self" {
                full.push(r.ident.to_string());
            }
            out.push((r.rename.to_string(), full));
        }
        syn::UseTree::Glob(_) => {
            let mut full = prefix.clone();
            full.push("*".into());
            out.push(("*".into(), full));
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                expand_use(t, prefix, out);
            }
        }
    }
}

fn use_map_of(items: &[syn::Item]) -> UseMap {
    let mut m = UseMap::new();
    for it in items {
        if let syn::Item::Use(u) = it {
            let mut out = Vec::new();
            expand_use(&u.tree, &mut Vec::new(), &mut out);
            for (name, full) in out {
                if name == "*" || name == "_" {
                    continue;
                }
                let (full, _) = strip_head(full);
                m.insert(name, full);
            }
        }
    }
    m
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

struct Source {
    text: String,
    line_starts: Vec<usize>,
}

impl Source {
    fn new(text: String) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { text, line_starts }
    }

    /// proc-macro2 lines are 1-based, columns are 0-based chars.
    fn offset(&self, line: usize, col: usize) -> usize {
        let start = self
            .line_starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(self.text.len());
        let rest = &self.text[start..];
        match rest.char_indices().nth(col) {
            Some((i, _)) => start + i,
            None => self.text.len(),
        }
    }

    fn line_text(&self, line: usize) -> &str {
        let s = self.line_starts[line - 1];
        let e = self
            .line_starts
            .get(line)
            .copied()
            .unwrap_or(self.text.len());
        &self.text[s..e]
    }
}

/// Non-doc comments in `src`, skipping string/char literals.
fn comments(src: &str, out: &mut Vec<String>) {
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let e = src[i..].find('\n').map_or(b.len(), |n| i + n);
                let c = &src[i..e];
                let doc = (c.starts_with("///") && !c.starts_with("////")) || c.starts_with("//!");
                if !doc {
                    out.push(c.split_whitespace().collect::<Vec<_>>().join(" "));
                }
                i = e;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0;
                let s = i;
                while i < b.len() {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                let c = &src[s..i.min(b.len())];
                let doc = (c.starts_with("/**") && !c.starts_with("/***") && c != "/**/")
                    || c.starts_with("/*!");
                if !doc {
                    out.push(c.split_whitespace().collect::<Vec<_>>().join(" "));
                }
            }
            b'r' | b'b' if is_raw_str(b, i) => {
                // r"..", r#".."#, br#".."#
                let mut j = i + 1;
                if b[i] == b'b' {
                    j += 1;
                }
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                j += 1; // opening quote
                loop {
                    if j >= b.len() {
                        break;
                    }
                    if b[j] == b'"' && (0..hashes).all(|h| b.get(j + 1 + h) == Some(&b'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                i = j;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'\'' => {
                // char literal ('x', '\n', '\u{..}') vs lifetime ('a)
                if b.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    while i < b.len() && b[i] != b'\'' {
                        i += 1;
                    }
                    i += 1;
                } else {
                    let ch_len = src[i + 1..].chars().next().map_or(1, char::len_utf8);
                    if b.get(i + 1 + ch_len) == Some(&b'\'') {
                        i += 2 + ch_len;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
}

fn is_raw_str(b: &[u8], i: usize) -> bool {
    if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
        return false;
    }
    let mut j = i + 1;
    if b[i] == b'b' {
        if b.get(j) != Some(&b'r') {
            return false;
        }
        j += 1;
    }
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    b.get(j) == Some(&b'"')
}

// ---------------------------------------------------------------------------
// `items`
// ---------------------------------------------------------------------------

struct Row {
    kind: &'static str,
    key: String,
    code: u64,
    shape: u64,
    idents: Vec<String>,
    doc: u64,
    comments: u64,
    ntok: usize,
    file: String,
    line: usize,
    modpath: String,
}

struct ItemCx<'a> {
    src: &'a Source,
    file: &'a str,
    rows: &'a mut Vec<Row>,
}

fn is_doc(a: &syn::Attribute) -> bool {
    a.path().is_ident("doc")
}

fn split_attrs(attrs: &[syn::Attribute]) -> (Vec<Tok>, Vec<String>) {
    let mut code = Vec::new();
    let mut doc = Vec::new();
    for a in attrs {
        if is_doc(a) {
            doc.push(a.to_token_stream().to_string());
        } else {
            code.extend(toks_of(a));
        }
    }
    (code, doc)
}

/// Key text for a type: token text with every path reduced to its last
/// segment (`crate::a::Foo<T>` -> `Foo<T>`).
fn key_text<T: ToTokens>(t: &T) -> String {
    let toks = toks_of(t);
    let mut out = String::new();
    let mut i = 0;
    while i < toks.len() {
        if path_head(&toks, i) {
            let (segs, j) = path_run(&toks, i);
            out.push_str(segs.last().unwrap());
            i = j;
        } else {
            out.push_str(&toks[i].s);
            i += 1;
        }
        if i < toks.len()
            && toks[i].k == K::Ident
            && out
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            out.push(' ');
        }
    }
    out
}

impl ItemCx<'_> {
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        kind: &'static str,
        key: String,
        attrs: &[syn::Attribute],
        parts: Vec<Tok>,
        span_toks: &[Tok],
        uses: &UseMap,
        modpath: &str,
    ) {
        let (mut code_toks, doc) = split_attrs(attrs);
        code_toks.extend(parts);
        let norm = normalize(&code_toks, uses);
        let code = fnv(std::iter::once(kind.to_string()).chain(norm.iter().cloned()));
        let (shape, idents) = shape_of(kind, &norm);
        let docs = fnv(doc.iter());
        let (first, last) = match (span_toks.first(), span_toks.last()) {
            (Some(f), Some(l)) => (f, l),
            _ => return,
        };
        // Comments inside the item plus the comment lines directly above it.
        let mut start_line = first.line;
        while start_line > 1 {
            let t = self.src.line_text(start_line - 1).trim_start();
            if t.starts_with("//") && !t.starts_with("///") && !t.starts_with("//!")
                || t.starts_with("////")
            {
                start_line -= 1;
            } else {
                break;
            }
        }
        let s = if start_line < first.line {
            self.src.line_starts[start_line - 1]
        } else {
            self.src.offset(first.line, first.col)
        };
        let e = self.src.offset(last.end_line, last.end_col).max(s);
        let mut cs = Vec::new();
        comments(&self.src.text[s..e], &mut cs);
        let com = fnv(cs.iter());
        self.rows.push(Row {
            kind,
            key,
            code,
            shape,
            idents,
            doc: docs,
            comments: com,
            ntok: norm.len(),
            file: self.file.to_string(),
            line: first.line,
            modpath: modpath.to_string(),
        });
    }

    fn items(&mut self, items: &[syn::Item], modpath: &str) {
        let uses = use_map_of(items);
        let mut macro_seq: HashMap<String, usize> = HashMap::new();
        for it in items {
            let all = toks_of(it);
            match it {
                syn::Item::Fn(f) => {
                    let body_uses = body_use_map(&uses, &f.block);
                    let mut parts = toks_of(&f.sig);
                    parts.extend(block_toks(&f.block));
                    let key = format!("{}", f.sig.ident);
                    self.emit("fn", key, &f.attrs, parts, &all, &body_uses, modpath);
                }
                syn::Item::Impl(im) => {
                    let ctx = match &im.trait_ {
                        Some((bang, path, _)) => format!(
                            "{}{} for {}",
                            if bang.is_some() { "!" } else { "" },
                            key_text(path),
                            key_text(&im.self_ty)
                        ),
                        None => key_text(&im.self_ty),
                    };
                    let mut header = toks_of(&im.generics);
                    if let Some((_, path, _)) = &im.trait_ {
                        header.extend(toks_of(path));
                    }
                    header.extend(toks_of(&im.self_ty));
                    if let Some(w) = &im.generics.where_clause {
                        header.extend(toks_of(w));
                    }
                    if im.unsafety.is_some() {
                        header.push(Tok {
                            s: "unsafe".into(),
                            k: K::Ident,
                            line: 0,
                            col: 0,
                            end_line: 0,
                            end_col: 0,
                        });
                    }
                    // Header span: from the first attr/`impl` to the `{`.
                    let brace = all
                        .iter()
                        .position(|t| {
                            t.k == K::Open
                                && t.s == "{"
                                && t.line >= im.impl_token.span.start().line
                        })
                        .unwrap_or(all.len());
                    self.emit(
                        "impl",
                        ctx.clone(),
                        &im.attrs,
                        header,
                        &all[..brace.max(1).min(all.len())],
                        &uses,
                        modpath,
                    );
                    for ii in &im.items {
                        let span = toks_of(ii);
                        match ii {
                            syn::ImplItem::Fn(f) => {
                                let body_uses = body_use_map(&uses, &f.block);
                                let mut parts = toks_of(&f.sig);
                                if f.defaultness.is_some() {
                                    parts.insert(
                                        0,
                                        Tok {
                                            s: "default".into(),
                                            k: K::Ident,
                                            line: 0,
                                            col: 0,
                                            end_line: 0,
                                            end_col: 0,
                                        },
                                    );
                                }
                                parts.extend(block_toks(&f.block));
                                self.emit(
                                    "fn",
                                    format!("{ctx}::{}", f.sig.ident),
                                    &f.attrs,
                                    parts,
                                    &span,
                                    &body_uses,
                                    modpath,
                                );
                            }
                            syn::ImplItem::Const(c) => {
                                let mut parts = toks_of(&c.ty);
                                parts.extend(toks_of(&c.expr));
                                self.emit(
                                    "const",
                                    format!("{ctx}::{}", c.ident),
                                    &c.attrs,
                                    parts,
                                    &span,
                                    &uses,
                                    modpath,
                                );
                            }
                            syn::ImplItem::Type(t) => {
                                self.emit(
                                    "type",
                                    format!("{ctx}::{}", t.ident),
                                    &t.attrs,
                                    toks_of(&t.ty),
                                    &span,
                                    &uses,
                                    modpath,
                                );
                            }
                            syn::ImplItem::Macro(m) => {
                                self.emit(
                                    "macro-call",
                                    format!("{ctx}::{}!", key_text(&m.mac.path)),
                                    &m.attrs,
                                    toks_of(&m.mac),
                                    &span,
                                    &uses,
                                    modpath,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                syn::Item::Trait(t) => {
                    let mut stripped = t.clone();
                    stripped.attrs.retain(|a| !is_doc(a));
                    for ti in &mut stripped.items {
                        match ti {
                            syn::TraitItem::Fn(f) => {
                                f.default = None;
                                f.attrs.retain(|a| !is_doc(a));
                            }
                            syn::TraitItem::Const(c) => c.attrs.retain(|a| !is_doc(a)),
                            syn::TraitItem::Type(ty) => ty.attrs.retain(|a| !is_doc(a)),
                            _ => {}
                        }
                    }
                    stripped.vis = syn::Visibility::Inherited;
                    let name = t.ident.to_string();
                    self.emit(
                        "trait",
                        name.clone(),
                        &[],
                        toks_of(&stripped),
                        &all,
                        &uses,
                        modpath,
                    );
                    for ti in &t.items {
                        if let syn::TraitItem::Fn(f) = ti {
                            if let Some(b) = &f.default {
                                let body_uses = body_use_map(&uses, b);
                                let mut parts = toks_of(&f.sig);
                                parts.extend(block_toks(b));
                                self.emit(
                                    "fn",
                                    format!("{name}::{}", f.sig.ident),
                                    &f.attrs,
                                    parts,
                                    &toks_of(ti),
                                    &body_uses,
                                    modpath,
                                );
                            }
                        }
                    }
                }
                syn::Item::Struct(s) => {
                    let mut c = s.clone();
                    c.attrs.clear();
                    for f in c.fields.iter_mut() {
                        f.attrs.retain(|a| !is_doc(a));
                    }
                    self.emit(
                        "struct",
                        s.ident.to_string(),
                        &s.attrs,
                        toks_of(&c),
                        &all,
                        &uses,
                        modpath,
                    );
                }
                syn::Item::Enum(e) => {
                    let mut c = e.clone();
                    c.attrs.clear();
                    for v in c.variants.iter_mut() {
                        v.attrs.retain(|a| !is_doc(a));
                        for f in v.fields.iter_mut() {
                            f.attrs.retain(|a| !is_doc(a));
                        }
                    }
                    self.emit(
                        "enum",
                        e.ident.to_string(),
                        &e.attrs,
                        toks_of(&c),
                        &all,
                        &uses,
                        modpath,
                    );
                }
                syn::Item::Union(u) => {
                    let mut c = u.clone();
                    c.attrs.clear();
                    self.emit(
                        "union",
                        u.ident.to_string(),
                        &u.attrs,
                        toks_of(&c),
                        &all,
                        &uses,
                        modpath,
                    );
                }
                syn::Item::Const(c) => {
                    let mut parts = toks_of(&c.ty);
                    parts.extend(toks_of(&c.expr));
                    self.emit(
                        "const",
                        c.ident.to_string(),
                        &c.attrs,
                        parts,
                        &all,
                        &uses,
                        modpath,
                    );
                }
                syn::Item::Static(s) => {
                    let mut parts = toks_of(&s.mutability);
                    parts.extend(toks_of(&s.ty));
                    parts.extend(toks_of(&s.expr));
                    self.emit(
                        "static",
                        s.ident.to_string(),
                        &s.attrs,
                        parts,
                        &all,
                        &uses,
                        modpath,
                    );
                }
                syn::Item::Type(t) => {
                    let mut parts = toks_of(&t.generics);
                    parts.extend(toks_of(&t.ty));
                    self.emit(
                        "type",
                        t.ident.to_string(),
                        &t.attrs,
                        parts,
                        &all,
                        &uses,
                        modpath,
                    );
                }
                syn::Item::Macro(m) => {
                    let path = key_text(&m.mac.path);
                    let key = if let Some(id) = &m.ident {
                        format!("{path}! {id}")
                    } else {
                        // Item-level macro call: key by macro name and the
                        // first identifier inside it (thread_local!, ...).
                        let inner = toks_of(&m.mac.tokens);
                        let first = inner
                            .iter()
                            .find(|t| {
                                t.k == K::Ident
                                    && !matches!(
                                        t.s.as_str(),
                                        "pub"
                                            | "static"
                                            | "const"
                                            | "fn"
                                            | "struct"
                                            | "enum"
                                            | "mut"
                                            | "crate"
                                    )
                            })
                            .map_or(String::new(), |t| t.s.clone());
                        let base = format!("{path}!{first}");
                        let n = macro_seq.entry(base.clone()).or_default();
                        *n += 1;
                        if *n > 1 {
                            format!("{base}#{n}")
                        } else {
                            base
                        }
                    };
                    let kind = if m.ident.is_some() {
                        "macro_rules"
                    } else {
                        "macro-call"
                    };
                    self.emit(kind, key, &m.attrs, toks_of(&m.mac), &all, &uses, modpath);
                }
                syn::Item::Mod(m) => {
                    // The module's own attributes (`#[cfg(test)]`, lint
                    // allows), without `#[path]`: `mod x { .. }` and
                    // `#[path = ".."] mod x;` hash the same.
                    let attrs: Vec<syn::Attribute> = m
                        .attrs
                        .iter()
                        .filter(|a| !a.path().is_ident("path"))
                        .cloned()
                        .collect();
                    let head: Vec<Tok> = all
                        .iter()
                        .take_while(|t| !(t.k == K::Open && t.s == "{"))
                        .cloned()
                        .collect();
                    let ident = vec![Tok {
                        s: m.ident.to_string(),
                        k: K::Ident,
                        line: 0,
                        col: 0,
                        end_line: 0,
                        end_col: 0,
                    }];
                    self.emit(
                        "mod",
                        m.ident.to_string(),
                        &attrs,
                        ident,
                        &head,
                        &uses,
                        modpath,
                    );
                    if let Some((_, inner)) = &m.content {
                        let mp = if modpath.is_empty() {
                            m.ident.to_string()
                        } else {
                            format!("{modpath}::{}", m.ident)
                        };
                        self.items(inner, &mp);
                    }
                }
                syn::Item::ForeignMod(fm) => {
                    self.emit(
                        "extern",
                        format!("extern {}", key_text(&fm.abi)),
                        &fm.attrs,
                        toks_of(
                            &fm.items
                                .iter()
                                .map(|i| i.to_token_stream())
                                .collect::<TokenStream>(),
                        ),
                        &all,
                        &uses,
                        modpath,
                    );
                }
                _ => {}
            }
        }
    }
}

/// Body-level `use` declarations extend the module's use map (only the
/// top-level statements of the fn body).
fn body_use_map(module: &UseMap, block: &syn::Block) -> UseMap {
    let items: Vec<syn::Item> = block
        .stmts
        .iter()
        .filter_map(|s| match s {
            syn::Stmt::Item(it @ syn::Item::Use(_)) => Some(it.clone()),
            _ => None,
        })
        .collect();
    if items.is_empty() {
        return module.clone();
    }
    let mut m = module.clone();
    m.extend(use_map_of(&items));
    m
}

/// Body tokens without the top-level `use` statements.
fn block_toks(block: &syn::Block) -> Vec<Tok> {
    let mut v = vec![Tok {
        s: "{".into(),
        k: K::Open,
        line: 0,
        col: 0,
        end_line: 0,
        end_col: 0,
    }];
    for s in &block.stmts {
        if matches!(s, syn::Stmt::Item(syn::Item::Use(_))) {
            continue;
        }
        v.extend(toks_of(s));
    }
    v.push(Tok {
        s: "}".into(),
        k: K::Close,
        line: 0,
        col: 0,
        end_line: 0,
        end_col: 0,
    });
    v
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if name == "target" || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            rs_files(&p, out);
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

fn file_modpath(rel: &Path) -> String {
    let mut segs: Vec<String> = rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    if let Some(last) = segs.pop() {
        let stem = last.trim_end_matches(".rs").to_string();
        if !matches!(stem.as_str(), "mod" | "main" | "lib") {
            segs.push(stem);
        }
    }
    segs.join("::")
}

fn cmd_items(args: &[String]) -> ExitCode {
    let mut rows = Vec::new();
    let mut errors = 0;
    let mut nfiles = 0;
    for a in args {
        let (label, dir) = a.split_once('=').unwrap_or(("", a.as_str()));
        let dir = PathBuf::from(dir);
        let mut files = Vec::new();
        if dir.is_file() {
            files.push(dir.clone());
        } else {
            rs_files(&dir, &mut files);
        }
        for f in files {
            nfiles += 1;
            let text = match fs::read_to_string(&f) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("rsscan: {}: {e}", f.display());
                    errors += 1;
                    continue;
                }
            };
            let rel = f.strip_prefix(&dir).unwrap_or(&f).to_path_buf();
            let shown = if label.is_empty() {
                f.display().to_string()
            } else {
                format!("{label}/{}", rel.display())
            };
            let parsed = match syn::parse_file(&text) {
                Ok(p) => p,
                Err(e) => {
                    let lc = e.span().start();
                    println!("#parse-error\t{shown}:{}:{}\t{e}", lc.line, lc.column);
                    eprintln!("rsscan: parse error {shown}:{}: {e}", lc.line);
                    errors += 1;
                    continue;
                }
            };
            let src = Source::new(text);
            let mp = file_modpath(&rel);
            let modpath = if label.is_empty() {
                mp
            } else if mp.is_empty() {
                label.to_string()
            } else {
                format!("{label}::{mp}")
            };
            let mut cx = ItemCx {
                src: &src,
                file: &shown,
                rows: &mut rows,
            };
            cx.items(&parsed.items, &modpath);
        }
    }
    println!(
        "#rsscan-items v2\tkind\tkey\tcode\tdoc\tcomments\tntok\tlocation\tmodpath\tshape\tidents"
    );
    for r in &rows {
        let idents = if r.idents.is_empty() {
            "-".to_string()
        } else {
            r.idents.join(" ")
        };
        println!(
            "{}\t{}\t{:016x}\t{:016x}\t{:016x}\t{}\t{}:{}\t{}\t{:016x}\t{}",
            r.kind,
            r.key,
            r.code,
            r.doc,
            r.comments,
            r.ntok,
            r.file,
            r.line,
            r.modpath,
            r.shape,
            idents
        );
    }
    eprintln!(
        "rsscan: {} files, {} items, {} errors",
        nfiles,
        rows.len(),
        errors
    );
    if errors > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// `deps`
// ---------------------------------------------------------------------------

struct Deps {
    root_names: BTreeSet<String>,
    aliases: BTreeSet<String>,
    externs: BTreeSet<String>,
    out: Vec<String>,
    visited: BTreeSet<PathBuf>,
    base: PathBuf,
}

fn cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let toks = toks_of(&a.meta);
        let has_test = toks.iter().any(|t| t.k == K::Ident && t.s == "test");
        let has_not = toks.iter().any(|t| t.k == K::Ident && t.s == "not");
        has_test && !has_not
    })
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    for a in attrs {
        if a.path().is_ident("path") {
            if let syn::Meta::NameValue(nv) = &a.meta {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                {
                    return Some(s.value());
                }
            }
        }
    }
    None
}

/// The file of an out-of-line `mod x;` and whether it is a `mod.rs` (whose
/// children live beside it), resolved like rustc: `#[path]` relative to
/// `path_dir`, else `child_dir/x.rs`, else `child_dir/x/mod.rs`.
fn module_file(m: &syn::ItemMod, path_dir: &Path, child_dir: &Path) -> (PathBuf, bool) {
    if let Some(p) = path_attr(&m.attrs) {
        let f = path_dir.join(p);
        let is_mod_rs = f.file_name().is_some_and(|n| n == "mod.rs");
        return (f, is_mod_rs);
    }
    let a = child_dir.join(format!("{}.rs", m.ident));
    if a.exists() {
        (a, false)
    } else {
        (child_dir.join(m.ident.to_string()).join("mod.rs"), true)
    }
}

impl Deps {
    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.base)
            .unwrap_or(p)
            .display()
            .to_string()
    }

    /// Resolve a path (as written) from module `cur` to an absolute module
    /// path; None when it is not crate-internal.
    fn resolve(&self, cur: &[String], segs: &[String]) -> Option<Vec<String>> {
        let first = segs.first()?;
        if first == "crate" || self.aliases.contains(first) {
            return Some(segs[1..].to_vec());
        }
        if first == "super" || first == "self" {
            let mut base = cur.to_vec();
            let mut i = 0;
            while i < segs.len() && (segs[i] == "super" || segs[i] == "self") {
                if segs[i] == "super" {
                    base.pop();
                }
                i += 1;
            }
            base.extend_from_slice(&segs[i..]);
            return Some(base);
        }
        if cur.is_empty() && self.root_names.contains(first) {
            return Some(segs.to_vec());
        }
        None
    }

    fn record(&mut self, cur: &[String], segs: &[String], file: &str, line: usize, test: bool) {
        let from = cur.first().cloned().unwrap_or_else(|| "<root>".into());
        if let Some(abs) = self.resolve(cur, segs) {
            // `pub(crate)`, `pub(super)`, bare `self`: not a reference.
            if abs.is_empty() || segs.len() == 1 {
                return;
            }
            let to = match abs.first() {
                Some(t) if self.root_names.contains(t) => t.clone(),
                Some(_) | None => "<root>".into(),
            };
            if to != from {
                self.out.push(format!(
                    "edge\t{from}\t{to}\t{file}:{line}\t{}",
                    u8::from(test)
                ));
            }
        } else if let Some(first) = segs.first() {
            if self.externs.contains(first) && segs.len() > 1 {
                self.out.push(format!(
                    "ext\t{from}\t{first}\t{file}:{line}\t{}",
                    u8::from(test)
                ));
            }
        }
    }

    fn scan_tokens(&mut self, cur: &[String], toks: &[Tok], file: &str, test: bool) {
        let mut i = 0;
        while i < toks.len() {
            if toks[i].k == K::Punct
                && toks[i].s == "$"
                && toks.get(i + 1).is_some_and(|n| n.s == "crate")
            {
                let (mut segs, j) = path_run(toks, i + 1);
                segs[0] = "crate".into();
                self.record_run(cur, toks, &segs, j, toks[i].line, file, test);
                i = j;
                continue;
            }
            // `::wgpu::x` (leading global path)
            if toks[i].k == K::Punct
                && toks[i].s == "::"
                && toks.get(i + 1).is_some_and(|n| n.k == K::Ident)
                && (i == 0 || !matches!(toks[i - 1].k, K::Ident | K::Close) || toks[i - 1].s == "<")
            {
                let (segs, j) = path_run(toks, i + 1);
                if self.externs.contains(&segs[0]) && segs.len() > 1 {
                    let from = cur.first().cloned().unwrap_or_else(|| "<root>".into());
                    self.out.push(format!(
                        "ext\t{from}\t{}\t{file}:{}\t{}",
                        segs[0],
                        toks[i].line,
                        u8::from(test)
                    ));
                }
                i = j;
                continue;
            }
            if path_head(toks, i) {
                let (segs, j) = path_run(toks, i);
                self.record_run(cur, toks, &segs, j, toks[i].line, file, test);
                i = j;
                continue;
            }
            i += 1;
        }
    }

    /// Handles `crate::{a, b::c}` groups (use statements inside fn bodies
    /// and macro input) by recording each group element's head.
    #[allow(clippy::too_many_arguments)]
    fn record_run(
        &mut self,
        cur: &[String],
        toks: &[Tok],
        segs: &[String],
        j: usize,
        line: usize,
        file: &str,
        test: bool,
    ) {
        let group_follows = toks.get(j).is_some_and(|t| t.s == "::")
            && toks
                .get(j + 1)
                .is_some_and(|t| t.k == K::Open && t.s == "{");
        if group_follows {
            // element heads: idents right after `{` or `,` at depth 1
            let mut depth = 0;
            let mut k = j + 1;
            let mut expect_head = false;
            while k < toks.len() {
                let t = &toks[k];
                match t.k {
                    K::Open => {
                        depth += 1;
                        expect_head = depth == 1;
                    }
                    K::Close => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    K::Punct if t.s == "," && depth == 1 => expect_head = true,
                    K::Ident if expect_head && depth == 1 => {
                        let mut full = segs.to_vec();
                        if t.s != "self" {
                            full.push(t.s.clone());
                        }
                        self.record(cur, &full, file, t.line, test);
                        expect_head = false;
                    }
                    _ => expect_head = false,
                }
                k += 1;
            }
        } else {
            self.record(cur, segs, file, line, test);
        }
    }

    fn walk_file(&mut self, file: &Path, modpath: Vec<String>, test: bool, mod_rs: bool) {
        let canon = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        let rel = self.rel(file);
        self.out.push(format!(
            "module\t{}\t{}\t{}",
            if modpath.is_empty() {
                "<root>".into()
            } else {
                modpath.join("::")
            },
            rel,
            u8::from(test)
        ));
        let _ = self.visited.insert(canon);
        let text = match fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => {
                self.out.push(format!("error\t{rel}\t{e}"));
                return;
            }
        };
        let parsed = match syn::parse_file(&text) {
            Ok(p) => p,
            Err(e) => {
                self.out
                    .push(format!("error\t{rel}:{}\t{e}", e.span().start().line));
                return;
            }
        };
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        // Directory for child `mod x;` files (non-mod-rs files own a dir named after them).
        let child_dir = if mod_rs {
            dir.clone()
        } else {
            dir.join(file.file_stem().unwrap())
        };
        self.walk_items(&parsed.items, &modpath, test, &rel, &dir, &child_dir);
    }

    fn walk_items(
        &mut self,
        items: &[syn::Item],
        modpath: &[String],
        test: bool,
        rel: &str,
        path_dir: &Path,
        child_dir: &Path,
    ) {
        for it in items {
            match it {
                syn::Item::Mod(m) => {
                    let t = test || cfg_test(&m.attrs);
                    let mut mp = modpath.to_vec();
                    mp.push(m.ident.to_string());
                    if let Some((_, inner)) = &m.content {
                        self.out.push(format!(
                            "module\t{}\t{}\t{}",
                            mp.join("::"),
                            rel,
                            u8::from(t)
                        ));
                        let cd = child_dir.join(m.ident.to_string());
                        // #[path] inside inline modules is relative to the inline dir.
                        self.walk_items(inner, &mp, t, rel, &cd, &cd);
                    } else {
                        let (file, mod_rs) = module_file(m, path_dir, child_dir);
                        self.walk_file(&file, mp, t, mod_rs);
                    }
                }
                syn::Item::Use(u) => {
                    let t = test || cfg_test(&u.attrs);
                    let mut out = Vec::new();
                    expand_use(&u.tree, &mut Vec::new(), &mut out);
                    let line = u.use_token.span.start().line;
                    for (_, full) in out {
                        let full: Vec<String> = full.into_iter().filter(|s| s != "*").collect();
                        if full.is_empty() {
                            continue;
                        }
                        self.record(modpath, &full, rel, line, t);
                    }
                }
                other => {
                    let attrs: &[syn::Attribute] = match other {
                        syn::Item::Fn(x) => &x.attrs,
                        syn::Item::Impl(x) => &x.attrs,
                        syn::Item::Struct(x) => &x.attrs,
                        syn::Item::Enum(x) => &x.attrs,
                        syn::Item::Const(x) => &x.attrs,
                        syn::Item::Static(x) => &x.attrs,
                        syn::Item::Trait(x) => &x.attrs,
                        syn::Item::Type(x) => &x.attrs,
                        syn::Item::Macro(x) => &x.attrs,
                        syn::Item::Union(x) => &x.attrs,
                        _ => &[],
                    };
                    let t = test || cfg_test(attrs);
                    if let syn::Item::Impl(im) = other {
                        // cfg(test) on impl items (test-only helper methods)
                        let head = toks_of(&im.self_ty);
                        self.scan_tokens(modpath, &head, rel, t);
                        if let Some((_, p, _)) = &im.trait_ {
                            self.scan_tokens(modpath, &toks_of(p), rel, t);
                        }
                        for ii in &im.items {
                            let ia: &[syn::Attribute] = match ii {
                                syn::ImplItem::Fn(f) => &f.attrs,
                                syn::ImplItem::Const(c) => &c.attrs,
                                syn::ImplItem::Type(c) => &c.attrs,
                                syn::ImplItem::Macro(c) => &c.attrs,
                                _ => &[],
                            };
                            let tt = t || cfg_test(ia);
                            self.scan_tokens(modpath, &toks_of(ii), rel, tt);
                        }
                        continue;
                    }
                    let toks = toks_of(other);
                    self.scan_tokens(modpath, &toks, rel, t);
                }
            }
        }
    }
}

fn cmd_deps(args: &[String]) -> ExitCode {
    let mut root = None;
    let mut aliases = BTreeSet::new();
    let mut externs: BTreeSet<String> = ["wgpu", "winit", "tokio", "cpal"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut prefix: Vec<String> = Vec::new();
    let mut root_names_from: Option<PathBuf> = None;
    let mut crate_aliases: Vec<(String, PathBuf)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--alias" => {
                aliases.insert(args[i + 1].clone());
                i += 2;
            }
            // `--crate-alias rs910_core=<its lib.rs>`: paths through that
            // crate (or through a `pub use` facade of one of its modules)
            // resolve to its top-level modules, which join the root names.
            "--crate-alias" => {
                let Some((name, lib)) = args[i + 1].split_once('=') else {
                    eprintln!("--crate-alias expects name=path/to/lib.rs");
                    return ExitCode::from(2);
                };
                aliases.insert(name.to_string());
                crate_aliases.push((name.to_string(), PathBuf::from(lib)));
                i += 2;
            }
            "--extern" => {
                externs = args[i + 1].split(',').map(str::to_string).collect();
                i += 2;
            }
            "--prefix" => {
                prefix = vec![args[i + 1].clone()];
                i += 2;
            }
            "--names-from" => {
                root_names_from = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            a => {
                root = Some(PathBuf::from(a));
                i += 1;
            }
        }
    }
    let Some(root) = root else {
        eprintln!("usage: rsscan deps <crate-root.rs> [--alias name] [--crate-alias name=lib.rs] [--extern a,b] [--names-from lib.rs]");
        return ExitCode::from(2);
    };
    // Top-level module names: `mod` declarations in the crate root (or in
    // the library root when scanning a bin that uses it through an alias).
    let names_src = root_names_from.as_ref().unwrap_or(&root);
    let mut root_names = BTreeSet::new();
    let mut name_sources = vec![names_src.clone()];
    name_sources.extend(crate_aliases.iter().map(|(_, lib)| lib.clone()));
    for src in &name_sources {
        if let Ok(parsed) = fs::read_to_string(src)
            .map_err(|e| e.to_string())
            .and_then(|t| syn::parse_file(&t).map_err(|e| e.to_string()))
        {
            for it in &parsed.items {
                if let syn::Item::Mod(m) = it {
                    root_names.insert(m.ident.to_string());
                }
            }
        }
    }
    let base = root.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut d = Deps {
        root_names,
        aliases,
        externs,
        out: Vec::new(),
        visited: BTreeSet::new(),
        base,
    };
    for n in &d.root_names.clone() {
        d.out.push(format!("top\t{n}"));
    }
    d.walk_file(&root, prefix, false, true);
    println!("#rsscan-deps v1");
    for l in &d.out {
        println!("{l}");
    }
    let errs = d.out.iter().filter(|l| l.starts_with("error\t")).count();
    if errs > 0 {
        eprintln!("rsscan deps: {errs} errors");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// tests: the no-pack status of every test, without a no-pack build
// ---------------------------------------------------------------------------

/// The `no-pack` feature only switches `#[cfg_attr(feature = "no-pack",
/// ignore ...)]` on tests, so which tests a `--features no-pack` build
/// reports ignored can be read from the source. `tests` walks a test
/// target's module tree like rustc and prints every `#[test]` fn with its
/// libtest path. It also counts every `feature = "no-pack"` token sequence
/// in the walked files: when that count differs from the number of
/// canonical attributes found on tests (a `cfg(feature = "no-pack")` item, an
/// attribute inside a macro, a module-level gate), the source cannot be
/// read exactly and the caller must fall back to a real no-pack build.
struct TestScan {
    out: Vec<String>,
    base: PathBuf,
    canonical: usize,
    tokens: usize,
}

/// `#[test]`, `#[tokio::test]`, ...: the attribute path ends in `test`.
fn is_test_attr(a: &syn::Attribute) -> bool {
    a.path().segments.last().is_some_and(|s| s.ident == "test")
}

/// `#[cfg_attr(feature = "no-pack", ignore)]` or `ignore = "reason"`.
fn is_nopack_ignore(a: &syn::Attribute) -> bool {
    if !a.path().is_ident("cfg_attr") {
        return false;
    }
    let toks: Vec<String> = toks_of(&a.meta).into_iter().map(|t| t.s).collect();
    toks.len() >= 7
        && toks[1] == "("
        && toks[2] == "feature"
        && toks[3] == "="
        && toks[4] == "\"no-pack\""
        && toks[5] == ","
        && toks[6] == "ignore"
}

/// Occurrences of `feature = "no-pack"` (tokens, so comments and doc text
/// never count).
fn nopack_feature_tokens(ts: TokenStream) -> usize {
    let tts: Vec<TokenTree> = ts.into_iter().collect();
    let mut n = 0;
    for (i, tt) in tts.iter().enumerate() {
        match tt {
            TokenTree::Group(g) => n += nopack_feature_tokens(g.stream()),
            TokenTree::Ident(id) if id == "feature" => {
                let eq = matches!(tts.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '=');
                let lit = matches!(tts.get(i + 2), Some(TokenTree::Literal(l)) if l.to_string() == "\"no-pack\"");
                if eq && lit {
                    n += 1;
                }
            }
            _ => {}
        }
    }
    n
}

impl TestScan {
    fn walk_file(&mut self, file: &Path, modpath: Vec<String>, mod_rs: bool) {
        let rel = file
            .strip_prefix(&self.base)
            .unwrap_or(file)
            .display()
            .to_string();
        let text = match fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => {
                self.out.push(format!("error\t{rel}\t{e}"));
                return;
            }
        };
        match text.parse::<TokenStream>() {
            Ok(ts) => self.tokens += nopack_feature_tokens(ts),
            Err(e) => self.out.push(format!("error\t{rel}\t{e}")),
        }
        let parsed = match syn::parse_file(&text) {
            Ok(p) => p,
            Err(e) => {
                self.out
                    .push(format!("error\t{rel}:{}\t{e}", e.span().start().line));
                return;
            }
        };
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let child_dir = if mod_rs {
            dir.clone()
        } else {
            dir.join(file.file_stem().unwrap())
        };
        self.walk_items(&parsed.items, &modpath, &rel, &dir, &child_dir);
    }

    fn walk_items(
        &mut self,
        items: &[syn::Item],
        modpath: &[String],
        rel: &str,
        path_dir: &Path,
        child_dir: &Path,
    ) {
        for it in items {
            match it {
                syn::Item::Mod(m) => {
                    let mut mp = modpath.to_vec();
                    mp.push(m.ident.to_string());
                    if let Some((_, inner)) = &m.content {
                        let cd = child_dir.join(m.ident.to_string());
                        self.walk_items(inner, &mp, rel, &cd, &cd);
                    } else {
                        let (file, mod_rs) = module_file(m, path_dir, child_dir);
                        self.walk_file(&file, mp, mod_rs);
                    }
                }
                syn::Item::Fn(f) if f.attrs.iter().any(is_test_attr) => {
                    let ignore = f.attrs.iter().any(|a| a.path().is_ident("ignore"));
                    let nopack = f.attrs.iter().filter(|a| is_nopack_ignore(a)).count();
                    self.canonical += nopack;
                    let mut path = modpath.to_vec();
                    path.push(f.sig.ident.to_string());
                    self.out.push(format!(
                        "test\t{}\t{}\t{}\t{rel}:{}",
                        path.join("::"),
                        u8::from(ignore),
                        u8::from(nopack > 0),
                        f.sig.ident.span().start().line
                    ));
                }
                _ => {}
            }
        }
    }
}

fn cmd_tests(args: &[String]) -> ExitCode {
    let [root] = args else {
        eprintln!("usage: rsscan tests <test-target-root.rs>");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(root);
    let mut scan = TestScan {
        out: Vec::new(),
        base: root.parent().unwrap_or(Path::new(".")).to_path_buf(),
        canonical: 0,
        tokens: 0,
    };
    scan.walk_file(&root, Vec::new(), true);
    println!("#rsscan-tests v1");
    for l in &scan.out {
        println!("{l}");
    }
    println!("nopack-attrs\t{}", scan.canonical);
    println!("nopack-tokens\t{}", scan.tokens);
    let errs = scan.out.iter().filter(|l| l.starts_with("error\t")).count();
    if errs > 0 {
        eprintln!("rsscan tests: {errs} errors");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("items") => cmd_items(&args[1..]),
        Some("deps") => cmd_deps(&args[1..]),
        Some("tests") => cmd_tests(&args[1..]),
        Some("magic-ids") => magic_ids::cmd(&args[1..]),
        _ => {
            eprintln!("usage: rsscan items <[label=]dir|file>...\n       rsscan deps <crate-root.rs> [--alias name] [--crate-alias name=lib.rs] [--extern a,b] [--names-from lib.rs] [--prefix name]\n       rsscan tests <test-target-root.rs>\n       rsscan magic-ids <dir|file>...");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_of(src: &str) -> Vec<Row> {
        let parsed = syn::parse_file(src).unwrap();
        let s = Source::new(src.to_string());
        let mut rows = Vec::new();
        let mut cx = ItemCx {
            src: &s,
            file: "t.rs",
            rows: &mut rows,
        };
        cx.items(&parsed.items, "t");
        rows
    }

    fn only(src: &str) -> Row {
        let mut r = rows_of(src);
        assert_eq!(r.len(), 1, "expected one item in {src}");
        r.pop().unwrap()
    }

    #[test]
    fn shape_survives_a_rename_but_not_a_swap() {
        let base = only("fn area(w: i32, h: i32) -> i32 { w * 2 - h }");
        // Consistent rename of the fn, params and a called name: same shape,
        // different code hash.
        let renamed = only("fn extent(width: i32, height: i32) -> i32 { width * 2 - height }");
        assert_eq!(base.shape, renamed.shape);
        assert_ne!(base.code, renamed.code);
        // a - b -> b - a with both operands params: not hidden.
        let swapped = only("fn area(w: i32, h: i32) -> i32 { h * 2 - w }");
        assert_ne!(base.shape, swapped.shape);
        // f(x, y) -> f(y, x) with both params: not hidden.
        let call = only("fn g(x: i32, y: i32) -> i32 { f(x, y) }");
        let call_swapped = only("fn g(x: i32, y: i32) -> i32 { f(y, x) }");
        assert_ne!(call.shape, call_swapped.shape);
        // Literals, operators and keywords are compared exactly.
        assert_ne!(
            base.shape,
            only("fn area(w: i32, h: i32) -> i32 { w * 3 - h }").shape
        );
        assert_ne!(
            base.shape,
            only("fn area(w: i32, h: i32) -> i32 { w * 2 + h }").shape
        );
        assert_ne!(
            base.shape,
            only("fn area(w: i32, h: i64) -> i32 { w * 2 - h }").shape
        );
    }

    #[test]
    fn swapped_fields_share_a_shape_but_not_the_identifier_order() {
        // `self.a - self.b` -> `self.b - self.a` alpha-normalises to the same
        // shape (both fields are first seen in the body), so fn-hash.py
        // relies on the identifier lists to reject it.
        let fn_row = |src: &str| rows_of(src).into_iter().find(|r| r.kind == "fn").unwrap();
        let a = fn_row("impl S { fn d(&self) -> i32 { self.a - self.b } }");
        let b = fn_row("impl S { fn d(&self) -> i32 { self.b - self.a } }");
        assert_eq!(a.shape, b.shape);
        assert_ne!(a.idents, b.idents);
    }
}
