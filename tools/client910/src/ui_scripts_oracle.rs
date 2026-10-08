use crate::{
    ui_components_oracle::{int, text},
    ui_scripts::{Scripts, Source},
};
use native910::{
    opcode::OpcodeBook,
    script::{CompiledScript, Counts, Instruction, Operand, VarRef},
    vars::VarScope,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
#[derive(Default)]
struct Data {
    files: BTreeMap<i32, Vec<u8>>,
    trace: Vec<i32>,
    fail: i32,
    bind_fail: bool,
}
struct Memory(Rc<RefCell<Data>>);
impl Source for Memory {
    fn file(&mut self, id: i32) -> anyhow::Result<Option<Vec<u8>>> {
        let mut d = self.0.borrow_mut();
        d.trace.extend([0, id]);
        anyhow::ensure!(d.fail != id, "source failure");
        Ok(d.files.get(&id).cloned())
    }
}
fn script(var: bool) -> CompiledScript {
    CompiledScript {
        name: Some("cache-fixture".into()),
        locals: Counts {
            int: 2,
            obj: 1,
            long: 0,
        },
        args: Counts::default(),
        code: vec![
            Instruction {
                opcode: 0,
                command: if var { "push_var" } else { "push_constant_int" }.into(),
                operand: if var {
                    Operand::VarRef(VarRef {
                        domain: VarScope::Client,
                        id: 0,
                        transmog: false,
                    })
                } else {
                    Operand::Int(12345)
                },
            },
            Instruction {
                opcode: 0,
                command: "return".into(),
                operand: Operand::Byte(0),
            },
        ],
    }
}
#[test]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-scripts");
    let out = scratch.dir().to_path_buf();
    let book = OpcodeBook::embedded()?;
    let raw = [
        vec![],
        vec![0],
        vec![0, 1],
        native910::script::encode_script(&script(false), &book)?,
        native910::script::encode_script(&script(true), &book)?,
    ];
    let data = Rc::new(RefCell::new(Data {
        fail: -999,
        ..Default::default()
    }));
    let scripts = Scripts::new(Box::new(Memory(data.clone())))?;
    let mut input = vec![];
    let mut output = vec![];
    for b in &raw {
        int(&mut input, b.len() as i32);
        input.extend(b);
    }
    let mut a = vec![];
    for round in 0..4 {
        a.push([2, 0, 0]);
        for id in 0..192 {
            a.push([1, id, 3]);
        }
        for id in 0..150 {
            a.push([0, id, 0]);
        }
        for id in [30, 100, 149, 30, 0, 1, 2, 150, 151, 152] {
            a.push([0, id, 0]);
        }
        a.extend([
            [1, 100, -1],
            [0, 100, 0],
            [2, 0, 0],
            [0, 100, 0],
            [1, 100, 0],
            [0, 100, 0],
            [0, 100, 0],
            [1, 100, 1],
            [0, 100, 0],
            [1, 100, 2],
            [0, 100, 0],
            [0, 100, 0],
            [1, 100, 3],
            [0, 100, 0],
            [0, 100, 0],
            [0, -1, 0],
            [0, i32::MIN, 0],
            [1, 200, 4],
            [3, 1, 0],
            [0, 200, 0],
            [0, 200, 0],
            [3, 0, 0],
            [0, 200, 0],
            [3, 1, 0],
            [0, 200, 0],
            [2, 0, 0],
            [0, 200, 0],
            [3, 0, 0],
            [0, 200, 0],
            [4, 201, 0],
            [0, 201, 0],
            [4, -999, 0],
            [1, 201, 3],
            [0, 201, 0],
            [0, 201, 0],
            [1, 202, round],
            [0, 202, 0],
        ]);
    }
    int(&mut input, a.len() as i32);
    let mut ids: Vec<Rc<CompiledScript>> = vec![];
    let mut offsets = String::new();
    for (step, a) in a.iter().enumerate() {
        for v in a {
            int(&mut input, *v);
        }
        let mut result = None;
        let ok = match a[0] {
            0 => {
                match scripts.get(a[1], |s| {
                    for ins in &s.code {
                        if let Operand::VarRef(v) = &ins.operand {
                            let mut d = data.borrow_mut();
                            d.trace.extend([1, v.domain as i32, v.id as i32]);
                            anyhow::ensure!(!d.bind_fail, "binding failure");
                        }
                    }
                    Ok(())
                }) {
                    Ok(s) => {
                        result = s;
                        true
                    }
                    Err(_) => false,
                }
            }
            1 => {
                if a[2] < 0 {
                    data.borrow_mut().files.remove(&a[1]);
                } else {
                    data.borrow_mut()
                        .files
                        .insert(a[1], raw[a[2] as usize].clone());
                }
                true
            }
            2 => {
                scripts.clear();
                true
            }
            3 => {
                data.borrow_mut().bind_fail = a[1] != 0;
                true
            }
            4 => {
                data.borrow_mut().fail = a[1];
                true
            }
            _ => unreachable!(),
        };
        offsets.push_str(&format!("{} {step} {a:?}\n", output.len()));
        output.push(ok as u8);
        if ok {
            let id = if let Some(s) = &result {
                if let Some(id) = ids.iter().position(|v| Rc::ptr_eq(v, s)) {
                    id as i32
                } else {
                    ids.push(s.clone());
                    ids.len() as i32 - 1
                }
            } else {
                -1
            };
            int(&mut output, id);
            if let Some(s) = result {
                text(
                    &mut output,
                    s.name
                        .as_ref()
                        .map(|s| s.encode_utf16().collect::<Vec<_>>())
                        .as_deref(),
                );
                for v in [
                    s.locals.int,
                    s.locals.obj,
                    s.locals.long,
                    s.args.int,
                    s.args.obj,
                    s.args.long,
                ] {
                    int(&mut output, v as i32);
                }
                int(&mut output, s.code.len() as i32);
                for ins in &s.code {
                    int(&mut output, ins.opcode as i32);
                    int(
                        &mut output,
                        match ins.operand {
                            Operand::Int(v) => v,
                            Operand::Byte(v) => v as i32,
                            Operand::VarRef(ref v) => v.transmog as i32,
                            _ => unreachable!(),
                        },
                    );
                }
            }
        }
        let order = scripts.order();
        int(&mut output, 128 - order.len() as i32);
        int(&mut output, order.len() as i32);
        for id in order {
            int(&mut output, id);
        }
        let mut d = data.borrow_mut();
        int(&mut output, d.trace.len() as i32);
        for v in d.trace.drain(..) {
            int(&mut output, v);
        }
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(out.join("rust-offsets.txt"), offsets)?;
    std::fs::write(out.join("count.txt"), a.len().to_string())?;
    scratch.finish("ui-scripts", &[("rust.bin", "recording")]);
    Ok(())
}

struct Resolver<'a>(&'a Scripts);
impl native910::vm::ScriptProvider for Resolver<'_> {
    fn resolve(&self, id: i32) -> native910::vm::VmResult<Option<CompiledScript>> {
        self.0
            .get(id, |_| Ok(()))
            .map(|s| s.map(|s| (*s).clone()))
            .map_err(|e| native910::vm::VmError::TrapFailed {
                command: format!("load script {id}"),
                reason: format!("{e:#}"),
            })
    }
}
#[test]
fn callees_load_only_when_called_and_decode_errors_are_not_missing() -> anyhow::Result<()> {
    use native910::vm::{ScriptProvider, Vm, VmError};
    let book = OpcodeBook::embedded()?;
    let data = Rc::new(RefCell::new(Data {
        fail: -999,
        ..Default::default()
    }));
    for (id, target) in [(42, 2), (43, 1)] {
        let program = CompiledScript {
            name: None,
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                Instruction {
                    opcode: 0,
                    command: "branch".into(),
                    operand: Operand::Branch(target),
                },
                Instruction {
                    opcode: 0,
                    command: "gosub_with_params".into(),
                    operand: Operand::Script(201),
                },
                Instruction {
                    opcode: 0,
                    command: "return".into(),
                    operand: Operand::Byte(0),
                },
            ],
        };
        data.borrow_mut()
            .files
            .insert(id, native910::script::encode_script(&program, &book)?);
    }
    data.borrow_mut().files.insert(201, vec![0, 1]);
    let scripts = Scripts::new(Box::new(Memory(data.clone())))?;
    let provider = Resolver(&scripts);
    let mut engine = native910::runtime::RuntimeHost::default();
    let first = provider.resolve(42)?.unwrap();
    assert!(Vm::new(&mut engine, &provider).execute(&first, &[]).is_ok());
    assert_eq!(data.borrow().trace, [0, 42]);
    let second = provider.resolve(43)?.unwrap();
    assert!(
        matches!(Vm::new(&mut engine,&provider).execute(&second,&[]),Err(VmError::TrapFailed{command,..}) if command=="load script 201")
    );
    assert_eq!(data.borrow().trace, [0, 42, 0, 43, 0, 201]);
    assert_eq!(scripts.order(), [42, 43]);
    Ok(())
}
#[test]
fn root_load_failure_precedes_context_reservation() -> anyhow::Result<()> {
    use crate::{
        ui_changes::Changes,
        ui_components::{Arg, Store},
        ui_hook_host::{Domains, Runner},
        ui_hooks::{Executor, Pool, Request},
        ui_properties::State,
    };
    let data = Rc::new(RefCell::new(Data {
        fail: -999,
        ..Default::default()
    }));
    data.borrow_mut().files.insert(44, vec![0, 1]);
    data.borrow_mut().files.insert(45, vec![0]);
    let scripts = Scripts::new(Box::new(Memory(data)))?;
    let provider = Resolver(&scripts);
    let mut pool = Pool::default();
    let mut engine = native910::runtime::RuntimeHost::default();
    let mut changes = Changes::default();
    let mut now = || 0;
    let mut store = Store::default();
    let mut state = State::default();
    {
        let mut runner = Runner {
            pool: &mut pool,
            provider: &provider,
            engine: &mut engine,
            domains: Domains::Plain {
                changes: &mut changes,
                now: &mut now,
            },
            executions: vec![],
            missing: vec![],
        };
        assert!(runner
            .run(
                &mut store,
                &mut state,
                Request {
                    args: Some(vec![Arg::Int(44)]),
                    ..Default::default()
                },
                100
            )
            .is_err());
        assert!(runner.missing.is_empty());
        runner.run(
            &mut store,
            &mut state,
            Request {
                args: Some(vec![Arg::Int(45)]),
                ..Default::default()
            },
            100,
        )?;
        assert_eq!(runner.missing, [45]);
        assert!(runner.executions.is_empty());
    }
    assert!(pool.contexts.is_empty());
    assert_eq!(pool.used, 0);
    assert!(scripts.order().is_empty());
    Ok(())
}
