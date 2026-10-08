//! Test helpers shared by the client crates' unit tests (moved from
//! client910's `test_support`, Phase 2.4). Test-only: the module is
//! `#[cfg(any(test, feature = "test-hooks"))]`, and client910 and the
//! rs910-* crates enable `test-hooks` through `[dev-dependencies]` only.

use std::path::{Path, PathBuf};

/// Environment variable that names the repository root explicitly.
pub const REPO_ROOT_ENV: &str = "ALTO_REPO_ROOT";
/// Environment variable that names the pack directory explicitly.
pub const PACK_DIR_ENV: &str = "ALTO_PACK_DIR";

/// Whether `dir` is the repository root: it holds both `tools/Cargo.toml`
/// and `server/`.
fn is_repo_root(dir: &Path) -> bool {
    dir.join("tools/Cargo.toml").is_file() && dir.join("server").is_dir()
}

/// The repository root: `override_root` when given, else the first directory
/// at or above `start` that [`is_repo_root`], else `fallback`.
pub fn resolve_repo_root(override_root: Option<&Path>, start: &Path, fallback: &Path) -> PathBuf {
    if let Some(root) = override_root {
        return root.to_path_buf();
    }
    start
        .ancestors()
        .find(|dir| is_repo_root(dir))
        .unwrap_or(fallback)
        .to_path_buf()
}

/// The repository root of the checkout (or git worktree) being tested.
///
/// Resolved when the test runs, never baked in at compile time: two worktrees
/// that share one cargo target directory can reuse an unchanged crate build,
/// and a compile-time path would then name the other worktree. Order:
/// `ALTO_REPO_ROOT`; the first directory at or above the current directory
/// holding `tools/Cargo.toml` and `server/` (cargo runs tests with the
/// current directory at the package root); this crate's own checkout.
pub fn repo_root() -> PathBuf {
    let override_root = std::env::var_os(REPO_ROOT_ENV).map(PathBuf::from);
    let start = std::env::current_dir().unwrap_or_default();
    let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    resolve_repo_root(override_root.as_deref(), &start, &built_in)
}

/// `server/data/pack`: `ALTO_PACK_DIR` when set, else under [`repo_root`].
pub fn pack_root() -> PathBuf {
    std::env::var_os(PACK_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("server/data/pack"))
}

/// `tools/client910`.
pub fn client_dir() -> PathBuf {
    repo_root().join("tools/client910")
}

/// `tools/client910/fixtures`.
pub fn client_fixtures() -> PathBuf {
    client_dir().join("fixtures")
}

/// Frozen recordings of the original client's observable behaviour.
///
/// A behaviour oracle was run once against the original client; what it
/// observed (packet bytes, component state, stacks, values) is committed under
/// `tools/client910/fixtures/recorded/<group>/`, and the tests here compare
/// the port's own output stream with it. No JDK and no original client
/// sources are needed to run them.
///
/// A recording of at most [`RAW_LIMIT`] bytes is stored as `<name>.bin`. A
/// larger one is stored as `<name>.blk`: a header line `len=<bytes>
/// block=<bytes>` followed by one FNV-1a 64 digest (16 hex digits) per block.
/// A mismatch names the byte offset of the first differing block.
pub mod frozen {
    use super::{Path, PathBuf};

    /// Recordings up to this size are committed verbatim.
    pub const RAW_LIMIT: usize = 256 * 1024;

    /// Digest block size for a recording of `len` bytes: 16 KiB, doubled until
    /// there are at most 4096 blocks.
    pub fn block_size(len: usize) -> usize {
        let mut block = 16 * 1024;
        while len.div_ceil(block) > 4096 {
            block *= 2;
        }
        block
    }

    /// `tools/client910/fixtures/recorded`.
    pub fn store() -> PathBuf {
        super::client_fixtures().join("recorded")
    }

    /// FNV-1a 64 over `bytes`.
    pub fn fnv64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    /// The `.blk` text of `bytes`.
    pub fn block_digest(bytes: &[u8]) -> String {
        let size = block_size(bytes.len());
        let mut text = format!("len={} block={size}\n", bytes.len());
        for block in bytes.chunks(size) {
            text.push_str(&format!("{:016x}\n", fnv64(block)));
        }
        text
    }

