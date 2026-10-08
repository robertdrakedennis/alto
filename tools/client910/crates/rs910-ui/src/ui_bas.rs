//! bas_getanim_ready, backed by body-animation-set group 32.
use crate::{cache::Pack, protocol910::bas_types::Bas, ui_runtime::Engine};
use anyhow::{Context, Result};
use native910::vm::{Value, VmError, VmResult};
use std::collections::BTreeMap;

pub fn load(pack: &Pack) -> Result<BTreeMap<i32, Bas>> {
    let raw = pack
        .read_group("config", 32)
        .context("BAS config group 32")?;
    let count = raw.keys().last().map_or(0, |v| v + 1);
    let mut out = BTreeMap::new();
    for id in 0..count {
        let bas = match raw.get(&id) {
            Some(bytes) => {
                Bas::decode(id as i32, bytes).map_err(|e| anyhow::anyhow!("BAS {id}: {e:?}"))?
            }
            None => Bas::default(),
        };
        out.insert(id as i32, bas);
    }
    Ok(out)
}

pub fn dispatch(
    engine: &Engine,
    command: &str,
    ints: &mut Vec<i32>,
) -> Option<VmResult<Option<Value>>> {
    if command != "bas_getanim_ready" {
        return None;
    }
    if ints.is_empty() {
        return Some(Err(VmError::StackUnderflow { stack: "int" }));
    }
    let id = ints.pop().unwrap();
    Some(
        (|| -> Result<Option<Value>> {
            let defaults = Bas::default();
            let bas = engine
                .configs
                .bas
                .as_ref()
                .context("BAS definitions unavailable")?
                .get(&id)
                .unwrap_or(&defaults);
            let ids = bas.extra_seq_ids.as_deref().unwrap_or(&[]);
            let ready = if ids.is_empty() {
                bas.readyanim
            } else {
                let weights = bas
                    .idle_weights
                    .as_ref()
                    .context("BAS extra sequences missing weights")?;
                anyhow::ensure!(
                    weights.len() >= ids.len(),
                    "BAS weights shorter than sequences"
                );
                let mut selected = 0;
                for i in 1..ids.len() {
                    if weights[i] > weights[selected] {
                        selected = i;
                    }
                }
                ids[selected]
            };
            Ok(Some(Value::Int(ready)))
        })()
        .map_err(|e| VmError::TrapFailed {
            command: command.into(),
            reason: format!("{e:#}"),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ready_query_cases() {
        let mut engine = Engine::default();
        let first = Bas {
            readyanim: 1426,
            ..Default::default()
        };
        let tie = Bas {
            readyanim: 9,
            extra_seq_ids: Some(vec![101, 202, 303]),
            idle_weights: Some(vec![0, 7, 7]),
            ..Default::default()
        };
        let zero = Bas {
            readyanim: 77,
            extra_seq_ids: Some(vec![404, 505]),
            idle_weights: Some(vec![0, 0]),
            ..Default::default()
        };
        engine.configs.bas = Some(BTreeMap::from([(1, first), (2, tie), (3, zero)]));
        let run = |id| {
            let mut ints = vec![id];
            match dispatch(&engine, "bas_getanim_ready", &mut ints)
                .unwrap()
                .unwrap()
                .unwrap()
            {
                Value::Int(value) => value,
                other => panic!("unexpected {other:?}"),
            }
        };
        assert_eq!(run(1), 1426);
        assert_eq!(run(2), 202);
        assert_eq!(run(3), 404);
        assert_eq!(run(99), -1);
    }
}

#[cfg(test)]
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
#[allow(
    clippy::field_reassign_with_default,
    reason = "test body kept as moved"
)]
fn all_recorded_queries() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
    let mut engine = Engine::default();
    engine.configs.bas = Some(load(&pack)?);
    use std::fmt::Write as _;
    let count = engine.configs.bas.as_ref().unwrap().len() as i32;
    let mut answers = String::new();
    for id in 0..count {
        let value = dispatch(&engine, "bas_getanim_ready", &mut vec![id])
            .unwrap()
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let Some(Value::Int(answer)) = value else {
            anyhow::bail!("BAS {id}: {value:?}");
        };
        let _ = writeln!(answers, "{id} {answer}");
    }
    rs910_core::test_support::frozen::assert_stream("ui-bas-query/recorded", answers.as_bytes());
    Ok(())
}
