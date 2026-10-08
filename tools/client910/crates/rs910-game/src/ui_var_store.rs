//! Lifetime-aware local files and acknowledged server-permanent batches.
use crate::{
    client_vars::{ClientVars, Value},
    protocol910::{script_types::script_type, varbits::Binding},
};
use anyhow::{Context, Result};
use native910::packet::Packet;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub fn encode(id: i32, value: &Value, out: &mut Vec<u8>) -> Result<()> {
    out.extend_from_slice(&(id as u16).to_be_bytes());
    match value {
        Value::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::String(s) => {
            anyhow::ensure!(!s.contains(&0), "NUL in persistent client string");
            out.push(0);
            // The CP-1252 encoding replaces each unsupported char with '?',
            // including each lone surrogate; never encode these as UTF-8.
            // (rs910-core's copy since Phase 2.1; the inline table search here
            // was identical on all 65536 units.)
            for &unit in s {
                out.push(rs910_core::cp1252::cp1252_encode_unit(unit));
            }
            out.push(0);
        }
        Value::Null => anyhow::bail!("null client variable cannot be persisted"),
    }
    Ok(())
}
pub fn decode(packet: &mut Packet<'_>, defs: &BTreeMap<i32, Binding>) -> Result<(i32, Value)> {
    let id = i32::from(packet.g2()?);
    let def = defs.get(&id).context("unknown persistent variable")?;
    let value = match def.data_type.and_then(script_type).map(|t| t.0) {
        Some(0) => Value::Int(packet.g4s()?),
        Some(1) => Value::Long(packet.g8s()?),
        Some(2) => Value::String(packet.gjstr2()?.encode_utf16().collect()),
        _ => anyhow::bail!("unsupported persistent variable type for {id}"),
    };
    Ok((id, value))
}
pub fn restore_server(
    client: &mut ClientVars,
    defs: &BTreeMap<i32, Binding>,
    bytes: &[u8],
) -> Result<()> {
    let mut packet = Packet::new(bytes);
    while packet.remaining() > 0 {
        let (id, value) = decode(&mut packet, defs)?;
        // The login writes directly, without dirtying the domain.
        client.values.insert(id, value);
    }
    Ok(())
}

