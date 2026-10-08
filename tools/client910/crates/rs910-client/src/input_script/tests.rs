use super::*;

fn script(vars: &[(&str, &str)]) -> InputScript {
    InputScript::from_lookup(&|name| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| OsString::from(*v))
    })
}

/// Each injector's row rules (trimming, arity, dropped fields, ranges) and
/// the fixed per-cycle order KEY, TYPE, WHEEL, TOOLKIT, UI_OPERATIONS,
/// UI_INPUT, UI_CLICK, UI_HOVER, UI_CLICKS.
#[test]
fn ui_injections_follow_row_rules_in_the_fixed_order() {
    let s = script(&[
        (
            "CLIENT910_KEY_INPUT",
            "10,13,0; 10 , 13 , 1 ;11,x,13,1;12,1;100,9,0",
        ),
        (
            "CLIENT910_TYPE_INPUT",
            "5:Ab 9;x:y;6;  7 :a:b\\n\\t;5:again;100:t",
        ),
        ("CLIENT910_WHEEL_INPUT", "20,1;20,-3;21, 2 ;bad;100,4"),
        ("CLIENT910_TOOLKIT_INPUT", "30,0;30,5,1; 31 ,2;100,3"),
        // Untrimmed: " 1477" is dropped, leaving a 3-field row.
        (
            "CLIENT910_UI_OPERATIONS",
            "40,1477,5,1;40, 1477,5,1;41,1,2,3,4;100,1,2,3",
        ),
        (
            "CLIENT910_UI_INPUT",
            "50,100,200,1,0;50,100,200,0,-1;51,1,2,3;100,1,2,3,4",
        ),
        (
            "CLIENT910_UI_CLICK",
            "100,120,60,62;7,8,61,61,2;;1,2;3,4,5,6,7,8",
        ),
        (
            "CLIENT910_UI_HOVER",
            "10,20,70,72;1,2;30,40,71,71,9;6,6,100",
        ),
        (
            "CLIENT910_UI_CLICKS",
            "l,1,2,80;r, 3 ,4,80;m,5,6,99;x,1,2,80;l,1,2",
        ),
    ]);
    use UiInjection::*;
    let key = |code, mode| Key { code, mode };
    let text = |t: &str| Type { text: t.into() };
    assert_eq!(s.ui_injections(10), [key(13, 0), key(13, 1)]);
    // "x" is dropped, leaving the three fields 11,13,1.
    assert_eq!(s.ui_injections(11), [key(13, 1)]);
    assert!(s.ui_injections(12).is_empty());
    assert_eq!(s.ui_injections(5), [text("Ab 9"), text("again")]);
    assert_eq!(s.ui_injections(7), [text("a:b\n\t")]);
    assert_eq!(
        s.ui_injections(20),
        [Wheel { delta: 1 }, Wheel { delta: -3 }]
    );
    assert_eq!(s.ui_injections(21), [Wheel { delta: 2 }]);
    assert_eq!(s.ui_injections(30), [Toolkit { cycle: 30, id: 0 }]);
    assert_eq!(s.ui_injections(31), [Toolkit { cycle: 31, id: 2 }]);
    assert_eq!(
        s.ui_injections(40),
        [Operation {
            parent: 1477,
            child: 5,
            op: 1
        }]
    );
    assert!(s.ui_injections(41).is_empty());
    let gesture = |held, press| Gesture {
        cycle: 50,
        x: 100,
        y: 200,
        held,
        press,
    };
    assert_eq!(s.ui_injections(50), [gesture(1, 0), gesture(0, -1)]);
    assert!(s.ui_injections(51).is_empty());
    // UI_CLICK holds from `first` to `last`; only `first` is the press.
    let click = |x, y, action, first| Click {
        x,
        y,
        action,
        first,
    };
    assert_eq!(s.ui_injections(60), [click(100, 120, 0, true)]);
    assert_eq!(
        s.ui_injections(61),
        [click(100, 120, 0, false), click(7, 8, 2, true)]
    );
    assert_eq!(s.ui_injections(62), [click(100, 120, 0, false)]);
    assert!(s.ui_injections(63).is_empty());
    // UI_HOVER holds from `first` to `last` (a fifth field is ignored).
    assert_eq!(s.ui_injections(70), [Hover { x: 10, y: 20 }]);
    assert_eq!(
        s.ui_injections(71),
        [Hover { x: 10, y: 20 }, Hover { x: 30, y: 40 }]
    );
    assert!(s.ui_injections(73).is_empty());
    // UI_CLICKS presses at its cycle and releases on the next one.
    assert_eq!(
        s.ui_injections(80),
        [
            ClicksPress {
                button: "l".into(),
                x: 1,
                y: 2,
                cycle: 80,
                action: 0
            },
            ClicksPress {
                button: "r".into(),
                x: 3,
                y: 4,
                cycle: 80,
                action: 2
            },
        ]
    );
    assert_eq!(s.ui_injections(81), [ClicksRelease, ClicksRelease]);
    // Cycle 100: one event of every kind, in the fixed order.
    assert_eq!(
        s.ui_injections(100),
        [
            key(9, 0),
            text("t"),
            Wheel { delta: 4 },
            Toolkit { cycle: 100, id: 3 },
            Operation {
                parent: 1,
                child: 2,
                op: 3
            },
            Gesture {
                cycle: 100,
                x: 1,
                y: 2,
                held: 3,
                press: 4
            },
            Hover { x: 6, y: 6 },
            ClicksRelease,
        ]
    );
    // Malformed UI_CLICK entries are reported on cycles % 100 == 1 only.
    let malformed = |spec: &str| ClickMalformed { spec: spec.into() };
    assert_eq!(
        s.ui_injections(101),
        [malformed(""), malformed("1,2"), malformed("3,4,5,6,7,8")]
    );
    assert_eq!(s.ui_injections(201).len(), 3);
    assert!(s.ui_injections(102).is_empty());
    // No injector set: nothing, every cycle.
    assert!(script(&[]).ui_injections(1).is_empty());
}

