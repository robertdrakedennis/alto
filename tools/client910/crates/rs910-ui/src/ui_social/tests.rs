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

fn friend_bytes(name: &str, world: u16, flags: u8) -> Vec<u8> {
    // UPDATE_FRIENDLIST row.
    let mut out = vec![0];
    out.extend(name.as_bytes());
    out.push(0);
    out.push(0); // previous name ""
    out.extend(world.to_be_bytes());
    out.push(2); // rank
    out.push(flags);
    if world > 0 {
        out.extend(b"W\0");
        out.push(1);
        out.extend(7_i32.to_be_bytes());
    }
    out.push(0); // notes
    out
}

#[test]
fn friend_add_applies_local_guards_before_the_packet() {
    let mut state = State {
        local_player_name: "Me".into(),
        ..State::default()
    };
    state.friends.push(FriendEntry {
        display_name: "Bob".into(),
        ..FriendEntry::default()
    });
    state.ignores.push(IgnoreEntry {
        name_unfiltered: "Eve_1".into(),
        ..IgnoreEntry::default()
    });
    let mut ints = Vec::new();
    let mut run = |state: &mut State, command: &str, name: &str| {
        let mut objs = vec![name.to_string()];
        state.mutation(command, &mut ints, &mut objs).unwrap()
    };
    // Duplicate (normalised " bob " == "bob"), ignored ("eve 1"), self.
    assert!(run(&mut state, "friend_add", " BOB ").is_empty());
    assert!(run(&mut state, "friend_add", "eve 1").is_empty());
    assert!(run(&mut state, "friend_add", "me").is_empty());
    assert!(run(&mut state, "ignore_add", "bob").is_empty());
    assert_eq!(
        state.system_messages,
        vec![
            (4, " BOB  is already on your friends list.".into()),
            (4, "Please remove eve 1 from your ignore list first.".into()),
            (4, "You can't add yourself to your own friends list.".into()),
            (4, "Please remove bob from your friends list first.".into()),
        ]
    );
    // FRIENDLIST_ADD (85/-1): p1(pjstrlen) + pjstr.
    assert_eq!(
        run(&mut state, "friend_add", "Carol"),
        vec![
            crate::proto::client::FRIENDLIST_ADD,
            6,
            b'C',
            b'a',
            b'r',
            b'o',
            b'l',
            0
        ]
    );
    // IGNORELIST_ADD (13/-1): p1(pjstrlen + 1) + pjstr + temp flag.
    assert_eq!(
        run(&mut state, "ignore_add_temp", "Dan"),
        vec![
            crate::proto::client::IGNORELIST_ADD,
            5,
            b'D',
            b'a',
            b'n',
            0,
            1
        ]
    );
    // A full free list (200) refuses with FRIENDLIST_FULL.
    state.system_messages.clear();
    state.friends = vec![FriendEntry::default(); 200];
    assert!(run(&mut state, "friend_add", "Zed").is_empty());
    assert_eq!(
        state.system_messages,
        vec![(4, "Your friends list is full (200 names maximum)".into())]
    );
    state.player_is_members = true;
    assert_eq!(
        run(&mut state, "friend_add", "Zed")[0],
        crate::proto::client::FRIENDLIST_ADD
    );
}

#[test]
fn friend_del_sends_only_for_a_related_entry() {
    let mut state = State::default();
    state.friends.push(FriendEntry {
        display_name: "Bob Smith".into(),
        ..FriendEntry::default()
    });
    let mut ints = Vec::new();
    let mut objs = vec!["nobody".to_string()];
    assert!(state
        .mutation("friend_del", &mut ints, &mut objs)
        .unwrap()
        .is_empty());
    assert!(!state.take_stamps().friend);
    // Related-name checks compare normalised names ("bob_smith").
    let mut objs = vec!["bob_smith".to_string()];
    assert_eq!(
        state.mutation("friend_del", &mut ints, &mut objs).unwrap(),
        [
            &[crate::proto::client::FRIENDLIST_DEL, 10][..],
            b"bob_smith\0"
        ]
        .concat()
    );
    assert!(state.friends.is_empty());
    assert!(state.take_stamps().friend);
}

#[test]
fn friend_world_changes_queue_login_toasts() -> anyhow::Result<()> {
    let mut state = State {
        now_seconds: 100,
        ..State::default()
    };
    state.apply_friend_update(&friend_bytes("Bob", 0, 0), 1)?;
    assert!(
        state.friend_toasts.is_empty(),
        "a new friend queues nothing"
    );
    assert!(state.take_stamps().friend);
    state.apply_friend_update(&friend_bytes("Bob", 1, 0), 1)?;
    assert_eq!(state.friend_toasts.len(), 1);
    assert!(state.poll_friend_toasts(105).is_empty(), "five-second hold");
    assert_eq!(
        state.poll_friend_toasts(106),
        vec!["Bob has logged in.".to_string()]
    );
    // A logout followed by a login inside the hold cancels both.
    state.apply_friend_update(&friend_bytes("Bob", 0, 0), 1)?;
    state.apply_friend_update(&friend_bytes("Bob", 1, 0), 1)?;
    assert!(state.friend_toasts.is_empty());
    Ok(())
}

