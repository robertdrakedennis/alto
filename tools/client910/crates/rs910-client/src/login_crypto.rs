//! The cryptography of a login: the public key the RSA block is encrypted
//! with, the session seeds each login draws, and the sealing of a plain login
//! block (RSA block, tiny cipher over the rest).
//!
//! The key is configuration, never built in ([`LoginKeySource`]). A login
//! built with [`LoginCrypto::Plain`] sends its blocks unencrypted with zero
//! seeds and masks no opcodes: for tests and recorded replays against
//! servers that run without encryption, never for a real server.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{bail, ensure, Context, Result};
use rs910_protocol::rsa_block::RsaPublicKey;
use rs910_protocol::tiny_cipher;
use rs910_protocol::wire_cipher::SessionSeeds;

/// Where a client finds the login RSA public key: a command-line value or
/// environment variable wins over a key file; a key file path comes from the
/// command line, the environment or the default locations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoginKeySource {
    pub modulus: Option<String>,
    pub exponent: Option<String>,
    pub key_file: Option<PathBuf>,
    /// The cache pack root: the default key file is `keys/login-rsa.pub` next
    /// to it (where the development server writes it).
    pub pack_root: Option<PathBuf>,
}

/// The environment variables that override the command line.
pub const ENV_MODULUS: &str = "ALTO_RSA_MODULUS";
pub const ENV_EXPONENT: &str = "ALTO_RSA_EXPONENT";
pub const ENV_KEY_FILE: &str = "ALTO_RSA_KEY_FILE";
/// Where the development server writes its public key, relative to the
/// repository root, when no pack root says otherwise.
pub const DEFAULT_KEY_FILE: &str = "server/data/keys/login-rsa.pub";
/// The key file's name beside the pack root's `keys` directory.
const KEY_FILE_NAME: &str = "login-rsa.pub";

impl LoginKeySource {
    /// Resolve the key: environment overrides first, then the command-line
    /// values, then the key file (environment, command line, default path).
    pub fn resolve(&self) -> Result<RsaPublicKey> {
        self.resolve_with(|name| std::env::var(name).ok())
    }

    /// [`LoginKeySource::resolve`] with an explicit environment.
    pub fn resolve_with(&self, env: impl Fn(&str) -> Option<String>) -> Result<RsaPublicKey> {
        let modulus = env(ENV_MODULUS).or_else(|| self.modulus.clone());
        let exponent = env(ENV_EXPONENT).or_else(|| self.exponent.clone());
        match (modulus, exponent) {
            (Some(modulus), Some(exponent)) => {
                return RsaPublicKey::from_hex(&modulus, &exponent)
                    .context("the configured login RSA key is invalid");
            }
            (None, None) => {}
            _ => bail!("the login RSA modulus and exponent must be given together"),
        }
        if let Some(path) = env(ENV_KEY_FILE)
            .map(PathBuf::from)
            .or_else(|| self.key_file.clone())
        {
            return read_key_file(&path, std::slice::from_ref(&path));
        }
        // The default locations: beside the pack root, then the repository's.
        let mut searched = Vec::new();
        if let Some(data) = self.pack_root.as_ref().and_then(|pack| pack.parent()) {
            searched.push(data.join("keys").join(KEY_FILE_NAME));
        }
        searched.push(PathBuf::from(DEFAULT_KEY_FILE));
        let found = searched
            .iter()
            .find(|path| path.is_file())
            .unwrap_or(&searched[0]);
        read_key_file(found, &searched)
    }
}

fn read_key_file(path: &Path, searched: &[PathBuf]) -> Result<RsaPublicKey> {
    let places = searched
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "no login RSA key: cannot read {places} (start the development server once, or run \
             `npm --prefix server run keys:generate`, or pass --rsa-modulus and --rsa-exponent, \
             --rsa-key-file, or --login-crypto off for a server without encryption)"
        )
    })?;
    RsaPublicKey::from_config_text(&text)
        .with_context(|| format!("the login RSA key file {} is invalid", path.display()))
}

/// How a login's blocks are protected.
#[derive(Clone, Debug, Default)]
pub enum LoginCrypto {
    /// No encryption: tests and recorded replays.
    #[default]
    Plain,
    /// RSA block, tiny cipher and opcode masking with the configured key.
    Rsa(Arc<LoginSecurity>),
    /// The key could not be found or read; a login fails with this message.
    Unavailable(Arc<str>),
}

