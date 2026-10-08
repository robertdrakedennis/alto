use super::Preferences;

#[test]
fn bloom_command_matches_recorded_state_machine() -> anyhow::Result<()> {
    let recorded = rs910_core::test_support::frozen::text("bloom-settings/reference.txt");
    let mut rows = Vec::new();
    for supported in [false, true] {
        for enabled in [false, true] {
            for raw in [-7, -1, 0, 1, 2, 255] {
                let mut p = Preferences {
                    bloom: supported,
                    bloom_enabled: enabled,
                    ..Default::default()
                };
                p.dispatch("detail_bloom", &mut vec![raw]).unwrap().unwrap();
                rows.push(format!(
                    "{supported},{enabled},{raw}|pref={}|enabled={}|saves={}",
                    p.options.get("bloom").unwrap(),
                    p.bloom_enabled,
                    i32::from(p.dirty)
                ));
            }
        }
    }
    assert_eq!(rows.join("\n") + "\n", recorded);
    eprintln!(
        "{} bloom state transitions matched the recording",
        rows.len()
    );
    Ok(())
}
