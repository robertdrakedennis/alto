//! Compact recordings of the original client committed under
//! `fixtures/recorded-goldens/`.
//!
//! Large arrays (colour tables, scene dumps, raster traces) were recorded once
//! and are committed as one line per array: its length, an FNV-1a 64 hash of
//! its words and up to [`SAMPLES`] evenly spaced sample values (every value
//! when the array has at most [`KEEP_ALL`] words, so small inputs such as the
//! atan2 tie pairs can be read back from the fixture). Tests compare the
//! production output against these lines.
//!
//! Line format: `<name> <len> <fnv64 hex> [samples...]`; `#` starts a
//! comment. The hash is FNV-1a over sign-extended words: `h ^= word as i64; h *= 0x100000001b3`.

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Sample values kept per array.
pub const SAMPLES: usize = 16;

/// Arrays up to this length keep every value.
pub const KEEP_ALL: usize = 256;

/// FNV-1a 64 over sign-extended words.
#[must_use]
pub fn fnv(words: &[i32]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for &w in words {
        h ^= i64::from(w) as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Indices of the sample values of an array of `len` words.
fn sample_indices(len: usize) -> Vec<usize> {
    if len <= KEEP_ALL {
        return (0..len).collect();
    }
    // Offset off power-of-two strides so table samples are not all
    // aligned on the same packed field value.
    (0..SAMPLES)
        .map(|i| (i * len / SAMPLES + 37 * i + 11) % len)
        .collect()
}

/// `fixtures/recorded-goldens/<file>`.
#[must_use]
pub fn path(file: &str) -> PathBuf {
    rs910_core::test_support::client_dir()
        .join("fixtures/recorded-goldens")
        .join(file)
}

/// A parsed golden file.
pub struct Golden {
    file: String,
    entries: BTreeMap<String, (usize, u64, Vec<i32>)>,
}

impl Golden {
    #[track_caller]
    pub fn load(file: &str) -> Self {
        let p = path(file);
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let mut entries = BTreeMap::new();
        for raw in text.lines() {
            let l = raw.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let mut it = l.split_ascii_whitespace();
            let name = it.next().unwrap().to_owned();
            let len: usize = it.next().and_then(|v| v.parse().ok()).expect(raw);
            let hash = u64::from_str_radix(it.next().expect(raw), 16).expect(raw);
            let samples = it.map(|v| v.parse().expect(raw)).collect();
            assert!(
                entries.insert(name.clone(), (len, hash, samples)).is_none(),
                "{file}: duplicate golden {name}"
            );
        }
        Self {
            file: file.to_owned(),
            entries,
        }
    }

    /// The committed sample values (every value for short arrays).
    #[track_caller]
    pub fn values(&self, name: &str) -> &[i32] {
        let (len, _, samples) = self.entry(name);
        assert_eq!(
            *len,
            samples.len(),
            "{}: {name} keeps only samples",
            self.file
        );
        samples
    }

    #[track_caller]
    fn entry(&self, name: &str) -> &(usize, u64, Vec<i32>) {
        self.entries
            .get(name)
            .unwrap_or_else(|| panic!("{}: no golden {name}", self.file))
    }

    /// Assert `words` is the recorded array `name`; the message names the first
    /// differing sample when one differs.
    #[track_caller]
    pub fn check(&self, name: &str, words: &[i32]) {
        if let Err(e) = self.compare(name, words) {
            panic!("{e}");
        }
    }

    /// [`Self::check`] as a `Result`, for tests that report several names.
    pub fn compare(&self, name: &str, words: &[i32]) -> Result<(), String> {
        let (len, hash, samples) = self.entry(name);
        let idx = sample_indices(*len);
        let first_sample = idx
            .iter()
            .zip(samples)
            .find(|(&i, &v)| words.get(i) != Some(&v))
            .map(|(&i, &v)| {
                format!(
                    "; first differing sample [{i}]: recorded {v}, Rust {:?}",
                    words.get(i)
                )
            })
            .unwrap_or_default();
        if words.len() != *len {
            return Err(format!(
                "{}: {name}: length recorded {len}, Rust {}{first_sample}",
                self.file,
                words.len()
            ));
        }
        let got = fnv(words);
        if got != *hash {
            return Err(format!(
                "{}: {name}: hash recorded {hash:016x}, Rust {got:016x}{first_sample}",
                self.file
            ));
        }
        Ok(())
    }
}
