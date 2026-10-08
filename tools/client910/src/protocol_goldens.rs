//! Tests of rs910-protocol modules that read client910 material: the protocol
//! tables in `tools/protocol-reference` (generated from `revisions/910/protocol`,
//! read through serde_json) and the client packet builders of `ui_player_options` /
//! `ui_scene_options`. They stay in this crate (tools/README.md "Crate
//! conventions"); moved verbatim from `proto.rs`'s `mod tests`.

mod proto {
    use crate::proto::*;

    /// Every (opcode, size, name) entry of the protocol table data for
    /// `direction` ("server" or "client"), never taken from this module.
    fn recorded_table(direction: &str) -> Vec<(u8, i32, String)> {
        let mut key = direction.to_owned();
        key[..1].make_ascii_uppercase();
        key.push_str("Prot");
        let path = rs910_core::test_support::repo_root()
            .join("tools/protocol-reference/protocol-reference.json");
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
        )
        .unwrap();
        json[key.as_str()]
            .as_array()
            .unwrap_or_else(|| panic!("{direction} table missing from {}", path.display()))
            .iter()
            .map(|row| {
                (
                    u8::try_from(row["opcode"].as_u64().unwrap()).unwrap(),
                    i32::try_from(row["size"].as_i64().unwrap()).unwrap(),
                    row["name"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    /// The server and client tables are dense (0-194 / 0-123); every
    /// recorded id must carry the recorded size and name here, and no other
    /// opcode may be known.
    #[test]
    fn protocol_tables_match_the_recording() {
        type Table = (fn(u8) -> Option<i32>, fn(u8) -> &'static str);
        let tables: [(&str, usize, Table); 2] = [
            ("server", server::COUNT, (server::size, server::name)),
            ("client", client::COUNT, (client::size, client::name)),
        ];
        for (class, count, (size, name)) in tables {
            let recorded = recorded_table(class);
            assert_eq!(recorded.len(), count, "{class} entry count");
            let mut ids: Vec<u8> = recorded.iter().map(|(id, _, _)| *id).collect();
            ids.sort_unstable();
            assert_eq!(
                ids,
                (0..count as u8).collect::<Vec<_>>(),
                "{class} is dense"
            );
            for (id, recorded_size, recorded_name) in &recorded {
                assert_eq!(
                    size(*id),
                    Some(*recorded_size),
                    "{class} {recorded_name} ({id}) size"
                );
                // Opcodes without a handler are recorded as `UNUSED_<opcode>`
                // (client) / `UNHANDLED_<opcode>` (server); the names match exactly.
                assert_eq!(name(*id), recorded_name, "{class} {id} name");
            }
            for opcode in count as u8..=u8::MAX {
                assert_eq!(size(opcode), None, "{class} {opcode} must be unknown");
                assert_eq!(name(opcode), "UNKNOWN");
            }
        }
    }

    /// The client packet builders emit their recorded client opcode with a
    /// payload of the registry's fixed size; `-1` builders lead with the
    /// one-byte length prefix, which doubles as the framing
    /// length.
    #[test]
    fn client_builders_emit_registry_framing() {
        use crate::ui_dialogue as dialogue;
        use crate::ui_player_options as player;
        use crate::ui_scene_options as scene;
        let loc = scene::encode_loc_id(7, 0, 0, 0, 0, false, false);
        let minimap = scene::move_minimap_click([0, 0], [0, 0], false, 0, 0, 0, [0, 0]);
        let fixed: [(u8, u8, usize); 13] = [
            (
                client::RESUME_P_COUNTDIALOG,
                dialogue::build_resume_count(1).0,
                dialogue::build_resume_count(1).1.len(),
            ),
            (
                client::RESUME_P_OBJDIALOG,
                dialogue::build_resume_obj(7).0,
                dialogue::build_resume_obj(7).1.len(),
            ),
            (
                client::RESUME_P_HSLDIALOG,
                dialogue::build_resume_hsl(7).0,
                dialogue::build_resume_hsl(7).1.len(),
            ),
            (client::MOVE_MINIMAPCLICK, minimap[0], minimap.len() - 1),
            {
                let (op, payload) = player::build_opplayert(15, 1, 0, 0, 0, false).unwrap();
                (client::OPPLAYERT, op, payload.len())
            },
            {
                let (op, payload) = scene::build_opnpc(9, 0x1234, false).unwrap();
                (client::OPNPC1, op, payload.len())
            },
            {
                let (op, payload) =
                    scene::build_oploc(3, loc, [3200, 3200], [0, 0], false).unwrap();
                (client::OPLOC1, op, payload.len())
            },
            {
                let (op, payload) =
                    scene::build_opobj(18, 1, [0, 0], [0, 0], false, false).unwrap();
                (client::OPOBJ1, op, payload.len())
            },
            {
                let (op, payload) = scene::build_opnpct(8, 1, Default::default(), false).unwrap();
                (client::OPNPCT, op, payload.len())
            },
            {
                let (op, payload) =
                    scene::build_opobjt(17, 1, [0, 0], [0, 0], Default::default(), false).unwrap();
                (client::OPOBJT, op, payload.len())
            },
            {
                let (op, payload) =
                    scene::build_oploct(2, loc, [0, 0], [0, 0], Default::default(), false).unwrap();
                (client::OPLOCT, op, payload.len())
            },
            {
                let (op, payload) =
                    scene::build_apcoordt(59, [0, 0], [0, 0], Default::default()).unwrap();
                (client::APCOORDT, op, payload.len())
            },
            {
                let (op, payload) = player::build_if_buttont(
                    58,
                    &player::TargetedButtonUse {
                        target_parentlayer: 0,
                        target_invobject: 0,
                        target_id: 0,
                        active_invobject: 0,
                        active_id: 0,
                        active_parentlayer: 0,
                    },
                )
                .unwrap();
                (client::IF_BUTTONT, op, payload.len())
            },
        ];
        for (opcode, built_op, len) in fixed {
            let label = client::name(opcode);
            assert_eq!(built_op, opcode, "{label} opcode");
            assert_eq!(
                client::size(opcode),
                Some(len as i32),
                "{label} payload size"
            );
        }
        for (opcode, (built_op, payload)) in [
            (
                client::RESUME_P_STRINGDIALOG,
                dialogue::build_resume_string("ab").unwrap(),
            ),
            (
                client::RESUME_P_NAMEDIALOG,
                dialogue::build_resume_name("ab").unwrap(),
            ),
        ] {
            let label = client::name(opcode);
            assert_eq!(built_op, opcode, "{label} opcode");
            assert_eq!(client::size(opcode), Some(-1), "{label} is u8-framed");
            assert_eq!(payload[0] as usize, payload.len() - 1, "{label} p1 length");
        }
    }
}
