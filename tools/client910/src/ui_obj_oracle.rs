/// Every cached object definition, serialised, against the frozen recording of
/// the original client's object type.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn object_definitions_match_the_recording() -> anyhow::Result<()> {
    let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
    let objs = crate::config::ObjStore::load(&pack)?;
    let mut result = vec![];
    for group in pack.read_archive_index("obj.config")?.group_id {
        for (file, _) in pack.read_group("obj.config", group)? {
            let id = (group << 8) | file;
            let o = objs.get(id).unwrap();
            let p = &o.inventory;
            let mut v = vec![
                id as i32,
                o.models.first().copied().map_or(-1, |v| v as i32),
                p.zoom,
            ];
            v.extend(p.angles);
            v.extend(p.offset);
            v.extend(p.resize);
            v.extend([
                p.ambient,
                p.contrast,
                p.stackable,
                p.cost,
                o.members as i32,
                p.tradeable as i32,
                p.placeholder as i32,
                o.category,
                p.shardcount,
            ]);
            v.extend(p.wearpos);
            v.extend(p.icursor);
            for pair in p.derived {
                v.extend(pair);
            }
            for n in v {
                result.extend(n.to_be_bytes());
            }
            for t in std::iter::once(Some(o.name.as_str()))
                .chain(o.ops.iter().map(|s| s.as_deref()))
                .chain(o.iops.iter().map(|s| s.as_deref()))
            {
                if let Some(s) = t {
                    result.extend((s.len() as i32).to_be_bytes());
                    result.extend(s.as_bytes());
                } else {
                    result.extend((-1i32).to_be_bytes());
                }
            }
        }
    }
    rs910_core::test_support::frozen::assert_stream("ui-objects/recording", &result);
    Ok(())
}
