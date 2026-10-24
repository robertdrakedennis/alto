mod common;
use native910::{config::ConfigTypes, dbtable, pack::PackArchive};
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn query_indexes_roundtrip_and_listall_has_proven_count_shape() {
    let root = common::pack_root();
    let archive = PackArchive::open(&root.join("client.dbtableindex.js5")).unwrap();
    let mut checked = 0;
    for id in archive.group_ids() {
        for bytes in archive.group_files(id).unwrap().unwrap().values() {
            let index = dbtable::decode_dbindex(bytes).unwrap();
            assert_eq!(dbtable::encode_dbindex(&index).unwrap(), *bytes);
            checked += 1;
        }
    }
    assert!(checked > 0);
    let configs = ConfigTypes::load(&root).unwrap();
    assert_eq!(configs.db_listall_pushes_count(None), Some(true));
    assert_eq!(configs.db_listall_pushes_count(Some(-1)), None);
    eprintln!(
        "{checked} query indexes byte-exact; every provisioned listall index contains integer key zero"
    );
}
