//! Versioned E0 decision trace: named i32/f32-bit sections, little endian.
//! No tolerance, sorting or entity-key deduplication is applied by the diff.
use std::collections::HashSet;

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Trace(pub Vec<(String, Vec<i32>)>);

#[allow(
    dead_code,
    reason = "shared with bin/drawdiff.rs: the client uses the encoder, drawdiff the decoder"
)]
impl Trace {
    pub fn push(&mut self, name: impl Into<String>, words: Vec<i32>) {
        self.0.push((name.into(), words));
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"DRW1".to_vec();
        let word = |out: &mut Vec<u8>, v: usize| out.extend_from_slice(&(v as u32).to_le_bytes());
        word(&mut out, self.0.len());
        for (name, values) in &self.0 {
            word(&mut out, name.len());
            out.extend_from_slice(name.as_bytes());
            word(&mut out, values.len());
            for v in values {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        struct Cursor<'a>(&'a [u8]);
        impl<'a> Cursor<'a> {
            fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
                if n > self.0.len() {
                    return Err("truncated trace".into());
                }
                let (head, tail) = self.0.split_at(n);
                self.0 = tail;
                Ok(head)
            }
            fn word(&mut self) -> Result<u32, String> {
                Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
            }
        }
        let mut c = Cursor(bytes);
        if c.take(4)? != b"DRW1" {
            return Err("unsupported trace version".into());
        }
        let count = c.word()? as usize;
        if count > c.0.len() / 8 {
            return Err("invalid section count".into());
        }
        let mut out = Self::default();
        let mut seen = HashSet::new();
        for _ in 0..count {
            let len = c.word()? as usize;
            let name = std::str::from_utf8(c.take(len)?)
                .map_err(|e| e.to_string())?
                .to_owned();
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate section {name}"));
            }
            let count = c.word()? as usize;
            let len = count.checked_mul(4).ok_or("section length overflow")?;
            let data = c.take(len)?;
            let words = data
                .chunks_exact(4)
                .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            out.push(name, words);
        }
        if !c.0.is_empty() {
            return Err("trailing trace bytes".into());
        }
        if out.0.is_empty() {
            return Err("empty trace".into());
        }
        Ok(out)
    }
    pub fn compare(&self, other: &Self) -> Result<(), String> {
        for ((name, a), (other_name, b)) in self.0.iter().zip(&other.0) {
            if name != other_name {
                return Err(format!(
                    "section order: reference {name}, Rust {other_name}"
                ));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if x != y {
                    return Err(format!(
                        "{name} word {i}: reference {x} ({x:#010x}), Rust {y} ({y:#010x})"
                    ));
                }
            }
            if a.len() != b.len() {
                return Err(format!(
                    "{name} length: reference {}, Rust {}",
                    a.len(),
                    b.len()
                ));
            }
        }
        if self.0.len() != other.0.len() {
            return Err(format!(
                "section count: reference {}, Rust {}",
                self.0.len(),
                other.0.len()
            ));
        }
        Ok(())
    }
}
