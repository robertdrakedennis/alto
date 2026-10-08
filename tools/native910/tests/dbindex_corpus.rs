mod common;
use native910::{
    config::ConfigTypes,
    dbtable::{self, DbCellValue, DbIndex},
    pack::PackArchive,
};
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
#[test]
fn empty_list_is_present_and_duplicate_keys_use_last_value() {
    let index = DbIndex {
        base: 0,
        entries: vec![
            (DbCellValue::Int(0), vec![1]),
            (DbCellValue::Int(0), vec![]),
        ],
    };
    let decoded = dbtable::decode_dbindex(&dbtable::encode_dbindex(&index).unwrap()).unwrap();
    assert_eq!(decoded.lookup_int(0), Some([].as_slice()));
    assert_eq!(decoded.lookup_int(1), None);
    assert!(dbtable::decode_dbindex(&[255, 0]).is_err());
    assert!(dbtable::decode_dbindex(&[0, 127]).is_err());
}
