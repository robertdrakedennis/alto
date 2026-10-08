use crate::{
    ui_components::{self as c, Arg, Component, Interface, InterfaceRef, Ref, Store},
    ui_components_oracle::{component, int, text},
    ui_hook_host::{Domains, Runner},
    ui_hooks::{self as h, Executor, Pool, Request},
    ui_properties::State,
};
use native910::{
    opcode::OpcodeBook,
    script::{CompiledScript, Counts, Instruction, Operand},
    vm::Host,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
fn ins(command: &str, operand: Operand) -> Instruction {
    Instruction {
        opcode: 0,
        command: command.into(),
        operand,
    }
}
fn push(v: i32) -> Instruction {
    ins("push_constant_int", Operand::Int(v))
}
fn program(id: i32, locals: Counts, mut code: Vec<Instruction>) -> CompiledScript {
    code.push(ins("return", Operand::Byte(0)));
    CompiledScript {
        name: Some(id.to_string()),
        locals,
        args: Counts::default(),
        code,
    }
}
fn scripts() -> HashMap<i32, CompiledScript> {
    let mut p = HashMap::new();
    p.insert(
        1,
        program(
            1,
            Counts {
                int: 20,
                obj: 4,
                long: 3,
            },
            vec![],
        ),
    );
    p.insert(
        2,
        program(
            2,
            Counts::default(),
            vec![
                push(65536),
                push(3),
                push(0),
                ins("cc_create", Operand::Byte(0)),
                push(11),
                ins("cc_settrans", Operand::Byte(0)),
            ],
        ),
    );
    p.insert(
        3,
        program(
            3,
            Counts {
                int: 1,
                ..Default::default()
            },
            vec![
                ins("push_int_local", Operand::Local(0)),
                ins("cc_settrans", Operand::Byte(0)),
            ],
        ),
    );
    p.insert(
        4,
        program(
            4,
            Counts::default(),
            vec![ins("push_constant_string", Operand::Long(i64::MIN + 123))],
        ),
    );
    p.insert(
        5,
        program(
            5,
            Counts {
                long: 1,
                ..Default::default()
            },
            vec![ins("pop_long_local", Operand::Local(0))],
        ),
    );
    p.insert(
        6,
        program(
            6,
            Counts::default(),
            vec![
                push(3),
                ins("define_array", Operand::Array(0)),
                push(1),
                push(99),
                ins("pop_array_int", Operand::Array(0)),
            ],
        ),
    );
    p.insert(
        7,
        program(
            7,
            Counts {
                int: 1,
                ..Default::default()
            },
            vec![
                push(1),
                ins("push_array_int", Operand::Array(0)),
                ins("pop_int_local", Operand::Local(0)),
            ],
        ),
    );
    p.insert(
        8,
        program(
            8,
            Counts::default(),
            vec![push(3), ins("define_array", Operand::Array(0))],
        ),
    );
    p.insert(
        9,
        program(
            9,
            Counts::default(),
            vec![ins("branch", Operand::Branch(0))],
        ),
    );
    p.insert(10, program(10, Counts::default(), vec![]));
    p.insert(
        30,
        program(
            30,
            Counts {
                int: 1,
                ..Default::default()
            },
            vec![
                ins("push_int_local", Operand::Local(0)),
                ins("if_clearscripthooks", Operand::Byte(0)),
            ],
        ),
    );
    p.insert(
        31,
        program(
            31,
            Counts::default(),
            vec![push(65536), ins("if_callonresize", Operand::Byte(0))],
        ),
    );
    p.insert(
        32,
        program(
            32,
            Counts::default(),
            vec![push(65536), ins("cc_deleteall", Operand::Byte(0))],
        ),
    );
    p.insert(
        33,
        program(
            33,
            Counts::default(),
            vec![ins("cc_callonresize", Operand::Byte(0))],
        ),
    );
    for (n, (name, count)) in crate::ui_properties::DISCARDS.iter().enumerate() {
        let mut code = vec![push(123)];
        code.extend((0..*count).map(|_| push(i32::MIN)));
        code.push(ins(name, Operand::Byte(0)));
        p.insert(
            100 + n as i32,
            program(100 + n as i32, Counts::default(), code),
        );
    }
    for (n, name) in [
        "cc_setonverticalswipe",
        "if_setonverticalswipe",
        "cc_setonhorizontalswipe",
        "if_setonhorizontalswipe",
        "cc_setondragcomplete_alias",
        "if_setondragcomplete_alias",
    ]
    .iter()
    .enumerate()
    {
        let mut code = vec![
            push(55),
            push(-1),
            push(78),
            ins("push_constant_string", Operand::Str("i".into())),
        ];
        if name.starts_with("if_") {
            code.push(push(65536));
        }
        code.push(ins(name, Operand::Byte(0)));
        p.insert(
            200 + n as i32,
            program(200 + n as i32, Counts::default(), code),
        );
    }
    p
}
fn cid(nodes: &mut Vec<Ref>, c: Option<&Ref>) -> i32 {
    c.map_or(-1, |c| {
        if let Some(n) = nodes.iter().position(|v| Rc::ptr_eq(v, c)) {
            n as i32
        } else {
            nodes.push(c.clone());
            nodes.len() as i32 - 1
        }
    })
}
fn itf(store: &Store, i: Option<&InterfaceRef>) -> i32 {
    i.map_or(-1, |i| {
        *store
            .interfaces
            .iter()
            .find(|(_, v)| Rc::ptr_eq(v, i))
            .unwrap()
            .0
    })
}
fn array(o: &mut Vec<u8>, nodes: &mut Vec<Ref>, a: Option<&c::Array>) {
    match a {
        None => int(o, -1),
        Some(a) => {
            int(o, a.borrow().len() as i32);
            for c in a.borrow().iter() {
                int(o, cid(nodes, c.as_ref()));
            }
        }
    }
}
fn ints(o: &mut Vec<u8>, a: &[i32]) {
    int(o, a.len() as i32);
    for v in a {
        int(o, *v);
    }
}
fn longs(o: &mut Vec<u8>, a: &[i64]) {
    int(o, a.len() as i32);
    for v in a {
        o.extend(v.to_be_bytes());
    }
}
fn fixture() -> (Store, State, Vec<Ref>) {
    let mut store = Store::default();
    let mut state = State::default();
    let mut nodes = vec![];
    for (p, k) in [
        (65536, 0),
        (65537, 3),
        (65538, 3),
        (131072, 3),
        (131073, 3),
        (65536, 3),
        (65536, 3),
    ] {
        let mut c = Component::default();
        c.f.parentlayer = p;
        c.f.r#type = k;
        nodes.push(Rc::new(RefCell::new(c)));
    }
    for (n, node) in nodes.iter().enumerate() {
        let mut c = node.borrow_mut();
        for h in ["onload", "ondialogabort", "onsubchange"] {
            c.hooks
                .insert(h, vec![Arg::Int(1), Arg::Int(n as i32 + 10)]);
        }
        c.f.hashook = true;
    }
    nodes[0]
        .borrow_mut()
        .hooks
        .insert("onresize", vec![Arg::Int(31)]);
    nodes[0]
        .borrow_mut()
        .hooks
        .insert("onload", vec![Arg::Int(30), Arg::Int(65537)]);
    nodes[2]
        .borrow_mut()
        .hooks
        .insert("onload", vec![Arg::Int(4)]);
    for (n, id) in [(5, 0), (6, 1)] {
        let mut c = nodes[n].borrow_mut();
        c.f.id = id;
        c.f.layer = 65536;
    }
    nodes[0].borrow_mut().children =
        Some(Rc::new(RefCell::new(vec![None, Some(nodes[6].clone())])));
    nodes[0].borrow_mut().sorted = Some(Rc::new(RefCell::new(vec![
        Some(nodes[5].clone()),
        Some(nodes[6].clone()),
    ])));
    store.interfaces.insert(
        1,
        Interface::new(nodes[0..3].iter().cloned().map(Some).collect()),
    );
    store.interfaces.insert(
        2,
        Interface::new(nodes[3..5].iter().cloned().map(Some).collect()),
    );
    state.layout.subs.push((65536, 2));
    (store, state, nodes)
}
fn snapshot(o: &mut Vec<u8>, store: &Store, state: &State, pool: &Pool, nodes: &mut Vec<Ref>) {
    int(o, state.layout.hooks.len() as i32);
    for r in &state.layout.hooks {
        int(o, cid(nodes, r.component.as_ref()));
        int(o, r.nested_count);
        let args = r.args.as_ref().unwrap();
        int(o, args.len() as i32);
        for a in args {
            let Arg::Int(v) = a else {
                panic!("unexpected fixture hook arg")
            };
            int(o, *v);
        }
    }
    int(o, pool.used as i32);
    int(o, pool.contexts.len() as i32);
    for c in &pool.contexts {
        int(o, c.nested_count);
        ints(o, &c.locals.ints);
        int(o, c.locals.strings.len() as i32);
        for s in &c.locals.strings {
            text(o, s.as_deref());
        }
        longs(o, &c.locals.longs);
        longs(o, &c.longs);
        for a in &c.active {
            int(o, cid(nodes, a.component.as_ref()));
            int(o, itf(store, a.interface.as_ref()));
        }
        for a in &c.arrays.values {
            ints(o, a);
        }
    }
    for g in [1, 2] {
        array(o, nodes, Some(&store.interfaces[&g].borrow().components));
    }
    let mut data = vec![];
    let mut n = 0;
    while n < nodes.len() {
        let c = nodes[n].borrow().clone();
        n += 1;
        component(&mut data, &c);
        array(&mut data, nodes, c.children.as_ref());
        array(&mut data, nodes, c.sorted.as_ref());
        data.push(match (&c.children, &c.sorted) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        } as u8);
    }
    int(o, n as i32);
    o.extend(data);
}
pub(crate) struct Strict;
impl Host for Strict {
    fn var_get(
        &mut self,
        _: native910::vars::VarScope,
        _: u16,
        _: bool,
    ) -> native910::vm::VmResult<native910::vm::Value> {
        panic!("unexpected variable read")
    }
    fn var_set(
        &mut self,
        _: native910::vars::VarScope,
        _: u16,
        _: bool,
        _: native910::vm::Value,
    ) -> native910::vm::VmResult<()> {
        panic!("unexpected variable write")
    }
    fn varbit_get(&mut self, _: u16, _: bool) -> native910::vm::VmResult<i32> {
        panic!("unexpected varbit")
    }
    fn varbit_set(&mut self, _: u16, _: bool, _: i32) -> native910::vm::VmResult<()> {
        panic!("unexpected varbit")
    }
    fn array_define(&mut self, _: i32, _: usize) -> native910::vm::VmResult<()> {
        panic!("array escaped context")
    }
    fn array_len(&mut self, _: i32) -> native910::vm::VmResult<usize> {
        panic!("array escaped context")
    }
    fn array_get(&mut self, _: i32, _: i32) -> native910::vm::VmResult<i32> {
        panic!("array escaped context")
    }
    fn array_set(&mut self, _: i32, _: i32, _: i32) -> native910::vm::VmResult<()> {
        panic!("array escaped context")
    }
    fn trap_context(
        &mut self,
        c: &native910::vm::InstructionContext<'_>,
        _: &mut Vec<i32>,
        _: &mut Vec<String>,
        _: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        panic!("unhandled hook command {}", c.command)
    }
}
#[test]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-hooks");
    let out = scratch.dir().to_path_buf();
    let scripts = scripts();
    let book = OpcodeBook::embedded()?;
    let mut input = vec![];
    let mut output = vec![];
    let names = crate::ui_component_fields::FIELD_NAMES;
    int(&mut input, names.len() as i32);
    for n in names {
        text(&mut input, Some(&n.encode_utf16().collect::<Vec<_>>()));
    }
    let mut ids = scripts.keys().copied().collect::<Vec<_>>();
    ids.sort();
    int(&mut input, ids.len() as i32);
    for id in ids {
        let b = native910::script::encode_script(&scripts[&id], &book)?;
        int(&mut input, id);
        int(&mut input, b.len() as i32);
        input.extend(b);
    }
    let mut actions = vec![];
    for round in 0..12 {
        actions.push((0, 0, 0, round));
        for id in (100..100 + crate::ui_properties::DISCARDS.len() as i32).chain(200..206) {
            actions.push((1, id, 0, 100));
        }
        actions.extend([
            (1, 33, 0, 100),
            (1, 33, 10, 100),
            (1, 31, 9, 100),
            (4, 0, 0, 0),
            (1, 31, 10, 100),
            (1, 31, 0, 100),
            (4, 0, 0, 0),
            (5, 1, 5, 0),
            (5, 1, 6, 0),
            (4, 0, 0, 0),
            (5, 1, 6, 0),
            (1, 32, 0, 100),
            (4, 0, 0, 0),
            (0, 0, 0, 0),
        ]);
        for (id, variant, limit) in [
            (1, 0, 100),
            (1, 1, 100),
            (2, 0, 100),
            (3, 2, 100),
            (4, 0, 100),
            (5, 0, 100),
            (6, 0, 100),
            (7, 0, 100),
            (8, 0, 100),
            (7, 0, 100),
            (9, 0, 0),
            (9, 0, 8),
            (999, 0, 100),
            (1, 3, 100),
            (1, 4, 100),
        ] {
            actions.push((1, id, variant, limit));
        }
        actions.extend([
            (2, 1, 0, 0),
            (3, 1, 1, 0),
            (3, 1, 0, 0),
            (2, 2, 0, 0),
            (1, 10, 2, 100),
            (1, 1, 0, 100),
            (1, 3, 2, 100),
        ]);
    }
    int(&mut input, actions.len() as i32);
    let (mut store, mut state, mut nodes) = fixture();
    let mut pool = Pool::default();
    let mut changes = crate::ui_changes::Changes::default();
    let mut engine = Strict;
    let mut offsets = String::new();
    for (step, &(op, id, variant, limit)) in actions.iter().enumerate() {
        for v in [op, id, variant, limit] {
            int(&mut input, v);
        }
        if op == 0 {
            (store, state, nodes) = fixture();
            pool = Pool::default();
        }
        let mut r = Request {
            component: Some(nodes[1].clone()),
            drop: Some(nodes[6].clone()),
            mouse: [31, 47],
            opindex: 2,
            key: 65,
            keychar: 97,
            opbase: Some("operate😀".encode_utf16().collect()),
            nested_count: 3,
            args: Some(vec![Arg::Int(id)]),
            ..Default::default()
        };
        if op == 1 {
            match variant {
                0 if id == 1 => {
                    let a = r.args.as_mut().unwrap();
                    a.extend((-2147483647..=-2147483639).map(Arg::Int));
                    a.extend([
                        Arg::String("event_opbase".encode_utf16().collect()),
                        Arg::Null,
                        Arg::Long(i64::MAX),
                        Arg::String("literal".encode_utf16().collect()),
                    ]);
                }
                1 => {
                    r.mouse = [-2147483646, -2147483645];
                    r.component = None;
                    r.drop = None;
                    r.opbase = None;
                    r.args.as_mut().unwrap().extend([
                        Arg::Int(-2147483647),
                        Arg::String("event_opbase".encode_utf16().collect()),
                    ]);
                }
                2 => r.args.as_mut().unwrap().push(Arg::Int(77)),
                3 => r.args = None,
                4 => r.args = Some(vec![Arg::String(vec![])]),
                _ => {}
            }
        }
        if matches!(variant, 9 | 10) {
            r.nested_count = variant;
        }
        if op == 5 {
            r.component = Some(nodes[variant as usize].clone());
        }
        // Serialize exact requests, including null hook arrays and nullable opbase.
        int(&mut input, cid(&mut nodes, r.component.as_ref()));
        int(&mut input, cid(&mut nodes, r.drop.as_ref()));
        for v in [
            r.mouse[0],
            r.mouse[1],
            r.opindex,
            r.key,
            r.keychar,
            r.nested_count,
        ] {
            int(&mut input, v);
        }
        text(&mut input, r.opbase.as_deref());
        int(&mut input, r.args.as_ref().map_or(-1, |a| a.len() as i32));
        if let Some(a) = &r.args {
            for a in a {
                match a {
                    Arg::Int(v) => {
                        int(&mut input, 0);
                        int(&mut input, *v)
                    }
                    Arg::String(v) => {
                        int(&mut input, 2);
                        text(&mut input, Some(v))
                    }
                    Arg::Long(v) => {
                        int(&mut input, 1);
                        input.extend(v.to_be_bytes())
                    }
                    Arg::Null => int(&mut input, 3),
                }
            }
        }
        let mut now = || 0;
        let mut runner = Runner {
            pool: &mut pool,
            provider: &scripts,
            engine: &mut engine,
            domains: Domains::Plain {
                changes: &mut changes,
                now: &mut now,
            },
            executions: vec![],
            missing: vec![],
        };
        let result = match op {
            0 => Ok(()),
            1 => runner.run(&mut store, &mut state, r, limit as usize),
            2 => h::on_load(&mut store, &mut state, id, None, &mut runner),
            3 => h::immediate(&mut store, &mut state, id, variant, &mut runner),
            4 => runner.drain_main(&mut store, &mut state),
            5 => {
                state.layout.hooks.push_back(r);
                Ok(())
            }
            _ => unreachable!(),
        };
        offsets.push_str(&format!(
            "{} {step} {op} {id} {variant} {limit}\n",
            output.len()
        ));
        output.push(result.is_ok() as u8);
        ints(&mut output, &runner.missing);
        int(&mut output, runner.executions.len() as i32);
        for x in &runner.executions {
            int(&mut output, x.id);
            int(&mut output, x.limit as i32);
            output.push(x.result.is_ok() as u8);
            int(&mut output, x.snapshot.steps as i32);
            if x.result.is_ok() {
                ints(&mut output, &x.snapshot.ints);
                int(&mut output, x.snapshot.strings.len() as i32);
                for s in &x.snapshot.strings {
                    text(
                        &mut output,
                        s.as_deref().map(native910::jstr::units).as_deref(),
                    );
                }
            }
        }
        snapshot(&mut output, &store, &state, &pool, &mut nodes);
    }
    std::fs::write(out.join("hooks-input.bin"), input)?;
    std::fs::write(out.join("rust-hooks.bin"), output)?;
    std::fs::write(out.join("rust-hooks-offsets.txt"), offsets)?;
    std::fs::write(out.join("hooks-count.txt"), actions.len().to_string())?;
    scratch.finish("ui-hooks", &[("rust-hooks.bin", "hooks")]);
    Ok(())
}
