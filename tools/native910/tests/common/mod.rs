//! Shared helpers for the integration tests.
//!
//! Pack-dependent tests read the revision-910 cache in `server/data/pack`
//! (gitignored). They are marked
//! `#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]` so CI
//! (`--features no-pack`) reports them as ignored, and they call
//! [`require_pack_file`] so a local run without the pack FAILS naming the
//! file instead of returning early and reporting `ok`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The repository root of the checkout (or git worktree) being tested, found
/// when the test runs and never baked in at compile time (two worktrees that
/// share a cargo target directory can reuse an unchanged build). Order:
/// `ALTO_REPO_ROOT`; the first directory at or above the current directory
/// holding `tools/Cargo.toml` and `server/` (cargo runs tests and examples with
/// the current directory at the package root); this crate's own checkout.
///
/// native910 depends on no workspace crate, so this mirrors
/// `rs910_core::test_support::repo_root` (which carries the test).
pub fn repo_root() -> PathBuf {
    if let Some(root) = std::env::var_os("ALTO_REPO_ROOT") {
        return PathBuf::from(root);
    }
    let start = std::env::current_dir().unwrap_or_default();
    start
        .ancestors()
        .find(|dir| dir.join("tools/Cargo.toml").is_file() && dir.join("server").is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// `server/data/pack`: `ALTO_PACK_DIR` when set, else under [`repo_root`].
pub fn pack_root() -> PathBuf {
    std::env::var_os("ALTO_PACK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("server/data/pack"))
}

/// `tools/native910`.
pub fn native_dir() -> PathBuf {
    repo_root().join("tools/native910")
}

/// The path of `file` (for example `client.scripts.js5`) in the pack, after
/// asserting that it exists.
#[track_caller]
pub fn require_pack_file(file: &str) -> PathBuf {
    let path = pack_root().join(file);
    assert!(
        path.is_file(),
        "{} is missing: this test needs the revision-910 cache in server/data/pack",
        path.display()
    );
    path
}

/// The pack root, after asserting that every listed file exists in it.
#[track_caller]
pub fn require_pack(files: &[&str]) -> PathBuf {
    for file in files {
        require_pack_file(file);
    }
    pack_root()
}

/// Committed fixtures under `tests/fixtures/<name>`.
pub fn fixture(name: &str) -> PathBuf {
    native_dir().join("tests/fixtures").join(name)
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").unwrap();
        out
    })
}

/// Bytes of lowercase/uppercase hex `text`.
#[track_caller]
pub fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "odd hex length");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex digit"))
        .collect()
}

/// String rendering used by the CS2 trace harnesses: each value is
/// `n` (null) or `s` followed by its UTF-16 code units as 4-digit hex
/// (lone surrogates included, via [`native910::jstr::units`]).
pub fn utf16_objects<'a>(values: impl Iterator<Item = Option<&'a str>>) -> String {
    use std::fmt::Write;
    let mut out = String::from("[");
    for (i, value) in values.enumerate() {
        if i != 0 {
            out.push(',');
        }
        match value {
            None => out.push('n'),
            Some(value) => {
                out.push('s');
                for unit in native910::jstr::units(value) {
                    write!(out, "{unit:04x}").unwrap();
                }
            }
        }
    }
    out.push(']');
    out
}

/// FNV-1a 64 of `bytes`.
pub fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// How one recorded trace line is stored when the trace comes from cache
/// scripts: its FNV-1a 64 as 16 hex digits.
pub fn line_digest(line: &str) -> String {
    format!("{:016x}", fnv64(line.as_bytes()))
}

/// The first step at which `lines` differs from the recorded trace `base`
/// (a path without extension), or `None` when they agree.
///
/// Recordings of real cache scripts are committed as `<base>.dig`, one
/// [`line_digest`] per instruction, so no cache content is stored; a
/// recording of our own synthetic scripts is committed verbatim as
/// `<base>.trace`. A length difference is a divergence at the shorter end.
#[track_caller]
pub fn first_trace_divergence<S: AsRef<str>>(base: &Path, lines: &[S]) -> Option<usize> {
    let dig = base.with_extension("dig");
    let recorded: Vec<String> = if dig.is_file() {
        std::fs::read_to_string(&dig)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    } else {
        let trace = base.with_extension("trace");
        std::fs::read_to_string(&trace)
            .unwrap_or_else(|e| panic!("recorded trace {}: {e}", trace.display()))
            .lines()
            .map(line_digest)
            .collect()
    };
    let mut steps = recorded.iter().zip(lines);
    if let Some(step) = steps.position(|(r, l)| *r != line_digest(l.as_ref())) {
        return Some(step);
    }
    (recorded.len() != lines.len()).then(|| recorded.len().min(lines.len()))
}

/// Assert that the pack file at `path` exists (the require-pack contract for
/// tests that build their own pack paths).
#[track_caller]
pub fn require_present(path: &Path) {
    assert!(
        path.is_file(),
        "{} is missing: this test needs the revision-910 cache in server/data/pack",
        path.display()
    );
}