    /// The committed text file `name` (for example `bloom/state.txt`).
    #[track_caller]
    pub fn text(name: &str) -> String {
        let path = store().join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("frozen fixture {}: {e}", path.display()))
    }

    /// The committed binary file `name` (verbatim).
    #[track_caller]
    pub fn bytes(name: &str) -> Vec<u8> {
        let path = store().join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("frozen fixture {}: {e}", path.display()))
    }

    /// Asserts that `actual` is the frozen recording `name` (no extension: the
    /// stored form is `<name>.bin` or `<name>.blk`).
    #[track_caller]
    pub fn assert_stream(name: &str, actual: &[u8]) {
        let raw = store().join(format!("{name}.bin"));
        if raw.is_file() {
            let frozen = std::fs::read(&raw).unwrap();
            if frozen != actual {
                let at = frozen
                    .iter()
                    .zip(actual)
                    .position(|(a, b)| a != b)
                    .unwrap_or(frozen.len().min(actual.len()));
                panic!(
                    "{name}: differs from the frozen recording at byte {at} (frozen {} bytes, actual {} bytes)",
                    frozen.len(),
                    actual.len()
                );
            }
            return;
        }
        let digest = store().join(format!("{name}.blk"));
        let frozen = std::fs::read_to_string(&digest).unwrap_or_else(|_| {
            panic!(
                "no frozen recording {name}.bin or {name}.blk in {}",
                store().display()
            )
        });
        let actual_text = block_digest(actual);
        if frozen != actual_text {
            let mut f = frozen.lines();
            let mut a = actual_text.lines();
            let (fh, ah) = (f.next().unwrap_or(""), a.next().unwrap_or(""));
            if fh != ah {
                panic!("{name}: frozen `{fh}` but actual `{ah}`");
            }
            let block = f.zip(a).position(|(x, y)| x != y).unwrap_or(0);
            let size = fh
                .rsplit_once("block=")
                .and_then(|(_, n)| n.parse::<usize>().ok())
                .unwrap_or(0);
            panic!(
                "{name}: differs from the frozen digest in block {block} (byte {})",
                block * size
            );
        }
    }

    /// A scratch directory for tests whose oracle-style body writes named
    /// output files; [`Scratch::finish`] then compares chosen files with the
    /// frozen recordings.
    pub struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        /// A fresh per-process directory named after `group`.
        pub fn new(group: &str) -> Self {
            let dir = std::env::temp_dir()
                .join("alto-frozen")
                .join(std::process::id().to_string())
                .join(group);
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        /// The directory.
        pub fn dir(&self) -> &Path {
            &self.dir
        }

        /// `(written file, frozen name)` pairs: each file must equal its
        /// frozen recording, `<group>/<frozen name>`.
        #[track_caller]
        pub fn finish(&self, group: &str, pairs: &[(&str, &str)]) {
            for (file, name) in pairs {
                let bytes = std::fs::read(self.dir.join(file))
                    .unwrap_or_else(|e| panic!("{file} was not written: {e}"));
                assert_stream(&format!("{group}/{name}"), &bytes);
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory tree that is removed when dropped.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join("alto-repo-root-test")
                .join(format!("{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn repo_root_is_found_from_a_nested_directory_and_the_override_wins() {
        let tree = Tree::new("walk");
        let repo = tree.0.join("checkout");
        std::fs::create_dir_all(repo.join("tools/client910/crates/rs910-core")).unwrap();
        std::fs::create_dir_all(repo.join("server")).unwrap();
        std::fs::write(repo.join("tools/Cargo.toml"), "").unwrap();
        let fallback = tree.0.join("fallback");

        let nested = repo.join("tools/client910/crates/rs910-core");
        assert_eq!(resolve_repo_root(None, &nested, &fallback), repo);
        assert_eq!(resolve_repo_root(None, &repo, &fallback), repo);

        let explicit = tree.0.join("explicit");
        assert_eq!(
            resolve_repo_root(Some(&explicit), &nested, &fallback),
            explicit
        );

        let outside = tree.0.join("elsewhere/deeper");
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(resolve_repo_root(None, &outside, &fallback), fallback);
    }
}