#[derive(Default)]
pub struct Persistence {
    path: Option<PathBuf>,
    permanent: BTreeSet<i32>,
    last_local_save: i64,
    next_server_flush: i64,
    batch: Option<Vec<(i32, Value)>>,
    cursor: usize,
}
impl Persistence {
    pub fn install(
        &mut self,
        path: PathBuf,
        client: &mut ClientVars,
        defs: &BTreeMap<i32, Binding>,
    ) -> Result<()> {
        self.permanent = defs
            .iter()
            .filter(|(_, d)| d.lifetime == Some(1))
            .map(|(&id, _)| id)
            .collect();
        self.path = Some(path.clone());
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let mut p = Packet::new(&bytes);
        if p.remaining() < 3 || p.g1()? > 1 {
            return Ok(());
        }
        let count = p.g2()?;
        if p.remaining() < usize::from(count) * 6 {
            return Ok(());
        }
        for _ in 0..count {
            let (id, value) = decode(&mut p, defs)?;
            if self.permanent.contains(&id) {
                client.values.insert(id, value);
            }
        }
        Ok(())
    }
    pub fn save_local(&mut self, client: &mut ClientVars, now: i64, force: bool) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !client.permanent_dirty || (!force && self.last_local_save >= now - 60_000) {
            return Ok(());
        }
        let entries: Vec<_> = client
            .values
            .iter()
            .filter(|(id, _)| self.permanent.contains(id))
            .collect();
        let mut bytes = vec![1];
        bytes.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        for (&id, value) in entries {
            encode(id, value, &mut bytes)?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Same version-1 contents; the rename avoids a torn local file.
        let tmp = path.with_extension("dat.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, path)?;
        self.last_local_save = now;
        client.permanent_dirty = false;
        Ok(())
    }
    pub fn flush(
        &mut self,
        client: &mut ClientVars,
        now: i64,
        write_pos: usize,
    ) -> Result<Vec<u8>> {
        if now < self.next_server_flush {
            return Ok(vec![]);
        }
        if self.batch.is_none() {
            if !client.server_dirty {
                return Ok(vec![]);
            }
            self.batch = Some(
                client
                    .dirty_ids
                    .iter()
                    .map(|id| {
                        Ok((
                            *id,
                            client
                                .values
                                .get(id)
                                .context("dirty client variable missing")?
                                .clone(),
                        ))
                    })
                    .collect::<Result<_>>()?,
            );
            self.cursor = 0;
            client.server_dirty = false;
            client.dirty_ids.clear();
        }
        let batch = self.batch.as_ref().unwrap();
        if self.cursor >= batch.len() || write_pos > 1200 {
            return Ok(vec![]);
        }
        let mut out = vec![crate::proto::client::STORE_SERVERPERM_VARCS, 0, 0, 0];
        while self.cursor < batch.len() {
            let (id, value) = &batch[self.cursor];
            let mut bytes = vec![];
            encode(*id, value, &mut bytes)?;
            if out.len() + write_pos + bytes.len() > 1500 {
                break;
            }
            out.extend(bytes);
            self.cursor += 1;
        }
        let len = (out.len() - 3) as u16;
        out[1..3].copy_from_slice(&len.to_be_bytes());
        out[3] = u8::from(self.cursor >= batch.len());
        self.next_server_flush = now + 1000;
        Ok(out)
    }
    pub fn acknowledge(&mut self) {
        if self
            .batch
            .as_ref()
            .is_some_and(|batch| self.cursor >= batch.len())
        {
            self.batch = None;
            self.cursor = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn def(id: i32, kind: u8, lifetime: u8) -> Binding {
        let mut d = Binding::empty(2, id);
        d.data_type = Some(kind);
        d.lifetime = Some(lifetime);
        d
    }
    #[test]
    fn typed_values_and_surrogate_replacement() {
        let mut bytes = vec![];
        encode(258, &Value::Int(-123), &mut bytes).unwrap();
        encode(3, &Value::Long(0x0102030405060708), &mut bytes).unwrap();
        encode(
            4,
            &Value::String(vec![65, 0x20ac, 0xd800, 0xdfff]),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(
            bytes,
            vec![
                1, 2, 255, 255, 255, 133, 0, 3, 1, 2, 3, 4, 5, 6, 7, 8, 0, 4, 0, 65, 128, 63, 63, 0
            ]
        );
        let defs = BTreeMap::from([
            (258, def(258, 0, 2)),
            (3, def(3, 35, 2)),
            (4, def(4, 36, 2)),
        ]);
        // Cache type 36 is STRING; this also exercises cache-directed decoding.
        let mut c = ClientVars::default();
        restore_server(&mut c, &defs, &bytes).unwrap();
        assert_eq!(c.values[&258], Value::Int(-123));
        assert_eq!(c.values[&3], Value::Long(0x0102030405060708));
        assert_eq!(c.values[&4], Value::String("A€??".encode_utf16().collect()));
        assert!(!c.server_dirty);
        assert!(c.dirty_ids.is_empty());
        // VarC 2868 (camera bindings, varbits 18958-18961) reset default
        // 842019105 = 0x32303121 = W/S/A/D client key codes [33,49,48,50]
        // (server Settings.integration.test.ts): u16 id + i32 BE.
        let mut bytes = vec![];
        encode(2868, &Value::Int(842019105), &mut bytes).unwrap();
        assert_eq!(bytes, vec![0x0b, 0x34, 0x32, 0x30, 0x31, 0x21]);
        let defs = BTreeMap::from([(2868, def(2868, 0, 2))]);
        assert_eq!(
            decode(&mut Packet::new(&bytes), &defs).unwrap(),
            (2868, Value::Int(842019105))
        );
    }
    #[test]
    fn batches_wait_for_ack_and_retain_edits_during_flush() {
        let mut c = ClientVars::default();
        for id in 0..400 {
            c.set(&def(id, 0, 2), Value::Int(id + 1)).unwrap();
        }
        let mut p = Persistence::default();
        assert!(p.flush(&mut c, 0, 1201).unwrap().is_empty());
        let first = p.flush(&mut c, 0, 0).unwrap();
        assert!(first.len() <= 1500);
        assert_eq!(first[3], 0);
        p.acknowledge(); // Early acknowledgements cannot release a partial batch.
        c.set(&def(0, 0, 2), Value::Int(999)).unwrap();
        assert!(p.flush(&mut c, 999, 0).unwrap().is_empty());
        let last = p.flush(&mut c, 1000, 0).unwrap();
        assert_eq!(last[3], 1);
        assert!(p.flush(&mut c, 2000, 0).unwrap().is_empty());
        p.acknowledge();
        let edit = p.flush(&mut c, 2000, 0).unwrap();
        assert_eq!(edit, vec![71, 0, 7, 1, 0, 0, 0, 0, 3, 231]);
    }
    #[test]
    fn local_file_preserves_only_permanent_values() {
        let path = std::env::temp_dir().join(format!("alto-varcs-{}.dat", std::process::id()));
        let defs = BTreeMap::from([(1, def(1, 0, 1)), (2, def(2, 0, 2)), (3, def(3, 0, 0))]);
        let mut c = ClientVars::default();
        let mut p = Persistence::default();
        p.install(path.clone(), &mut c, &defs).unwrap();
        for id in 1..=3 {
            c.set(&defs[&id], Value::Int(id * 10)).unwrap();
        }
        p.save_local(&mut c, 60_001, false).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            vec![1, 0, 1, 0, 1, 0, 0, 0, 10]
        );
        let mut restored = ClientVars::default();
        Persistence::default()
            .install(path.clone(), &mut restored, &defs)
            .unwrap();
        assert_eq!(restored.values, BTreeMap::from([(1, Value::Int(10))]));
        std::fs::remove_file(path).unwrap();
    }
}