impl PartialEq for LoginCrypto {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Plain, Self::Plain) => true,
            (Self::Rsa(a), Self::Rsa(b)) => Arc::ptr_eq(a, b),
            (Self::Unavailable(a), Self::Unavailable(b)) => a == b,
            _ => false,
        }
    }
}
impl Eq for LoginCrypto {}

impl LoginCrypto {
    /// Encryption with the key of `source`; a missing or invalid key is
    /// remembered as [`LoginCrypto::Unavailable`] and reported by the login.
    #[must_use]
    pub fn from_source(source: &LoginKeySource) -> Self {
        match source.resolve() {
            Ok(key) => Self::Rsa(Arc::new(LoginSecurity::new(key))),
            Err(error) => {
                let reason = format!("{error:#}");
                log::warn!("[client910] logins will fail: {reason}");
                Self::Unavailable(reason.into())
            }
        }
    }

    /// The key and seeds for one login block, `None` when plain; an
    /// unavailable key is an error.
    pub fn begin(&self) -> Result<Option<(&LoginSecurity, LoginSeeds)>> {
        Ok(self
            .security()?
            .map(|security| (security, security.draw_seeds())))
    }

    /// The key and seed state, `None` when plain; an unavailable key is an
    /// error.
    pub fn security(&self) -> Result<Option<&LoginSecurity>> {
        match self {
            Self::Plain => Ok(None),
            Self::Rsa(security) => Ok(Some(security)),
            Self::Unavailable(reason) => bail!("{reason}"),
        }
    }
}

/// The seeds of a login block: this connection's, and (for a reconnect) the
/// previous connection's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginSeeds {
    pub current: SessionSeeds,
    pub previous: SessionSeeds,
}

impl LoginSeeds {
    /// The block of a plain login: zero seeds.
    pub const ZERO: Self = Self {
        current: [0; 4],
        previous: [0; 4],
    };
}

/// Where seeds come from.
#[derive(Debug)]
enum SeedSource {
    /// The operating system's randomness.
    System,
    /// A fixed sequence, for tests; exhausting it is a bug of the test.
    Scripted(VecDeque<i32>),
}

