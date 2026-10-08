mod common;

use native910::{interface, pack::PackArchive, repack};
use std::collections::BTreeMap;

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn interface_edits_preserve_siblings_and_publish_sparse_groups() {
    const NEXT_ID: u32 = 1;
    const SPARSE_FILE_GAP: u32 = 2;
    const FIRST_FILE: u32 = 0;
    const NO_PARENT: i32 = -1;
    let original = std::fs::read(common::require_pack_file("client.interfaces.js5")).unwrap();
    let archive = PackArchive::from_bytes(original.clone()).unwrap();
    let (group, files) = archive
        .group_ids()
        .find_map(|group| {
            let files = archive.group_files(group).unwrap()?;
            (files.len() > NEXT_ID as usize).then_some((group, files))
        })
        .unwrap();
    let (&file, bytes) = files.first_key_value().unwrap();
    let address = |group, file| {
        native910::xref::pack_component(native910::xref::ComponentRef {
            iface: i32::try_from(group).unwrap(),
            child: i32::try_from(file).unwrap(),
        })
        .unwrap()
    };
    let mut edited = interface::decode_component(bytes, address(group, file)).unwrap();
    edited.hide = !edited.hide;
    let replacement = interface::encode_component(&edited, address(group, file)).unwrap();
    let added_file = files.keys().next_back().unwrap() + SPARSE_FILE_GAP;
    let added_group = archive.group_ids().max().unwrap() + NEXT_ID;
    let mut long_component = edited;
    long_component.layer = NO_PARENT;
    long_component.opbase = "A deliberately longer authored component".into();
    let long_bytes =
        interface::encode_component(&long_component, address(added_group, FIRST_FILE)).unwrap();
    let additions = BTreeMap::from([
        ((group, file), replacement.clone()),
        ((group, added_file), replacement.clone()),
        ((added_group, FIRST_FILE), long_bytes.clone()),
        ((added_group, SPARSE_FILE_GAP), replacement.clone()),
    ]);
    assert_eq!(
        repack::interfaces(&original, &BTreeMap::new()).unwrap(),
        original
    );
    assert_eq!(
        repack::interfaces(&original, &BTreeMap::from([((group, file), bytes.clone())])).unwrap(),
        original
    );
    let output = repack::interfaces(&original, &additions).unwrap();
    let rebuilt = PackArchive::from_bytes(output.clone()).unwrap();
    let siblings = rebuilt.group_files(group).unwrap().unwrap();
    for (sibling, expected) in &files {
        assert_eq!(
            &siblings[sibling],
            if *sibling == file {
                &replacement
            } else {
                expected
            }
        );
    }
    assert_eq!(siblings[&added_file], replacement);
    assert_eq!(
        rebuilt.group_files(added_group).unwrap().unwrap(),
        BTreeMap::from([(FIRST_FILE, long_bytes), (SPARSE_FILE_GAP, replacement),])
    );
    for untouched in archive.group_ids().filter(|id| *id != group) {
        assert_eq!(
            rebuilt.group_container(untouched),
            archive.group_container(untouched)
        );
    }
    assert_eq!(repack::interfaces(&output, &additions).unwrap(), output);
    assert!(
        repack::interfaces(
            &original,
            &BTreeMap::from([((u32::MAX, file), bytes.clone())])
        )
        .is_err()
    );
    let directory = common::native_dir().join("target/interfaces-verified");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("client.interfaces.js5"), output).unwrap();
}
