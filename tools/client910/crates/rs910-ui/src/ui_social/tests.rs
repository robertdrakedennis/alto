use super::*;

#[test]
fn quickchat_dynamic_values_use_transmit_byte_width_and_enum_text() {
    let phrase_bytes = [
        1, b'E', b'n', b'u', b'm', b' ', b'<', 0, 3, 1, 0, 6, 0, 42, 0, 99, 0,
    ];
    let phrase = QuickChatStore::decode_phrase(&phrase_bytes).expect("phrase");
    let mut store = QuickChatStore::default();
    store.phrases.insert(12, phrase);

    let mut configs = crate::ui_configs::Configs::default();
    let mut values = BTreeMap::new();
    values.insert(
        7,
        crate::ui_configs::Scalar::String("seven".encode_utf16().collect()),
    );
    configs.enums.insert(
        42,
        crate::ui_configs::Enum {
            count: 1,
            storage: crate::ui_configs::Storage::Sparse(values),
            ..Default::default()
        },
    );

    let mut pos = 0;
    let message = store.render(12, &[0, 0, 0, 7, 0xaa], &mut pos, &configs, None);
    assert_eq!(message, "Enum seven");
    assert_eq!(pos, 4, "gVarLong consumes four whole bytes");
    assert_eq!(store.text_display(12).as_deref(), Some("Enum ..."));
    assert_eq!(store.dynamic_parameter(12, 0, 0), Some((6, 42)));

    let transmit_phrase =
        QuickChatStore::decode_phrase(&[1, b'X', b' ', b'<', 0, 3, 1, 0, 0, 0, 1, 0])
            .expect("transmit phrase");
    store.phrases.insert(13, transmit_phrase);
    assert_eq!(store.dynamic_count(13), Some(1));
    assert_eq!(store.dynamic_command(13, 0), Some(0));
    assert_eq!(store.transmit_values(13, &[0x1234]), Some(vec![0x12, 0x34]));

    let category = QuickChatStore::decode_category(&[
        1, b'R', b'o', b'o', b't', 0, 2, 1, 0, 7, b's', 3, 1, 0, 12, b'p', 0,
    ])
    .expect("category");
    store.categories.insert(3, category);
    assert_eq!(store.category_description(3), Some("Root"));
    assert_eq!(store.category_sub(3, 0), Some(7));
    assert_eq!(store.category_phrase(3, 0), Some(12));
    assert_eq!(store.category_find_sub_by_shortcut(3, 's'), 7);
    assert_eq!(store.category_find_phrase_by_shortcut(3, 'p'), 12);
    assert_eq!(store.find_phrases("enum", false), Some(vec![12]));
    assert_eq!(store.find_phrases("x", false), Some(vec![13]));
}

#[test]
fn clan_channel_reads_names_without_flag_and_rewinds_delta_hashes() -> anyhow::Result<()> {
    let mut state = State::default();
    // flags 0: use_display_names stays true.
    let mut full = vec![1, 0];
    full.extend(9_u64.to_be_bytes());
    full.extend(4_u64.to_be_bytes());
    full.extend(b"Clan\0");
    full.extend([0, 0, 0]);
    full.extend(1_u16.to_be_bytes());
    full.extend(b"Ann\0");
    full.push(0);
    full.extend(1_u16.to_be_bytes());
    state.apply_clan_channel_full(&full)?;
    assert_eq!(state.affined_channel.as_ref().unwrap().users[0].name, "Ann");
    // AddUser with a real 8-byte hash (first byte != 255 is rewound),
    // then DeleteUser with the 255 no-hash marker.
    let mut delta = vec![1];
    delta.extend(9_u64.to_be_bytes());
    delta.extend(4_u64.to_be_bytes());
    delta.push(1);
    delta.extend(0x0102_0304_0506_0708_u64.to_be_bytes());
    delta.extend(b"Ben\0");
    delta.extend(2_u16.to_be_bytes());
    delta.push(0);
    delta.extend(0_u64.to_be_bytes());
    delta.push(3);
    delta.extend(0_u16.to_be_bytes());
    delta.extend([0, 255]);
    delta.push(0);
    state.apply_clan_channel_delta(&delta)?;
    let channel = state.affined_channel.as_ref().unwrap();
    assert_eq!(channel.users.len(), 1);
    assert_eq!(channel.users[0].name, "Ben");
    assert_eq!(channel.update_num, 5);
    assert!(state.take_stamps().clan_channel);
    Ok(())
}

#[test]
fn namespace_and_base37_match_reference() {
    assert_eq!(
        namespace_normalize("  Bob-Smith "),
        Some("bob_smith".into())
    );
    assert_eq!(namespace_normalize("Élan"), Some("elan".into()));
    assert_eq!(namespace_normalize("abcdefghijklm"), None, "13 > 12");
    assert_eq!(namespace_normalize("--"), None);
    assert_eq!(
        from_base37(to_base37("my chan")),
        Some("My\u{a0}Chan".into())
    );
    assert_eq!(from_base37(to_base37("")), None);
}