/// The login key plus the process-wide seed state.
#[derive(Debug)]
pub struct LoginSecurity {
    key: RsaPublicKey,
    source: Mutex<SeedSource>,
    /// The seeds of the previous login block built, which a reconnect repeats.
    previous: Mutex<SessionSeeds>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl LoginSecurity {
    /// Seeds from the operating system.
    #[must_use]
    pub fn new(key: RsaPublicKey) -> Self {
        Self {
            key,
            source: Mutex::new(SeedSource::System),
            previous: Mutex::new([0; 4]),
        }
    }

    /// Seeds from a fixed sequence (tests): four per login block, one per
    /// account-creation filler word.
    #[must_use]
    pub fn scripted(key: RsaPublicKey, values: impl IntoIterator<Item = i32>) -> Self {
        Self {
            key,
            source: Mutex::new(SeedSource::Scripted(values.into_iter().collect())),
            previous: Mutex::new([0; 4]),
        }
    }

    /// Set the seeds the next reconnect repeats (a test of a reconnect that
    /// follows a login this process did not make).
    pub fn set_previous_seeds(&self, seeds: SessionSeeds) {
        *lock(&self.previous) = seeds;
    }

    fn next_word(&self) -> i32 {
        match &mut *lock(&self.source) {
            SeedSource::System => {
                let mut bytes = [0u8; 4];
                getrandom::fill(&mut bytes).expect("the operating system provides randomness");
                i32::from_be_bytes(bytes)
            }
            SeedSource::Scripted(values) => values.pop_front().expect("scripted seeds ran out"),
        }
    }

    /// Four fresh seeds for a login block; the block built before becomes
    /// the "previous" one a reconnect repeats.
    pub fn draw_seeds(&self) -> LoginSeeds {
        let current = [
            self.next_word(),
            self.next_word(),
            self.next_word(),
            self.next_word(),
        ];
        let mut previous = lock(&self.previous);
        let seeds = LoginSeeds {
            current,
            previous: *previous,
        };
        *previous = current;
        seeds
    }

    /// A non-negative word below 99999999 (the account-creation block's seeds
    /// and filler are drawn from this range).
    pub fn account_word(&self) -> i32 {
        let raw = self.next_word() as u32;
        // A fraction of the unit interval times 99999999, as the block's
        // words are made.
        let fraction = f64::from(raw) / 4_294_967_296.0;
        (fraction * 9.999_999_9e7) as i32
    }

    /// Four account-creation seeds (not tied to a reconnect).
    pub fn draw_account_seeds(&self) -> SessionSeeds {
        [
            self.account_word(),
            self.account_word(),
            self.account_word(),
            self.account_word(),
        ]
    }

    /// Seal a plain login body: `rsa` is the part to encrypt (replaced by the
    /// `u16` length and the ciphertext), `tiny_from` the offset in `plain`
    /// from which the rest is protected with the tiny cipher keyed by
    /// `seeds`.
    pub fn seal(
        &self,
        plain: &[u8],
        rsa: Option<Range<usize>>,
        tiny_from: Option<usize>,
        seeds: SessionSeeds,
    ) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(plain.len() + 8);
        let tiny_start = match rsa {
            Some(rsa) => {
                ensure!(rsa.end <= plain.len(), "the RSA region is outside the body");
                out.extend_from_slice(&plain[..rsa.start]);
                out.extend_from_slice(&self.key.encrypt_block(&plain[rsa.clone()])?);
                let after_block = out.len();
                out.extend_from_slice(&plain[rsa.end..]);
                match tiny_from {
                    Some(from) => {
                        ensure!(
                            from >= rsa.end,
                            "the tiny-cipher region overlaps the RSA block"
                        );
                        Some(after_block + (from - rsa.end))
                    }
                    None => None,
                }
            }
            None => {
                out.extend_from_slice(plain);
                tiny_from
            }
        };
        if let Some(start) = tiny_start {
            let end = out.len();
            tiny_cipher::encrypt_range(&mut out, start, end, &seeds);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> RsaPublicKey {
        let recorded = include_str!("../../../fixtures/recorded/login-crypto/primitives.txt");
        let modulus = recorded
            .lines()
            .find_map(|l| l.strip_prefix("rsa-modulus "))
            .unwrap();
        RsaPublicKey::from_hex(modulus, "10001").unwrap()
    }

    #[test]
    fn seeds_chain_into_the_previous_block() {
        let security = LoginSecurity::scripted(test_key(), 1..=8);
        let first = security.draw_seeds();
        assert_eq!(first.current, [1, 2, 3, 4]);
        assert_eq!(first.previous, [0; 4]);
        let second = security.draw_seeds();
        assert_eq!(second.current, [5, 6, 7, 8]);
        assert_eq!(second.previous, [1, 2, 3, 4]);
    }

    #[test]
    fn account_words_stay_in_their_range() {
        let security = LoginSecurity::scripted(test_key(), [0, i32::MAX, -1, i32::MIN]);
        for _ in 0..4 {
            let word = security.account_word();
            assert!((0..99_999_999).contains(&word), "{word}");
        }
    }

    #[test]
    fn key_resolution_prefers_the_environment_and_demands_a_pair() {
        let source = LoginKeySource {
            modulus: Some("c0cde05fe3711b64e0df6895216ed8cb".into()),
            exponent: Some("10001".into()),
            key_file: None,
            pack_root: None,
        };
        assert_eq!(source.resolve_with(|_| None).unwrap().bits(), 128);
        let env = |name: &str| match name {
            ENV_MODULUS => Some("c0cde05fe3711b64e0df6895216ed8cb4b1d6368567865c7".to_owned()),
            ENV_EXPONENT => Some("3".to_owned()),
            _ => None,
        };
        assert_eq!(source.resolve_with(env).unwrap().bits(), 192);
        let half = LoginKeySource {
            modulus: Some("c0cde05fe3711b64e0df6895216ed8cb".into()),
            ..LoginKeySource::default()
        };
        assert!(half.resolve_with(|_| None).is_err());
        let missing = LoginKeySource {
            key_file: Some("/nonexistent/alto-key".into()),
            ..LoginKeySource::default()
        };
        let error = format!("{:#}", missing.resolve_with(|_| None).unwrap_err());
        assert!(error.contains("no login RSA key"), "{error}");
    }

    /// With no key option the key file is the one beside the pack root's
    /// directory, where the development server writes it.
    #[test]
    fn the_default_key_file_sits_beside_the_pack_root() {
        let dir = std::env::temp_dir().join(format!("alto-login-key-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("data/keys")).unwrap();
        std::fs::write(
            dir.join("data/keys").join(KEY_FILE_NAME),
            "modulus=c0cde05fe3711b64e0df6895216ed8cb\nexponent=10001\n",
        )
        .unwrap();
        let source = LoginKeySource {
            pack_root: Some(dir.join("data/pack")),
            ..LoginKeySource::default()
        };
        let key = source.resolve_with(|_| None);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(key.unwrap().bits(), 128);
    }
}
