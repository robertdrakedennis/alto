//! Font loading and script execution consume the same resource records.
use crate::{
    font_metrics::Metrics,
    ui_components::{Arg, Store},
    ui_components_oracle::{int, text},
    ui_fonts::{Archive, Font, Fonts, Source},
    ui_hook_host::{Domains, ScriptRun},
    ui_hooks::{Pool, Request},
    ui_properties::State,
};
use native910::{
    opcode::OpcodeBook,
    script::{CompiledScript, Counts, Instruction, Operand},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
#[derive(Default)]
struct Data {
    files: BTreeMap<(Archive, i32), Vec<u8>>,
    trace: Vec<i32>,
    fail: Option<(Archive, i32)>,
}
struct Memory(Rc<RefCell<Data>>);
impl Memory {
    fn file(&self, op: i32, a: Archive, id: i32) -> anyhow::Result<Option<Vec<u8>>> {
        let mut d = self.0.borrow_mut();
        d.trace.extend([op, a as i32, id]);
        anyhow::ensure!(d.fail != Some((a, id)), "injected resource failure");
        Ok(d.files.get(&(a, id)).cloned())
    }
}
impl Source for Memory {
    fn load(&mut self, a: Archive, id: i32) -> anyhow::Result<bool> {
        Ok(self.file(0, a, id)?.is_some())
    }
    fn fetch(&mut self, a: Archive, id: i32) -> anyhow::Result<Option<Vec<u8>>> {
        self.file(1, a, id)
    }
    fn sprite_file(&mut self, id: i32) -> anyhow::Result<Option<Vec<u8>>> {
        self.file(2, Archive::Sprites, id)
    }
}
fn identity<T>(v: &Rc<T>, ids: &mut Vec<Rc<T>>) -> i32 {
    if let Some(n) = ids.iter().position(|i| Rc::ptr_eq(v, i)) {
        n as i32
    } else {
        ids.push(v.clone());
        ids.len() as i32 - 1
    }
}
fn metric(out: &mut Vec<u8>, m: &Rc<Metrics>, ids: &mut Vec<Rc<Metrics>>) {
    for v in [identity(m, ids), m.ascent, m.descent, m.space_width] {
        int(out, v);
    }
}
fn snapshot(out: &mut Vec<u8>, fonts: &Fonts, data: &Rc<RefCell<Data>>) {
    for c in fonts.snapshot() {
        int(out, 20 - c.len() as i32);
        int(out, c.len() as i32);
        for (key, soft, age, present) in c {
            out.extend(key.to_be_bytes());
            out.push(soft as u8);
            out.extend(age.to_be_bytes());
            out.push(present as u8);
        }
    }
    let trace = std::mem::take(&mut data.borrow_mut().trace);
    int(out, trace.len() as i32);
    for v in trace {
        int(out, v);
    }
}
fn ins(n: &str, o: Operand) -> Instruction {
    Instruction {
        opcode: 0,
        command: n.into(),
        operand: o,
    }
}
fn sprite(frames: &[(u16, u16)]) -> Vec<u8> {
    let mut b = vec![];
    for &(w, h) in frames {
        b.push(0);
        b.extend(vec![1; w as usize * h as usize]);
    }
    b.extend([33, 44, 55]);
    b.extend(13u16.to_be_bytes());
    b.extend(17u16.to_be_bytes());
    b.push(1);
    for axis in 0..4 {
        for &(w, h) in frames {
            b.extend(
                match axis {
                    0 => 2u16,
                    1 => 3,
                    2 => w,
                    _ => h,
                }
                .to_be_bytes(),
            );
        }
    }
    b.extend((frames.len() as u16).to_be_bytes());
    b
}
enum Return {
    None,
    Metric(Option<Rc<Metrics>>),
    Font(Option<Rc<Font>>),
    Int(i32),
    Icons(Option<Rc<Vec<crate::font_layout::Image>>>),
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-fonts");
    let out = scratch.dir().to_path_buf();
    let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
    let mut data = Data::default();
    let font_ids = pack.read_archive_index("fontmetrics")?.group_id;
    for &id in &font_ids {
        data.files.insert(
            (Archive::Metrics, id as i32),
            crate::js5_fetch::fetch_file(&pack, "fontmetrics", id)?.unwrap(),
        );
    }
    let installed = Fonts::from_pack(pack.clone(), Some(0))?;
    let defaults = installed.ids.clone().unwrap();
    for &id in &defaults {
        data.files.insert(
            (Archive::Sprites, id),
            crate::js5_fetch::fetch_file(&pack, "sprites", id as u32)?.unwrap(),
        );
    }
    for id in (1000..1064).chain([i32::MIN, 0, 2002]) {
        for a in [Archive::Metrics, Archive::Sprites] {
            data.files
                .insert((a, id), data.files[&(a, defaults[0])].clone());
        }
    }
    data.files.insert((Archive::Sprites, 2000), sprite(&[]));
    data.files.insert((Archive::Sprites, 2001), vec![0]);
    data.files
        .insert((Archive::Sprites, 2100), sprite(&[(2, 3), (4, 5)]));
    data.files
        .insert((Archive::Sprites, 2101), sprite(&[(0, 3), (4, 0)]));
    data.files
        .insert((Archive::Sprites, 2102), vec![0, 0, 0, 2, 0, 3, 128, 0]);
    data.files.insert((Archive::Metrics, 2001), vec![1]);
    // A kerning font makes caught icon-provider failures observably different
    // from a missing icon (which returns zero and resets the previous glyph).
    let mut synthetic = vec![0, 1];
    synthetic.extend([10; 256]);
    synthetic.extend([1; 256]);
    synthetic.extend([0; 256]);
    synthetic.extend([0, 16, 0, 16]);
    synthetic.extend([0; 1024 + 512]);
    synthetic.extend([0, 0, 1, 1, 1]);
    data.files.insert((Archive::Metrics, 2200), synthetic);

    let mut input = vec![];
    let mut output = vec![];
    let mut offsets = String::new();
    int(&mut input, data.files.len() as i32);
    for ((a, id), bytes) in &data.files {
        for v in [*a as i32, *id, bytes.len() as i32] {
            int(&mut input, v);
        }
        input.extend(bytes);
    }
    int(&mut input, defaults.len() as i32);
    for id in &defaults {
        int(&mut input, *id);
    }
    let images = installed.images.images.unwrap();
    int(&mut input, images.len() as i32);
    for im in &images {
        int(&mut input, im.width);
        int(&mut input, im.height);
    }
    let data = Rc::new(RefCell::new(data));
    let mut fonts = Fonts::new(Box::new(Memory(data.clone())), Some(defaults.clone()), None);
    fonts.images.images = Some(images);
    let mut actions = vec![
        [5, 0, 0, 0],
        [4, 0, 0, 0],
        [0, defaults[0], 1, 1],
        [2, 0, 0, 0],
        [4, 0, 0, 0],
        [4, 1, 0, 0],
    ];
    for &id in &defaults {
        actions.extend([[0, id, 1, 1], [1, id, 0, 0], [1, id, 1, 1]]);
    }
    for id in 1000..1040 {
        actions.push([0, id, 1, 1]);
        actions.push([1, id, id & 1, id & 1]);
    }
    for id in [1020, 1000, 1039, 1000, 1021, i32::MIN, 0, i32::MIN] {
        for flag in 0..4 {
            actions.push([0, id, flag & 1, flag >> 1]);
            actions.push([1, id, flag & 1, flag >> 1]);
        }
    }
    actions.extend([
        [7, 0, 0, 0],
        [0, 1000, 0, 1],
        [1, 1000, 0, 0],
        [8, 0, 0, 0],
        [7, -1, 0, 0],
        [9, 0, 0, 0],
        [0, 1000, 1, 1],
        [7, 3, 0, 0],
        [6, 0, 0, 0],
    ]);
    for id in [-1, -2, 9999, 2001] {
        actions.extend([[0, id, 1, 1], [0, id, 0, 0], [1, id, 1, 1], [1, id, 0, 0]]);
    }
    actions.extend([
        [11, 0, 2002, 0],
        [0, 2002, 1, 1],
        [1, 2002, 1, 1],
        [12, 0, 2002, 0],
        [0, 2002, 1, 1],
        [0, 2002, 1, 0],
        [12, -1, 0, 0],
    ]);
    for t in [-1, 0, 0, 1, -1, 1, 2] {
        for id in [2100, 2100, 2101, 2000, 2000, 2001, 2102, 9999] {
            actions.push([10, t, id, 0]);
        }
    }
    for id in 1000..1040 {
        actions.push([10, 2, id, 0]);
    }
    actions.extend([
        [7, 0, 0, 0],
        [13, 1039, 0, 0],
        [8, 0, 0, 0],
        [7, 0, 0, 0],
        [9, 0, 0, 0],
        [13, 1039, 0, 0],
        [6, 0, 0, 0],
    ]);
    actions.extend([
        [3, 0, 0, 0],
        [0, defaults[0], 1, 1],
        [4, 0, 0, 0],
        [11, 0, defaults[1], 0],
        [2, 0, 0, 0],
        [4, 0, 0, 0],
        [0, defaults[0], 1, 1],
        [0, defaults[1], 1, 1],
        [0, defaults[2], 1, 1],
        [14, 0, defaults[1], defaults[0]],
        [2, 0, 0, 0],
        [4, 1, 0, 0],
    ]);
    let mut rng = 910u64;
    for _ in 0..1000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let op = match rng & 15 {
            0 => 6,
            1 => 7,
            2 => 8,
            3 => 9,
            4 => 10,
            5 => 13,
            _ => (rng >> 8 & 1) as i32,
        };
        let id = 1000 + ((rng >> 16) % 64) as i32;
        actions.push(match op {
            7 => [7, (rng >> 32 & 3) as i32, 0, 0],
            10 => [10, 0, id, 0],
            _ => [op, id, (rng >> 32 & 1) as i32, (rng >> 33 & 1) as i32],
        });
    }
    int(&mut input, actions.len() as i32);
    let mut mids = vec![];
    let mut fids = vec![];
    let mut iids = vec![];
    for (step, a) in actions.iter().enumerate() {
        for v in a {
            int(&mut input, *v);
        }
        let archive = if a[1] == 0 {
            Archive::Sprites
        } else {
            Archive::Metrics
        };
        let result = (|| -> anyhow::Result<Return> {
            Ok(match a[0] {
                0 => Return::Metric(fonts.get_metrics(a[1], a[2] != 0, a[3] != 0)?),
                1 => Return::Font(fonts.get_font(a[1], a[2] != 0, a[3] != 0)?),
                2 => {
                    fonts.load_fonts()?;
                    Return::None
                }
                3 => {
                    fonts.clear_fonts();
                    Return::None
                }
                4 => Return::Int(fonts.loaded_count(a[1] != 0)?),
                5 => Return::Int(fonts.count()),
                6 => {
                    fonts.reset();
                    Return::None
                }
                7 => {
                    fonts.clean(a[1]);
                    Return::None
                }
                8 => {
                    fonts.clear_soft();
                    Return::None
                }
                9 => {
                    fonts.clear_soft_referents();
                    Return::None
                }
                10 => {
                    Return::Icons(fonts.icon_dimensions((a[1] >= 0).then_some(a[1] as u64), a[2])?)
                }
                11 => {
                    data.borrow_mut().files.remove(&(archive, a[2]));
                    Return::None
                }
                12 => {
                    data.borrow_mut().fail = if a[1] < 0 {
                        None
                    } else {
                        Some((archive, a[2]))
                    };
                    Return::None
                }
                13 => Return::Int(fonts.icon_width(a[1])?),
                14 => {
                    let b = data.borrow().files[&(archive, a[3])].clone();
                    data.borrow_mut().files.insert((archive, a[2]), b);
                    Return::None
                }
                _ => unreachable!(),
            })
        })();
        offsets.push_str(&format!("{} resource {step} {a:?}\n", output.len()));
        output.push(result.is_ok() as u8);
        if let Ok(r) = result {
            match r {
                Return::None | Return::Metric(None) | Return::Font(None) | Return::Icons(None) => {
                    int(&mut output, 0)
                }
                Return::Metric(Some(m)) => {
                    int(&mut output, 1);
                    metric(&mut output, &m, &mut mids);
                }
                Return::Font(Some(f)) => {
                    int(&mut output, 2);
                    int(&mut output, identity(&f, &mut fids));
                    metric(&mut output, &f.metrics, &mut mids);
                    output.push(f.monochrome as u8);
                }
                Return::Int(v) => {
                    int(&mut output, 3);
                    int(&mut output, v);
                }
                Return::Icons(Some(i)) => {
                    int(&mut output, 4);
                    int(&mut output, identity(&i, &mut iids));
                    int(&mut output, i.len() as i32);
                    for v in i.iter() {
                        int(&mut output, v.width);
                        int(&mut output, v.height);
                    }
                }
            }
        }
        snapshot(&mut output, &fonts, &data);
    }
    // Restore original sprites before measuring through the full retained host.
    for &id in &defaults {
        data.borrow_mut().files.insert(
            (Archive::Sprites, id),
            crate::js5_fetch::fetch_file(&pack, "sprites", id as u32)?.unwrap(),
        );
    }
    fonts.reset();
    fonts.load_fonts()?;
    data.borrow_mut().trace.clear();
    let mut state = State {
        fonts: Some(fonts),
        ..Default::default()
    };
    let mut store = Store::default();
    let mut changes = crate::ui_changes::Changes::default();
    let mut pool = Pool::default();
    let mut engine = crate::ui_hooks_oracle::Strict;
    let book = OpcodeBook::embedded()?;
    let mut cases = vec![];
    let samples = [
        "".into(),
        "AV To abc-def  ghi".into(),
        "A<img=0>V<img=-1><img=9999999>".into(),
        "A<sprite=2100,1>V".into(),
        "A<sprite=2001>V".into(),
        "A<sprite=9999>V".into(),
        "A<sprite=2101,999>V".into(),
        "<col=ff00ff>test</col><br><br>end".into(),
        "<lt><gt><nbsp><shy><times><euro><copy><reg>".into(),
        "Æ\0€😀".into(),
        "<img=+1><sprite=+2100,-2><sprite=bogus>".into(),
        "<br>".repeat(99),
        "<br>".repeat(100),
        "x <br>".repeat(101),
    ];
    for id in font_ids
        .into_iter()
        .map(|i| i as i32)
        .chain([-1, 9999, 2001, 2200])
    {
        for name in ["stringwidth", "paraheight", "parawidth", "paraline"] {
            for (n, s) in samples.iter().enumerate() {
                for width in [-1, 0, 1, 31, 200] {
                    let args = match name {
                        "stringwidth" => vec![id],
                        "paraline" => vec![width, id, [-1, 0, 1, 98, 99, 100, i32::MAX][n % 7]],
                        _ => vec![width, id],
                    };
                    cases.push((name, s, args));
                }
            }
        }
    }
    int(&mut input, cases.len() as i32);
    for (n, (name, s, args)) in cases.iter().enumerate() {
        // Supplementary text is measured below as raw UTF-16. The VM cannot
        // yet carry a returned line containing only one half of the pair.
        let vm_text = if *name == "paraline" {
            s.replace('😀', "")
        } else {
            (*s).clone()
        };
        let units: Vec<u16> = vm_text.encode_utf16().collect();
        text(&mut input, Some(&units));
        let mut code = vec![
            ins("push_constant_int", Operand::Int(321)),
            ins("push_constant_string", Operand::Str("kept".into())),
            ins("push_string_local", Operand::Local(0)),
        ];
        for &v in args {
            code.push(ins("push_constant_int", Operand::Int(v)));
        }
        code.push(ins(name, Operand::Byte(0)));
        code.push(ins("return", Operand::Byte(0)));
        let script = CompiledScript {
            name: None,
            locals: Counts {
                obj: 1,
                ..Counts::default()
            },
            args: Counts::default(),
            code,
        };
        let raw = native910::script::encode_script(&script, &book)?;
        int(&mut input, raw.len() as i32);
        input.extend(&raw);
        let script = native910::script::decode_script(&raw, &book)?;
        let mut now = || panic!("font measurement read delayed clock");
        let result = pool.execute(
            &mut store,
            &mut state,
            ScriptRun {
                id: 1,
                script: &script,
                request: &Request {
                    args: Some(vec![Arg::Int(1), Arg::String(units)]),
                    ..Default::default()
                },
                limit: 100,
            },
            Domains::Plain {
                changes: &mut changes,
                now: &mut now,
            },
            &mut engine,
            &(),
        )?;
        offsets.push_str(&format!("{} VM {n} {name} {args:?}\n", output.len()));
        output.push(result.result.is_ok() as u8);
        // Compare consumed arguments and retained prefixes even on failure.
        int(&mut output, result.snapshot.ints.len() as i32);
        for v in result.snapshot.ints {
            int(&mut output, v);
        }
        int(&mut output, result.snapshot.strings.len() as i32);
        for s in result.snapshot.strings {
            text(
                &mut output,
                s.as_deref().map(native910::jstr::units).as_deref(),
            );
        }
        snapshot(&mut output, state.fonts.as_ref().unwrap(), &data);
    }
    // Same measurements below the VM String boundary: exact UTF-16 lines,
    // including isolated surrogate results, plus null input strings.
    int(&mut input, cases.len() as i32 * 2);
    let fonts = state.fonts.as_ref().unwrap();
    for (n, (name, s, args)) in cases.iter().enumerate() {
        for null in [false, true] {
            let units = (!null).then(|| s.encode_utf16().collect::<Vec<_>>());
            int(
                &mut input,
                ["stringwidth", "paraheight", "parawidth", "paraline"]
                    .iter()
                    .position(|c| c == name)
                    .unwrap() as i32,
            );
            text(&mut input, units.as_deref());
            int(&mut input, args.len() as i32);
            for v in args {
                int(&mut input, *v);
            }
            let result = (|| -> anyhow::Result<crate::ui_fonts::Measurement> {
                let id = args[if *name == "stringwidth" { 0 } else { 1 }];
                let m = fonts
                    .get_metrics(id, true, true)?
                    .ok_or_else(|| anyhow::anyhow!("missing metrics"))?;
                fonts.measure(&m, name, units.as_deref(), args)
            })();
            offsets.push_str(&format!(
                "{} UTF16 {n} {null} {name} {args:?}\n",
                output.len()
            ));
            output.push(result.is_ok() as u8);
            if let Ok(value) = result {
                match value {
                    crate::ui_fonts::Measurement::Int(v) => int(&mut output, v),
                    crate::ui_fonts::Measurement::Text(t) => text(&mut output, t.as_deref()),
                }
            }
            snapshot(&mut output, fonts, &data);
        }
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(out.join("rust-offsets.txt"), offsets)?;
    std::fs::write(
        out.join("counts.txt"),
        format!("{} {} {}\n", actions.len(), cases.len(), cases.len() * 2),
    )?;
    scratch.finish("ui-fonts", &[("rust.bin", "recording")]);
    Ok(())
}
