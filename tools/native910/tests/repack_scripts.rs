mod common;
use native910::opcode::OpcodeBook;
use native910::pack::PackArchive;
use native910::repack::{crc32, scripts};
use native910::script::{CompiledScript, Counts, Instruction, Operand, encode_script};
use std::collections::BTreeMap;

#[test]
fn crc_matches_ieee_check_vector() {
    assert_eq!(crc32(b"123456789") as u32, 0xcbf4_3926);
}

#[test]
#[ignore = "requires provisioned script pack"]
fn replacement_addition_and_unchanged_build() {
    let path = common::pack_root().join("client.scripts.js5");
    let original = std::fs::read(path).expect("required script corpus");
    assert_eq!(scripts(&original, &BTreeMap::new()).unwrap(), original);
    let archive = PackArchive::from_bytes(original.clone()).unwrap();
    let first = archive.group_ids().next().unwrap();
    let new_id = archive.group_ids().max().unwrap() + 1;
    let script = CompiledScript {
        name: Some("new_fixture".into()),
        args: Counts::default(),
        locals: Counts::default(),
        code: vec![Instruction {
            opcode: 0,
            command: "return".into(),
            operand: Operand::Byte(0),
        }],
    };
    let bytes = encode_script(&script, &OpcodeBook::embedded().unwrap()).unwrap();
    let replacement = BTreeMap::from([(first, bytes.clone()), (new_id, bytes.clone())]);
    let output = scripts(&original, &replacement).unwrap();
    let rebuilt = PackArchive::from_bytes(output.clone()).unwrap();
    assert_eq!(rebuilt.index().version, archive.index().version + 1);
    assert_eq!(rebuilt.group_files(new_id).unwrap().unwrap()[&0], bytes);
    assert_eq!(rebuilt.group_files(first).unwrap().unwrap()[&0], bytes);
    for id in archive.group_ids().filter(|id| *id != first) {
        assert_eq!(rebuilt.group_container(id), archive.group_container(id));
    }
    assert_eq!(scripts(&output, &replacement).unwrap(), output);
    let out = common::native_dir().join("target/repack-verified");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("client.scripts.js5"), output).unwrap();
}