/// WINDOW_RESIZES (untrimmed, positive sizes only), the CUTSCENE fixture
/// (id, cycle, optional capacity) and DROP_CONNECTION (trimmed).
#[test]
fn resizes_cutscene_and_drop_follow_their_row_rules() {
    let s = script(&[
        (
            "CLIENT910_WINDOW_RESIZES",
            "10,800,600;10, 900,700;11,0,5;12,1,1",
        ),
        ("CLIENT910_CUTSCENE", "7, 120"),
        ("CLIENT910_DROP_CONNECTION", " 300 "),
    ]);
    assert_eq!(s.window_resizes(10).collect::<Vec<_>>(), [(800, 600)]);
    assert_eq!(s.window_resizes(11).count(), 0);
    assert_eq!(s.window_resizes(12).collect::<Vec<_>>(), [(1, 1)]);
    assert_eq!(s.drop_connection(), Some(300));
    assert!(s.cutscene_enabled());
    assert_eq!(s.cutscene_fixture(119), None);
    assert_eq!(
        s.cutscene_fixture(120),
        Some(CutsceneFixture {
            id: 7,
            cycle: 120,
            capacity: None
        })
    );
    let s = script(&[
        ("CLIENT910_CUTSCENE", "7,130,32,9"),
        ("CLIENT910_DROP_CONNECTION", "x"),
    ]);
    assert_eq!(
        s.cutscene_fixture(130),
        Some(CutsceneFixture {
            id: 7,
            cycle: 130,
            capacity: Some(32)
        })
    );
    assert_eq!(s.drop_connection(), None);
    // A cutscene without a cycle enables the CAM_RESET return but never
    // injects.
    let s = script(&[("CLIENT910_CUTSCENE", "7")]);
    assert!(s.cutscene_enabled());
    assert!((-2..=500).all(|cycle| s.cutscene_fixture(cycle).is_none()));
    assert!(!script(&[]).cutscene_enabled());
}
