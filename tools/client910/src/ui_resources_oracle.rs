use crate::{
    cache::Pack,
    ui_components::{Component, Interface, InterfaceRef, Ref, Store},
    ui_components_oracle::{component, int},
    ui_resources::{Keys, Source},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
#[derive(Default)]
struct Data {
    ready: BTreeSet<i32>,
    hidden: BTreeSet<(i32, i32)>,
    files: BTreeMap<i32, BTreeMap<i32, Vec<u8>>>,
    capacities: BTreeMap<i32, usize>,
    trace: Vec<i32>,
}
struct Memory(Rc<RefCell<Data>>);
impl Source for Memory {
    fn capacity(&self) -> usize {
        5
    }
    fn ready(&mut self, id: i32) -> anyhow::Result<bool> {
        let mut d = self.0.borrow_mut();
        d.trace.extend([0, id]);
        Ok(d.ready.contains(&id))
    }
    fn group_capacity(&mut self, id: i32) -> anyhow::Result<usize> {
        let mut d = self.0.borrow_mut();
        d.trace.extend([1, id]);
        Ok(*d.capacities.get(&id).unwrap_or(&0))
    }
    fn file(&mut self, id: i32, file: i32, keys: Keys) -> anyhow::Result<Option<Vec<u8>>> {
        let mut d = self.0.borrow_mut();
        d.trace.extend([2, id, file, keys.is_some() as i32]);
        if let Some(k) = keys {
            d.trace.extend(k);
        }
        Ok(if d.hidden.contains(&(id, file)) {
            None
        } else {
            d.files.get(&id).and_then(|f| f.get(&file)).cloned()
        })
    }
    fn discard(&mut self, id: i32) -> anyhow::Result<()> {
        self.0.borrow_mut().trace.extend([3, id]);
        Ok(())
    }
}
fn cid(v: &mut Vec<Ref>, c: Option<&Ref>) -> i32 {
    match c {
        None => -1,
        Some(c) => {
            if let Some(n) = v.iter().position(|v| Rc::ptr_eq(v, c)) {
                n as i32
            } else {
                v.push(c.clone());
                v.len() as i32 - 1
            }
        }
    }
}
fn iid(v: &mut Vec<InterfaceRef>, i: &InterfaceRef) -> i32 {
    if let Some(n) = v.iter().position(|v| Rc::ptr_eq(v, i)) {
        n as i32
    } else {
        v.push(i.clone());
        v.len() as i32 - 1
    }
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-resources");
    let out = scratch.dir().to_path_buf();
    let root = rs910_core::test_support::repo_root();
    let pack = Pack::open(root.join("server/data/pack"));
    let raw = pack.read_group("interfaces", 1477)?;
    let mut input = vec![];
    let mut output = vec![];
    let names = crate::ui_component_fields::FIELD_NAMES;
    int(&mut input, names.len() as i32);
    for n in names {
        crate::ui_components_oracle::text(&mut input, Some(&n.encode_utf16().collect::<Vec<_>>()));
    }
    for file in [0, 1, 3] {
        let b = &raw[&file];
        int(&mut input, b.len() as i32);
        input.extend(b);
    }
    let mut actions = vec![];
    for round in 0..20 {
        actions.extend([
            [0, 1, 0, 0],
            [1, 65536, 0, 0],
            [1, 65538, -1, 0],
            [3, 1, 0, 0],
            [2, 1, 0, 0],
            [4, 1, 0, 0],
            [0, 1, 0, 1],
            [4, 1, 0, 1],
            [5, 1, 1, 1],
            [0, 1, 0, round & 1],
            [1, 65537, -1, 0],
            [5, 1, 1, 0],
            [0, 1, 0, 0],
            [1, 65537, -1, 0],
            [6, 1, 4, 0],
            [0, 1, 0, 1],
            [1, 65537, -1, 0],
            [2, 1, 0, 0],
            [0, 1, 0, 1],
            [1, 65536, -2, 0],
            [1, 65539, -1, 0],
            [1, 65540, -1, 0],
            [0, 2, 0, 0],
            [1, 131072, -1, 0],
            [0, 3, 0, 0],
            [1, 196609, -1, 0],
            [0, -1, 0, 0],
            [0, 5, 0, 0],
            [3, -1, 0, 0],
            [2, -1, 0, 0],
            [6, 1, 2, 0],
            [0, 1, 0, 0],
            [4, 1, 0, 0],
            [0, 1, 0, 0],
            [4, 1, 0, 1],
            [0, 1, 0, 0],
        ]);
    }
    int(&mut input, actions.len() as i32);
    let mut data = Data::default();
    data.ready.extend([1, 2, 3]);
    data.capacities.extend([(1, 4), (2, 0), (3, 2)]);
    data.files.insert(
        1,
        [0, 1, 3]
            .into_iter()
            .map(|f| (f, raw[&(f as u32)].clone()))
            .collect(),
    );
    data.files
        .insert(3, [(1, raw[&1].clone())].into_iter().collect());
    let data = Rc::new(RefCell::new(data));
    let mut store = Store::with_source(Box::new(Memory(data.clone())));
    let mut cs = vec![];
    let mut interfaces = vec![];
    let mut offsets = String::new();
    for (index, a) in actions.iter().enumerate() {
        for v in a {
            int(&mut input, *v);
        }
        let keys = (a[3] != 0).then_some([1, -2, i32::MIN, i32::MAX]);
        let result = (|| -> anyhow::Result<i32> {
            Ok(match a[0] {
                0 => store.open(a[1], keys)? as i32,
                1 => cid(&mut cs, store.get(a[1], a[2])?.as_ref()),
                2 => {
                    store.unload(a[1])?;
                    -1
                }
                3 => {
                    store.discard_if_unloaded(a[1])?;
                    -1
                }
                4 => {
                    if a[3] == 0 {
                        data.borrow_mut().ready.remove(&a[1]);
                    } else {
                        data.borrow_mut().ready.insert(a[1]);
                    }
                    -1
                }
                5 => {
                    if a[3] == 0 {
                        data.borrow_mut().hidden.remove(&(a[1], a[2]));
                    } else {
                        data.borrow_mut().hidden.insert((a[1], a[2]));
                    }
                    -1
                }
                6 => {
                    let mut values = vec![None; a[2] as usize];
                    if !values.is_empty() {
                        values[0] = Some(Rc::new(RefCell::new(Component::decode(
                            a[1] << 16,
                            &raw[&0],
                        )?)));
                    }
                    store.interfaces.insert(a[1], Interface::new(values));
                    store.resources.as_mut().unwrap().loaded.remove(&a[1]);
                    -1
                }
                _ => unreachable!(),
            })
        })();
        offsets.push_str(&format!("{} {index} {a:?}\n", output.len()));
        output.push(result.is_ok() as u8);
        if let Ok(v) = result {
            int(&mut output, v);
        }
        for group in 0..5 {
            output.push(store.resources.as_ref().unwrap().loaded.contains(&group) as u8);
            if let Some(i) = store.interfaces.get(&group) {
                int(&mut output, iid(&mut interfaces, i));
                let a = i.borrow().components.clone();
                int(&mut output, a.borrow().len() as i32);
                for c in a.borrow().iter() {
                    int(&mut output, cid(&mut cs, c.as_ref()));
                }
            } else {
                int(&mut output, -1);
            }
        }
        int(&mut output, cs.len() as i32);
        for c in &cs {
            component(&mut output, &c.borrow());
        }
        let mut d = data.borrow_mut();
        int(&mut output, d.trace.len() as i32);
        for v in d.trace.drain(..) {
            int(&mut output, v);
        }
    }
    std::fs::write(out.join("resources-input.bin"), input)?;
    std::fs::write(out.join("rust-resources.bin"), output)?;
    std::fs::write(out.join("rust-resources-offsets.txt"), offsets)?;
    std::fs::write(out.join("resources-count.txt"), actions.len().to_string())?;
    scratch.finish("ui-hooks", &[("rust-resources.bin", "resources")]);
    Ok(())
}
