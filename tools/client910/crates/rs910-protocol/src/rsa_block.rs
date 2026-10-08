//! The RSA public-key step of a login: one block, one modular exponentiation,
//! no padding. The key is configuration; none is built in.
//!
//! The block is read as a big-endian two's-complement integer, raised to the
//! public exponent modulo the key's modulus, and written as the minimal
//! two's-complement big-endian bytes of the result, behind a big-endian
//! `u16` byte count. A result whose top bit is set therefore gains a leading
//! zero byte and one with leading zero bytes loses them, so the encrypted
//! block's length varies by a byte or so.

use anyhow::{bail, ensure, Context, Result};
use num_bigint::BigUint;

/// An RSA public key: the modulus and the public exponent.
#[derive(Clone, PartialEq, Eq)]
pub struct RsaPublicKey {
    modulus: BigUint,
    exponent: BigUint,
}

impl std::fmt::Debug for RsaPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RsaPublicKey({} bits)", self.modulus.bits())
    }
}

fn parse_hex(what: &str, text: &str) -> Result<BigUint> {
    let digits = text.trim();
    let digits = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
        .unwrap_or(digits);
    ensure!(!digits.is_empty(), "the RSA {what} is empty");
    BigUint::parse_bytes(digits.as_bytes(), 16)
        .with_context(|| format!("the RSA {what} is not hexadecimal"))
}

impl RsaPublicKey {
    /// A key from big-endian hexadecimal modulus and exponent text.
    pub fn from_hex(modulus: &str, exponent: &str) -> Result<Self> {
        let modulus = parse_hex("modulus", modulus)?;
        let exponent = parse_hex("exponent", exponent)?;
        ensure!(modulus.bits() >= 64, "the RSA modulus is too small");
        ensure!(
            exponent.bits() > 0 && exponent.bits() <= modulus.bits(),
            "the RSA exponent is invalid"
        );
        Ok(Self { modulus, exponent })
    }

    /// A key from a key file's text: lines `modulus=<hex>` and
    /// `exponent=<hex>`; blank lines and lines starting with `#` are ignored.
    pub fn from_config_text(text: &str) -> Result<Self> {
        let mut modulus = None;
        let mut exponent = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                bail!("RSA key line without `=`: {line:?}");
            };
            match name.trim() {
                "modulus" => modulus = Some(value.to_owned()),
                "exponent" => exponent = Some(value.to_owned()),
                other => bail!("unknown RSA key field {other:?}"),
            }
        }
        Self::from_hex(
            &modulus.context("the RSA key file has no modulus")?,
            &exponent.context("the RSA key file has no exponent")?,
        )
    }

    /// The modulus size in bits.
    #[must_use]
    pub fn bits(&self) -> u64 {
        self.modulus.bits()
    }

    /// Encrypt `block`: the `u16` byte count followed by the result's bytes.
    pub fn encrypt_block(&self, block: &[u8]) -> Result<Vec<u8>> {
        ensure!(!block.is_empty(), "an empty RSA block");
        ensure!(
            block[0] < 0x80,
            "an RSA block must start with a byte below 0x80 (a positive integer)"
        );
        let value = BigUint::from_bytes_be(block);
        ensure!(
            value < self.modulus,
            "the RSA block is larger than the key's modulus"
        );
        let result = value.modpow(&self.exponent, &self.modulus);
        let mut bytes = result.to_bytes_be();
        if bytes.first().is_some_and(|&top| top >= 0x80) {
            bytes.insert(0, 0);
        }
        let mut out = Vec::with_capacity(2 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        out.extend_from_slice(&bytes);
        Ok(out)
    }
}

/// The private half of a test key, for the tests of whatever sits on the other
/// end of a login (a mock server that opens the block).
#[cfg(any(test, feature = "test-hooks"))]
pub mod test_support {
    use super::{BigUint, RsaPublicKey};

    /// A key pair read from the TEST-ONLY key file (never used outside tests).
    pub struct TestKeyPair {
        pub public: RsaPublicKey,
        private_exponent: BigUint,
    }

    fn field(text: &str, name: &str) -> BigUint {
        let marker = format!("\"{name}\": \"");
        let from = text
            .find(&marker)
            .unwrap_or_else(|| panic!("no {name} in the key file"))
            + marker.len();
        let to = from + text[from..].find('"').unwrap();
        BigUint::parse_bytes(&text.as_bytes()[from..to], 16).unwrap()
    }

    impl TestKeyPair {
        /// Parse the TEST-ONLY key file (`modulus`, `exponent`, `private_exponent`).
        #[must_use]
        pub fn from_file_text(text: &str) -> Self {
            let modulus = field(text, "modulus");
            let exponent = field(text, "exponent");
            Self {
                public: RsaPublicKey::from_hex(
                    &modulus.to_str_radix(16),
                    &exponent.to_str_radix(16),
                )
                .unwrap(),
                private_exponent: field(text, "private_exponent"),
            }
        }

        /// The block's plaintext bytes from its ciphertext (without the length
        /// prefix).
        #[must_use]
        pub fn decrypt(&self, cipher: &[u8]) -> Vec<u8> {
            BigUint::from_bytes_be(cipher)
                .modpow(&self.private_exponent, &self.public.modulus)
                .to_bytes_be()
        }

        /// Split a block that starts with its `u16` length into its ciphertext
        /// and what follows.
        #[must_use]
        pub fn split_block(wire: &[u8]) -> (&[u8], &[u8]) {
            let length = usize::from(u16::from_be_bytes([wire[0], wire[1]]));
            (&wire[2..2 + length], &wire[2 + length..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn test_key() -> RsaPublicKey {
        let recorded = include_str!("../../../fixtures/recorded/login-crypto/primitives.txt");
        let modulus = recorded
            .lines()
            .find_map(|l| l.strip_prefix("rsa-modulus "))
            .unwrap();
        RsaPublicKey::from_hex(modulus, "10001").unwrap()
    }

    /// Three blocks (with and without the sign byte on the result) encrypt
    /// to the bytes the original client's own block encryption produced.
    #[test]
    fn blocks_match_the_original_client() {
        let key = test_key();
        let recorded = include_str!("../../../fixtures/recorded/login-crypto/primitives.txt");
        let mut seen = 0;
        for line in recorded.lines().filter(|l| l.starts_with("rsa ")) {
            let fields: Vec<&str> = line.split(' ').collect();
            let sealed = key.encrypt_block(&unhex(fields[1])).unwrap();
            assert_eq!(sealed, unhex(fields[2]));
            seen += 1;
        }
        assert_eq!(seen, 3);
    }

    #[test]
    fn rejects_blocks_that_cannot_be_decrypted() {
        let key = test_key();
        assert!(key.encrypt_block(&[]).is_err());
        assert!(key.encrypt_block(&[0x80, 1, 2]).is_err());
        assert!(key.encrypt_block(&[0x7f; 200]).is_err());
    }

    #[test]
    fn key_file_text_is_read_and_checked() {
        let text = "# login key\nmodulus=0xc0cde05fe3711b64e0df6895216ed8cb\nexponent = 10001\n";
        assert_eq!(RsaPublicKey::from_config_text(text).unwrap().bits(), 128);
        assert!(RsaPublicKey::from_config_text("exponent=10001").is_err());
        assert!(RsaPublicKey::from_config_text("modulus=zz\nexponent=3").is_err());
        assert!(RsaPublicKey::from_config_text("modulus=ab\nexponent=3").is_err());
        assert!(RsaPublicKey::from_config_text("wat=1\nmodulus=ab").is_err());
    }
}
