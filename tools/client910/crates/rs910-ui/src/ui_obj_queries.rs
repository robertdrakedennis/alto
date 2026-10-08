//! Object queries over the resolved object type cache.
use crate::{
    config::{ObjStore, ParamValue},
    ui_runtime::Engine,
};
use anyhow::{Context, Result};
use native910::vm::{Value, VmError, VmResult};
pub fn dispatch(
    engine: &Engine,
    command: &str,
    ints: &mut Vec<i32>,
) -> Option<VmResult<Option<Value>>> {
    let count = match command {
        "oc_op" | "oc_iop" | "oc_icursor" | "oc_param" => 2,
        "oc_name" | "oc_cost" | "oc_stackable" | "oc_cert" | "oc_uncert" | "oc_shard"
        | "oc_unshard" | "oc_shardcount" | "oc_wearpos" | "oc_wearpos2" | "oc_wearpos3"
        | "oc_tradeable" | "oc_placeholder" | "oc_hasvarobj" | "oc_id" => 1,
        _ => return None,
    };
    if ints.len() < count {
        return Some(Err(VmError::StackUnderflow { stack: "int" }));
    }
    let a = ints.split_off(ints.len() - count);
    Some(
        (|| -> Result<Option<Value>> {
            let store: &ObjStore = engine
                .configs
                .objs
                .as_ref()
                .context("object definitions not installed")?;
            let default = crate::config::decode_obj(a[0] as u32, &[0])?;
            // list -> postDecode members gate
            // under allowMembers; absent param types count as
            // autodisable (ParamConfig default).
            let autodisable = |key: i32| {
                engine
                    .configs
                    .params
                    .get(&key)
                    .is_none_or(|d| d.autodisable)
            };
            let obj = store
                .get(a[0] as u32)
                .unwrap_or(&default)
                .members_gated(store.allow_members.get(), &autodisable);
            let p = &obj.inventory;
            if command == "oc_name" {
                return Ok(Some(Value::Str(obj.name.clone())));
            }
            if matches!(command, "oc_op" | "oc_iop") {
                let ops = if command == "oc_op" {
                    &obj.ops
                } else {
                    &obj.iops
                };
                return Ok(Some(Value::Str(
                    a[1].checked_sub(1)
                        .and_then(|i| usize::try_from(i).ok())
                        .and_then(|i| ops.get(i))
                        .and_then(Option::as_ref)
                        .cloned()
                        .unwrap_or_default(),
                )));
            }
            if command == "oc_param" {
                let def = engine.configs.params.get(&a[1]);
                let value = obj
                    .params
                    .iter()
                    .find(|(id, _)| *id == a[1])
                    .map(|(_, v)| v);
                let string = def.is_some_and(|d| {
                    d.kind == Some(36) || (d.kind.is_none() && d.kind_legacy == Some(b's'))
                });
                return Ok(Some(if string {
                    Value::Str(match value {
                        None => def
                            .and_then(|d| d.default_string.clone())
                            .unwrap_or_else(|| "null".into()),
                        Some(ParamValue::Str(s)) => s.clone(),
                        _ => anyhow::bail!("object string param type mismatch"),
                    })
                } else {
                    Value::Int(match value {
                        None => def.and_then(|d| d.default_int).unwrap_or(0),
                        Some(ParamValue::Int(v)) => *v,
                        _ => anyhow::bail!("object int param type mismatch"),
                    })
                }));
            }
            let value = match command {
                "oc_cost" => p.cost,
                "oc_stackable" => i32::from(p.stackable == 1),
                "oc_hasvarobj" => i32::from(p.stackable == 2),
                "oc_wearpos" => p.wearpos[0],
                "oc_wearpos2" => p.wearpos[1],
                "oc_wearpos3" => p.wearpos[2],
                "oc_tradeable" => i32::from(p.tradeable),
                "oc_placeholder" => i32::from(p.placeholder),
                "oc_id" => obj.id as i32,
                "oc_shardcount" => p.shardcount,
                "oc_icursor" => a[1]
                    .checked_sub(1)
                    .and_then(|i| usize::try_from(i).ok())
                    .and_then(|i| p.icursor.get(i))
                    .copied()
                    .unwrap_or(-1),
                _ => {
                    let d = p.derived[if command.contains("shard") { 3 } else { 0 }];
                    if d[0] >= 0
                        && if command.starts_with("oc_un") {
                            d[1] >= 0
                        } else {
                            d[1] == -1
                        }
                    {
                        d[0]
                    } else {
                        a[0]
                    }
                }
            };
            Ok(Some(Value::Int(value)))
        })()
        .map_err(|e| VmError::TrapFailed {
            command: command.into(),
            reason: format!("{e:#}"),
        }),
    )
}
#[cfg(test)]
mod members_tests {
    use super::*;
    use std::{collections::BTreeMap, rc::Rc};

    /// `oc_op`/`oc_iop`/`oc_tradeable`/`oc_param` read the post-decode type
    /// on a free world
    /// (`setAllowMembers(false)`,) a members object has
    /// the factory defaults.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn oc_queries_follow_allow_members_on_real_obj() {
        const WHIP: i32 = rs910_symbols::obj::ABYSSAL_WHIP.id();
        const GE_CATEGORY: i32 = rs910_symbols::param::EXCHANGE_CATEGORY.id();
        let pack = crate::cache::Pack::open(
            rs910_core::test_support::client_dir().join("../../server/data/pack"),
        );
        let files = pack
            .read_group(crate::config::OBJ_ARCHIVE, WHIP as u32 >> 8)
            .unwrap();
        let bytes = &files
            .iter()
            .find(|(f, _)| **f == WHIP as u32 & 0xff)
            .unwrap()
            .1;
        let whip = crate::config::decode_obj(WHIP as u32, bytes).unwrap();
        let mut engine = Engine::default();
        for (id, bytes) in pack.read_group("config", 11).unwrap() {
            if let Ok(p) = native910::config::decode_param(&bytes) {
                engine.configs.params.insert(id as i32, p);
            }
        }
        let store = Rc::new(ObjStore::from_map(BTreeMap::from([(WHIP as u32, whip)])));
        engine.configs.objs = Some(store.clone());
        let run = |engine: &Engine, command: &str, args: &[i32]| {
            let mut ints = args.to_vec();
            dispatch(engine, command, &mut ints)
                .unwrap()
                .unwrap()
                .unwrap()
        };
        let text = |v: Value| match v {
            Value::Str(s) => s,
            other => panic!("{other:?}"),
        };
        assert_eq!(text(run(&engine, "oc_iop", &[WHIP, 2])), "Wield");
        assert!(matches!(
            run(&engine, "oc_tradeable", &[WHIP]),
            Value::Int(1)
        ));
        store.allow_members.set(false);
        assert_eq!(text(run(&engine, "oc_iop", &[WHIP, 2])), "");
        assert_eq!(text(run(&engine, "oc_iop", &[WHIP, 5])), "Drop");
        assert_eq!(text(run(&engine, "oc_op", &[WHIP, 3])), "Take");
        assert!(matches!(
            run(&engine, "oc_tradeable", &[WHIP]),
            Value::Int(0)
        ));
        // The exchange category param is autodisabled: the default comes back.
        let default = engine
            .configs
            .params
            .get(&GE_CATEGORY)
            .and_then(|p| p.default_int)
            .unwrap_or(0);
        assert!(engine
            .configs
            .params
            .get(&GE_CATEGORY)
            .is_none_or(|p| p.autodisable));
        assert!(
            matches!(run(&engine, "oc_param", &[WHIP, GE_CATEGORY]), Value::Int(v) if v == default)
        );
    }
}
