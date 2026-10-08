//! Comparison of the chat history with its recorded behaviour.
use super::{ChatHistory, ChatLine, NewChatLine};
fn row(tag: &str, l: &ChatLine) -> String {
    format!(
        "{tag} uid={} type={} flags={} name={} clan={} phrase={} message={} crown={}",
        l.uid,
        l.chat_type,
        l.flags,
        l.name,
        l.clan.as_deref().unwrap_or("null"),
        l.phrase,
        l.message,
        l.crown.map_or("null".into(), |c| c.to_string())
    )
}
#[test]
fn chat_history_matches_the_recorded_trace() -> anyhow::Result<()> {
    let mut h = ChatHistory::with_clock(|| 0);
    h.mes("system");
    h.add_system_message(1, "alpha");
    h.add_message(NewChatLine {
        flags: 7,
        name: "N".into(),
        name_unfiltered: "U".into(),
        name_simple: "S".into(),
        clan: Some("beta".into()),
        phrase: 42,
        ..NewChatLine::system(0, "C")
    });
    let mut lines = vec![
        row("type0[0]", h.get_by_type_and_line(0, 0).unwrap()),
        row("type0[1]", h.get_by_type_and_line(0, 1).unwrap()),
        format!(
            "nav previous2={} next0={} last={}",
            h.previous_uid(2),
            h.next_uid_of(0),
            h.last_uid()
        ),
    ];
    for i in 0..101 {
        h.add_system_message(7, format!("m{i}"));
    }
    assert!(h.get_by_uid(3).is_none());
    lines.push(format!(
        "recycle uid0={} type7length={} type7oldest={} last={}",
        h.get_by_uid(0).is_none(),
        h.length(7),
        h.get_by_type_and_line(7, 99).unwrap().uid,
        h.last_uid()
    ));
    h.clear();
    h.add_system_message(0, "after-clear");
    lines.push(format!(
        "clear uid={} last={}",
        h.get_by_uid(0).unwrap().uid,
        h.last_uid()
    ));
    assert_eq!(
        lines.join("\n") + "\n",
        rs910_core::test_support::frozen::text("chat-history/recorded.txt")
    );
    Ok(())
}