#[test]
fn friend_list_uses_bubble_pass_order() -> anyhow::Result<()> {
    let mut state = State::default();
    let mut bytes = friend_bytes("Offline", 0, 0);
    bytes.extend(friend_bytes("Referrer", 2, 2));
    bytes.extend(friend_bytes("Here", 1, 0));
    state.apply_friend_update(&bytes, 1)?;
    let names: Vec<_> = state
        .friends
        .iter()
        .map(|f| f.display_name.as_str())
        .collect();
    assert_eq!(names, ["Here", "Referrer", "Offline"]);
    Ok(())
}

#[test]
fn friends_chat_full_and_single_follow_roster_rules() -> anyhow::Result<()> {
    let mut state = State {
        local_player_name: "Me".into(),
        ..State::default()
    };
    let user = |name: &str, world: u16, rank: i8| {
        let mut out = name.as_bytes().to_vec();
        out.extend([0, 0]);
        out.extend(world.to_be_bytes());
        out.push(rank as u8);
        out.extend(b"W\0");
        out
    };
    let mut full = b"Owner\0\0Chan\0".to_vec();
    full.push(0xfe_u8); // min kick -2
    full.push(2);
    full.extend(user("zed", 1, 0));
    full.extend(user("Me", 1, 3));
    state.apply_friend_chat_full(&full)?;
    let names: Vec<_> = state
        .friend_chat
        .users
        .iter()
        .map(|u| u.name.as_str())
        .collect();
    assert_eq!(names, ["Me", "zed"]);
    assert_eq!(state.friend_chat.rank, 3);
    assert!(state.take_stamps().clan);
    // Count 255 keeps the roster.
    let mut keep = b"Owner\0\0Chan\0".to_vec();
    keep.extend([0, 255]);
    state.apply_friend_chat_full(&keep)?;
    assert_eq!(state.friend_chat.users.len(), 2);
    // SINGLEUSER insert keeps normalised order; -128 deletes.
    let mut single = b"Bob\0\0".to_vec();
    single.extend(1_u16.to_be_bytes());
    single.push(0);
    single.extend(b"W\0");
    state.apply_friend_chat_single(&single)?;
    let names: Vec<_> = state
        .friend_chat
        .users
        .iter()
        .map(|u| u.name.as_str())
        .collect();
    assert_eq!(names, ["Bob", "Me", "zed"]);
    let mut delete = b"Bob\0\0".to_vec();
    delete.extend(1_u16.to_be_bytes());
    delete.push(0x80);
    state.apply_friend_chat_single(&delete)?;
    assert_eq!(state.friend_chat.users.len(), 2);
    // Empty FULL clears names/users but keeps rank.
    state.apply_friend_chat_full(&[])?;
    assert!(state.friend_chat.display_name.is_none());
    assert_eq!(state.friend_chat.rank, 3);
    Ok(())
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

#[test]
fn name_quicksort_keeps_slot_pairs() {
    let mut names: Vec<Option<Vec<u16>>> = ["b", "a", "c", "a"]
        .iter()
        .map(|n| Some(n.encode_utf16().collect()))
        .collect();
    let mut slots = vec![0, 1, 2, 3];
    sort_names_with_slots(&mut names, &mut slots);
    let sorted: Vec<String> = names
        .iter()
        .map(|n| String::from_utf16_lossy(n.as_ref().unwrap()))
        .collect();
    assert_eq!(sorted, ["a", "a", "b", "c"]);
    assert_eq!(slots[2], 0);
    assert_eq!(slots[3], 2);
}

#[test]
fn player_group_delta_status_and_varbit_marker_follow_reference() -> anyhow::Result<()> {
    let mut state = State {
        player_group: Some(PlayerGroupState {
            members: vec![PlayerGroupMemberState::default(); 2],
            ..PlayerGroupState::default()
        }),
        ..Default::default()
    };
    let mut delta = vec![0; 8];
    delta.extend(0_i32.to_be_bytes());
    delta.extend([8, 0, 1, 1]); // SetMemberReady(1, true) -> NOT_READY (1)
    delta.extend([13, 0xff, 0xff]); // SetVarbitValue(65535): no value
    delta.push(10); // StartGame -> READY (3) for all
    delta.extend([8, 0, 0, 0]); // SetMemberReady(0, false) -> TELEPORTED (0)
    delta.push(0);
    state.apply_player_group_delta(&delta, |_| None, |_| None, |_| None)?;
    let group = state.player_group.as_ref().unwrap();
    assert_eq!(group.members[0].status, 0);
    assert_eq!(group.members[1].status, 3);
    Ok(())
}
