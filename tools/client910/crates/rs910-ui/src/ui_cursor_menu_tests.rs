use super::*;
fn fixture(
    open: bool,
    grouped: bool,
    custom: bool,
    expanded: bool,
    overlap: bool,
    profile: i32,
) -> MiniMenu {
    let mut m = MiniMenu {
        slots: (101..106).map(|c| Some(Entry::default_cancel(c))).collect(),
        entries: (0..5).collect(),
        subs: vec![
            Some(SubMenu {
                title: Some("A".into()),
                entries: vec![0, 1],
                size: 2,
            }),
            Some(SubMenu {
                title: Some("B".into()),
                entries: vec![2, 3, 4],
                size: 3,
            }),
        ],
        submenus: vec![0, 1],
        option_count: 5,
        submenu_count: 2,
        open,
        grouped,
        active: Some(4),
        secondary: Some(1),
        ..Default::default()
    };
    m.popup.ascent = if profile == 0 { 12 } else { 18 };
    m.popup.descent = if profile == 0 { 3 } else { 5 };
    m.row_height = m.popup.ascent + m.popup.descent;
    m.popup.custom = custom;
    m.popup.bounds = [10, 20, 120, 100];
    m.popup.sub_bounds = [if overlap { 80 } else { 130 }, 50, 110, 80];
    m.popup.expanded = expanded.then_some(0);
    m
}
#[test]
fn cursor_hover_obeys_drag_override_and_strict_edges() {
    let mut m = fixture(false, false, false, false, false, 0);
    assert_eq!(m.hover_cursor([50, 50], false, false), 105);
    assert_eq!(m.hover_cursor([50, 50], false, true), 102);
    assert_eq!(m.hover_cursor([50, 50], true, false), -1);
    m.secondary = None;
    assert_eq!(m.hover_cursor([50, 50], false, true), -1);
    m.open = true;
    assert_eq!(m.hover_cursor([10, 51], false, false), -1);
    assert_eq!(m.hover_cursor([11, 51], false, false), 105);
    assert_eq!(m.hover_cursor([130, 51], false, false), -1);
}
/// The hover cursor over every menu layout (open, grouped, custom formatting,
/// expanded submenu, overlap, dragging, override, two font profiles) at
/// positions around every row and edge, against the frozen recording of the
/// original client's menu.
#[test]
fn cursor_menu_matches_the_recording() {
    use std::fmt::Write as _;
    let mut recording = String::new();
    for bits in 0..128 {
        for profile in 0..2 {
            let flag = |bit: i32| bits & bit != 0;
            let (open, grouped, custom, expanded) = (flag(1), flag(2), flag(4), flag(8));
            let (overlap, dragged, cursor_override) = (flag(16), flag(32), flag(64));
            let (ascent, descent) = if profile == 0 { (12, 3) } else { (18, 5) };
            let row_height = ascent + descent;
            let m = fixture(open, grouped, custom, expanded, overlap, profile);
            let mut ys: std::collections::BTreeSet<i32> =
                [-1, 0, 19, 20, 49, 50, 150, 171].into_iter().collect();
            for top in [20, 50] {
                for row in 0..5 {
                    let baseline = top + if custom { ascent + 21 } else { 31 } + row * row_height;
                    for dy in [
                        -ascent - 2,
                        -ascent - 1,
                        -ascent,
                        0,
                        descent - 1,
                        descent,
                        descent + 1,
                    ] {
                        ys.insert(baseline + dy);
                    }
                }
            }
            for x in [9, 10, 11, 79, 80, 81, 129, 130, 131, 189, 190, 240, 241] {
                for &y in &ys {
                    let cursor = m.hover_cursor([x, y], dragged, cursor_override);
                    let _ = writeln!(
                        recording,
                        "{},{},{},{},{},{},{},{profile},{x},{y},{cursor}",
                        i32::from(open),
                        i32::from(grouped),
                        i32::from(custom),
                        i32::from(expanded),
                        i32::from(overlap),
                        i32::from(dragged),
                        i32::from(cursor_override)
                    );
                }
            }
        }
    }
    rs910_core::test_support::frozen::assert_stream("cursors/menu", recording.as_bytes());
}
