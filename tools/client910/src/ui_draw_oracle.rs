use crate::{
    ui_components::{Array, Component, Interface, Ref, Store},
    ui_components_oracle::{int, text},
    ui_draw::{Backend, Call, Fade, Frame, Kind, Reply, View},
    ui_properties::State,
};
use std::{cell::RefCell, rc::Rc};
const FIELDS: &[&str] = &[
    "parentlayer",
    "id",
    "type",
    "layer",
    "x",
    "y",
    "width",
    "height",
    "scrollx",
    "scrolly",
    "trans",
    "hide",
    "clientcode",
    "dragrenderbehaviour",
    "colour",
    "fill",
    "linedirection",
    "linewid",
    "topLevelIndex",
    "lastDrawCycle",
];
fn fields(c: &Ref) -> [i32; 20] {
    let c = c.borrow();
    let f = &c.f;
    [
        f.parentlayer,
        f.id,
        f.r#type,
        f.layer,
        f.x,
        f.y,
        f.width,
        f.height,
        f.scrollx,
        f.scrolly,
        f.trans,
        f.hide as i32,
        f.clientcode,
        f.dragrenderbehaviour,
        f.colour,
        f.fill as i32,
        f.linedirection as i32,
        f.linewid,
        f.topLevelIndex,
        f.lastDrawCycle,
    ]
}
fn reference(nodes: &[Ref], c: Option<&Ref>) -> i32 {
    c.map_or(-1, |c| {
        nodes.iter().position(|v| Rc::ptr_eq(v, c)).unwrap() as i32
    })
}
struct Capture {
    scenario: i32,
    nodes: Vec<Ref>,
    calls: Vec<Call>,
    fail: i32,
    preview: Option<[i32; 2]>,
}
impl Backend for Capture {
    fn call(
        &mut self,
        frame: &mut Frame,
        _: &mut Store,
        _: &mut State,
        call: Call,
    ) -> anyhow::Result<Reply> {
        let kind = call.kind;
        let c = call.component.clone();
        let n = self.calls.len() as i32;
        self.calls.push(call);
        anyhow::ensure!(n != self.fail, "injected graphics failure");
        if kind == Kind::ClientComponent && self.scenario % 5 == 0 {
            let c = c.as_ref().unwrap();
            let mut c = c.borrow_mut();
            c.f.x = c.f.x.wrapping_add(7);
            c.f.y = c.f.y.wrapping_sub(3);
        }
        if kind == Kind::Scene && self.scenario % 17 == 0 {
            frame.drag.component = None;
        }
        Ok(match kind {
            Kind::GraphicReady => {
                Reply::Bool((reference(&self.nodes, c.as_ref()) + self.scenario / 7) % 3 != 0)
            }
            Kind::FramebufferEnabled => Reply::Bool(self.scenario & 1 != 0),
            Kind::Streaming => Reply::Bool(self.scenario & 2 != 0),
            Kind::StreamReady => Reply::Bool(self.scenario & 4 != 0),
            Kind::Preview => Reply::Size(self.preview),
            _ => Reply::Unit,
        })
    }
}
fn ints(out: &mut Vec<u8>, a: &[i32]) {
    for v in a {
        int(out, *v);
    }
}
fn fixture(s: i32) -> (Store, State, Frame, Capture, Vec<Array>) {
    let mut store = Store::default();
    let mut state = State::default();
    let mut frame = Frame::default();
    state.life.top = 1;
    state.layout.canvas = [320, 240];
    state.layout.debug_bounds = s & 1 != 0;
    state.life.game_screen_enabled = s & 2 != 0;
    frame.cycle = s.wrapping_mul(1234567);
    frame.last_cycle = -2;
    frame.draw_mode = [0, 1, 2, 3, -1][s as usize % 5];
    state.debug_visible = [s & 4 != 0, s & 8 != 0];
    frame.scene_state = [0, 3, 2][(s as usize / 15) % 3];
    frame.client_state = if s & 16 != 0 { 18 } else { 0 };
    frame.count = if s % 97 == 0 { 113 } else { 3 };
    frame.fade = Fade {
        start_cycle: frame.cycle - 2,
        end_cycle: frame.cycle + [0, 4, 8][s as usize % 3],
        start: [0, 255, 40, 20],
        end: [s % 400, 10, 230, -1],
    };
    let mut nodes = vec![];
    for n in 0..12 {
        let mut c = Component::default();
        let f = &mut c.f;
        f.parentlayer = if n == 10 {
            131072
        } else if n == 11 {
            65536
        } else {
            65536 + n
        };
        f.id = if n == 11 { 0 } else { -1 };
        f.r#type = [0, 3, 4, 5, 6, 9, 5, 3, 0, 3, 3, 3][n as usize];
        f.layer = match n {
            1..=4 | 11 => 65536,
            9 => 65544,
            _ => -1,
        };
        f.x = if n == 0 { 10 } else { n * 3 };
        f.y = if n == 0 { 20 } else { n * 5 };
        f.width = if n == 0 {
            220
        } else if n == 8 {
            200
        } else {
            80
        };
        f.height = if n == 0 {
            180
        } else if n == 8 {
            120
        } else {
            36
        };
        if n == 5 {
            f.width = [-3, 0, 17, i32::MAX][s as usize % 4];
            f.height = [0, 1, -1, 20][s as usize % 4];
        }
        f.scrollx = s % 17;
        f.scrolly = s % 23;
        f.trans = [0, 1, 127, 128, 255, 256, -1, 999][(s + n) as usize % 8];
        f.hide = (s + n) % 7 == 0;
        f.colour = s.wrapping_mul(975321).wrapping_add(n);
        f.fill = (s + n) & 1 != 0;
        f.linewid = [0, 1, 2, 5, -1][s as usize % 5];
        f.linedirection = s & 32 != 0;
        f.dragrenderbehaviour = s % 4;
        if n == 6 {
            f.clientcode = [
                0, 1337, 1403, 1338, 1339, 1400, 1401, 1405, 1406, 1407, 1408, 1409, 1410, 1411,
                328,
            ][s as usize % 15];
            if s % 2 == 0 {
                f.r#type = 2;
            }
        }
        if n == 7 {
            f.clientcode = 1405;
        }
        if n == 8 {
            f.clientcode = 1407;
        }
        c.default_active[0] = if (s + n) % 4 == 0 { 1 << 23 } else { 0 };
        nodes.push(Rc::new(RefCell::new(c)));
    }
    let main = Interface::new(nodes[..10].iter().cloned().map(Some).collect());
    let sorted = Rc::new(RefCell::new(
        main.borrow()
            .components
            .borrow()
            .iter()
            .cloned()
            .rev()
            .collect(),
    ));
    if s & 64 != 0 {
        main.borrow_mut().sorted = Some(sorted.clone());
    }
    let dynamic = Rc::new(RefCell::new(vec![Some(nodes[11].clone()), None]));
    nodes[0].borrow_mut().sorted = Some(dynamic.clone());
    nodes[0].borrow_mut().children = Some(Rc::new(RefCell::new(vec![None])));
    let sub = Interface::new(vec![Some(nodes[10].clone())]);
    let arrays = vec![
        main.borrow().components.clone(),
        sorted,
        dynamic,
        sub.borrow().components.clone(),
    ];
    store.interfaces.insert(1, main);
    store.interfaces.insert(2, sub);
    state
        .life
        .subs
        .put(crate::ui_lifecycle::Sub::new(2, 1, None), 65536);
    if s & 128 != 0 {
        state
            .layout
            .active_masks
            .insert((65537i64 << 32) - 1, 1 << 23);
    }
    for i in 0..114 {
        frame.requested[i] = (s + i as i32) & 1 != 0;
        state.life.redraw[i] = (s + i as i32) % 3 == 0;
        frame.bounds[i] = [i as i32 * 3, 0, 100, 150];
    }
    frame.drag.component = if s % 6 == 0 {
        None
    } else {
        Some(nodes[[1, 2, 5, 6, 11][s as usize % 5]].clone())
    };
    frame.drag.layer = if s & 256 == 0 {
        Some(nodes[0].clone())
    } else {
        None
    };
    frame.drag.default_layer = if s & 256 == 0 {
        Some(nodes[0].clone())
    } else {
        None
    };
    if s % 11 == 0 {
        frame.drag.default_layer = None;
        frame.drag.layer = Some(nodes[0].clone());
    }
    frame.drag.active = s & 2 != 0;
    frame.drag.parent_ready = s & 4 != 0;
    frame.drag.mouse = [s % 500 - 90, s % 300 - 20];
    frame.drag.press = [23, 17];
    frame.drag.bounds = [5, 7, 200, 170];
    let capture = Capture {
        scenario: s,
        nodes,
        calls: vec![],
        fail: if s % 13 == 0 { s % 21 } else { -1 },
        preview: if s & 8 != 0 {
            Some([[800, 600], [0, 0], [-1, 20], [120, 400]][s as usize % 4])
        } else {
            None
        },
    };
    (store, state, frame, capture, arrays)
}
#[test]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-draw");
    let out = scratch.dir().to_path_buf();
    let mut input = vec![];
    let mut output = vec![];
    let mut offsets = String::new();
    int(&mut input, FIELDS.len() as i32);
    for f in FIELDS {
        text(&mut input, Some(&f.encode_utf16().collect::<Vec<_>>()));
    }
    let scenarios = 768;
    int(&mut input, scenarios);
    for s in 0..scenarios {
        let (mut store, mut state, mut frame, mut backend, arrays) = fixture(s);
        ints(
            &mut input,
            &[s, backend.fail, backend.preview.is_some() as i32],
        );
        if let Some(p) = backend.preview {
            ints(&mut input, &p);
        }
        int(&mut input, backend.nodes.len() as i32);
        for c in &backend.nodes {
            ints(&mut input, &fields(c));
            int(&mut input, c.borrow().default_active[0]);
        }
        int(&mut input, arrays.len() as i32);
        for a in &arrays {
            int(&mut input, a.borrow().len() as i32);
            for c in a.borrow().iter() {
                int(&mut input, reference(&backend.nodes, c.as_ref()));
            }
        }
        // Main components/sort, sub components, and root dynamic sorted array.
        ints(&mut input, &[0, if s & 64 != 0 { 1 } else { -1 }, 3, 2]);
        ints(
            &mut input,
            &[
                state.layout.canvas[0],
                state.layout.canvas[1],
                state.layout.debug_bounds as i32,
                state.life.game_screen_enabled as i32,
                frame.cycle,
                frame.last_cycle,
                frame.count as i32,
                frame.draw_mode,
                state.debug_visible[0] as i32,
                state.debug_visible[1] as i32,
                frame.scene_state,
                frame.client_state,
            ],
        );
        for i in 0..114 {
            ints(
                &mut input,
                &[frame.requested[i] as i32, state.life.redraw[i] as i32],
            );
            ints(&mut input, &frame.bounds[i]);
        }
        for c in [
            &frame.drag.component,
            &frame.drag.layer,
            &frame.drag.default_layer,
        ] {
            int(&mut input, reference(&backend.nodes, c.as_ref()));
        }
        ints(
            &mut input,
            &[frame.drag.active as i32, frame.drag.parent_ready as i32],
        );
        ints(&mut input, &frame.drag.mouse);
        ints(&mut input, &frame.drag.press);
        ints(&mut input, &frame.drag.bounds);
        ints(&mut input, &[frame.fade.start_cycle, frame.fade.end_cycle]);
        ints(&mut input, &frame.fade.start);
        ints(&mut input, &frame.fade.end);
        int(&mut input, state.layout.active_masks.len() as i32);
        for (key, mask) in &state.layout.active_masks {
            input.extend(key.to_be_bytes());
            int(&mut input, *mask);
        }
        let actions = vec![
            [0, frame.cycle, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [4, s % 12, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [5, 13, 17, 150, 200],
            [0, frame.cycle.wrapping_add(1), 0, 0, 0],
            [1, 0, 0, 0, 0],
            [2, 99, -1, 0, 0],
            [2, 99, 2, 0, 0],
            [3, 0, 0, 0, 0],
            [6, 0, 0, 0, 0],
        ];
        int(&mut input, actions.len() as i32);
        for (n, a) in actions.iter().enumerate() {
            ints(&mut input, a);
            backend.calls.clear();
            let result = match a[0] {
                0 => frame.promote(&mut store, &mut state, a[1]),
                1 => frame.root(&mut store, &mut state, &mut backend),
                2 => frame.interface(
                    &mut store,
                    &mut state,
                    &mut backend,
                    a[1],
                    View {
                        clip: [0, 0, 320, 240],
                        offset: [7, 9],
                        top: a[2],
                    },
                ),
                3 => frame.components(
                    &mut store,
                    &mut state,
                    &mut backend,
                    &arrays[0],
                    -1,
                    View {
                        clip: [15, 21, 160, 100],
                        offset: [-17, 4],
                        top: 0,
                    },
                ),
                4 => frame.component_updated(&mut state, &backend.nodes[a[1] as usize]),
                5 => {
                    frame.request_at(&mut state, [a[1], a[2], a[3], a[4]]);
                    Ok(())
                }
                6 => {
                    frame.count = 113;
                    frame.root(&mut store, &mut state, &mut backend)
                }
                _ => unreachable!(),
            };
            offsets.push_str(&format!("{} {s} {n} {a:?}\n", output.len()));
            output.push(result.is_ok() as u8);
            int(&mut output, backend.calls.len() as i32);
            for call in &backend.calls {
                ints(
                    &mut output,
                    &[
                        call.kind as i32,
                        // The world-map draws take no component in the
                        // recorded client, whatever the port passes along.
                        if matches!(call.kind, Kind::WorldMap | Kind::WorldMapOverview) {
                            -1
                        } else {
                            reference(&backend.nodes, call.component.as_ref())
                        },
                        call.args.len() as i32,
                    ],
                );
                ints(&mut output, &call.args);
            }
            ints(
                &mut output,
                &[frame.cycle, frame.last_cycle, frame.count as i32],
            );
            for i in 0..114 {
                ints(
                    &mut output,
                    &[frame.requested[i] as i32, state.life.redraw[i] as i32],
                );
                ints(&mut output, &frame.bounds[i]);
            }
            for c in &backend.nodes {
                ints(&mut output, &fields(c));
            }
            int(
                &mut output,
                reference(&backend.nodes, frame.drag.component.as_ref()),
            );
            int(
                &mut output,
                frame.deferred.as_ref().map_or(-1, |d| {
                    arrays.iter().position(|a| Rc::ptr_eq(a, &d.array)).unwrap() as i32
                }),
            );
            ints(
                &mut output,
                &frame.deferred.as_ref().map_or([0, 0], |d| d.offset),
            );
            int(
                &mut output,
                store.interfaces[&1].borrow().sorted.is_some() as i32,
            );
        }
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(out.join("rust-offsets.txt"), offsets)?;
    std::fs::write(
        out.join("counts.txt"),
        format!("{scenarios} {}\n", scenarios * 11),
    )?;
    scratch.finish("ui-draw", &[("rust.bin", "recording")]);
    Ok(())
}

#[test]
fn queued_invalidation_precedes_draw_cycle_promotion() -> anyhow::Result<()> {
    let mut store = Store::default();
    let mut state = State::default();
    let mut frame = Frame::default();
    let c = Rc::new(RefCell::new(Component::default()));
    c.borrow_mut().f.topLevelIndex = 0;
    c.borrow_mut().f.lastDrawCycle = 7;
    frame.last_cycle = 7;
    frame.count = 1;
    state.life.top = 1;
    store.updated.push(c);
    frame.promote(&mut store, &mut state, 8)?;
    assert!(frame.requested[0]);
    assert!(!state.life.redraw[0]);
    assert!(store.updated.is_empty());
    assert_eq!(frame.last_cycle, 8);
    assert_eq!(frame.count, 0);
    Ok(())
}
