//! Event locals, reusable execution contexts and synchronous interface hooks.
use crate::{
    ui_components::{Active, Arg, Array, Ref, Store, Text},
    ui_properties::State,
};
use anyhow::{Context, Result};
use native910::{
    script::CompiledScript,
    vm::{HookLocals, Session, VmError, VmResult},
};
use rs910_core::fault::Fault;
use std::rc::Rc;
pub const INTERACTIVE_LIMIT: usize = 500_000;
pub const ONLOAD_LIMIT: usize = 5_000_000;
#[derive(Clone, Default)]
pub struct Request {
    pub args: Option<Vec<Arg>>,
    pub component: Option<Ref>,
    /// Only the retained variable-event queue uses this verified token codec.
    pub variable_event_tokens: Option<native910::execution::VariableEventTokens>,
    pub drop: Option<Ref>,
    pub mouse: [i32; 2],
    pub opindex: i32,
    pub key: i32,
    pub keychar: i32,
    pub opbase: Option<Text>,
    pub nested_count: i32,
    pub is_mouse: bool,
    /// `PROCESS_PLAYER` writes the active player index into int local 0
    /// before executing the trigger. Component hooks leave this unset.
    pub trigger_ints: Option<Vec<i32>>,
    /// `executeTriggeredScriptCutscene` writes the subtitle text
    /// into object local 0.
    pub trigger_strings: Option<Vec<Text>>,
}
impl Request {
    pub fn component(c: &Ref, args: Vec<Arg>) -> Self {
        Self {
            component: Some(c.clone()),
            args: Some(args),
            ..Self::default()
        }
    }
    pub fn script_id(&self) -> Result<i32> {
        match self.args.as_ref().and_then(|v| v.first()) {
            Some(Arg::Int(id)) => Ok(*id),
            _ => anyhow::bail!("hook script ID is not Integer"),
        }
    }
}
#[derive(Default, Clone)]
pub struct Locals {
    pub ints: Vec<i32>,
    pub strings: Vec<Option<Text>>,
    pub longs: Vec<i64>,
}
impl Locals {
    pub fn fill(&mut self, script: &CompiledScript, r: &Request) -> Result<()> {
        self.ints = vec![0; script.locals.int as usize];
        self.strings = vec![None; script.locals.obj as usize];
        self.longs = vec![0; script.locals.long as usize];
        let args = r.args.as_ref().context("null hook args")?;
        let mut indices = [0; 3];
        for a in args.iter().skip(1) {
            match a {
                Arg::Int(v) => {
                    // The original client uses sequential independent if statements, so an
                    // event value that equals a later sentinel is substituted again.
                    if let Some(tokens) = r.variable_event_tokens {
                        let n = indices[0];
                        indices[0] += 1;
                        *self.ints.get_mut(n).context("hook int local index")? =
                            variable_event_integer(*v, tokens, r.component.as_ref());
                        continue;
                    }
                    let replacements = [
                        r.mouse[0],
                        r.mouse[1],
                        r.component
                            .as_ref()
                            .map_or(-1, |c| c.borrow().f.parentlayer),
                        r.opindex,
                        r.component.as_ref().map_or(-1, |c| c.borrow().f.id),
                        r.drop.as_ref().map_or(-1, |c| c.borrow().f.parentlayer),
                        r.drop.as_ref().map_or(-1, |c| c.borrow().f.id),
                        r.key,
                        r.keychar,
                    ];
                    let mut v = *v;
                    for (index, replacement) in replacements.into_iter().enumerate() {
                        if v == -2147483647 + index as i32 {
                            v = replacement;
                        }
                    }
                    let n = indices[0];
                    indices[0] += 1;
                    *self.ints.get_mut(n).context("hook int local index")? = v;
                }
                Arg::String(v) => {
                    let n = indices[1];
                    indices[1] += 1;
                    let value = if v.iter().copied().eq("event_opbase".encode_utf16()) {
                        r.opbase.clone()
                    } else {
                        Some(v.clone())
                    };
                    *self.strings.get_mut(n).context("hook string local index")? = value;
                }
                Arg::Long(v) => {
                    let n = indices[2];
                    indices[2] += 1;
                    *self.longs.get_mut(n).context("hook long local index")? = *v;
                }
                Arg::Null => {}
            }
        }
        if let Some(values) = &r.trigger_ints {
            for (index, value) in values.iter().copied().enumerate() {
                if let Some(slot) = self.ints.get_mut(index) {
                    *slot = value;
                }
            }
        }
        if let Some(values) = &r.trigger_strings {
            for (index, value) in values.iter().enumerate() {
                *self
                    .strings
                    .get_mut(index)
                    .context(Fault::IndexOutOfRange.message("trigger object local"))? =
                    Some(value.clone());
            }
        }
        Ok(())
    }
    fn vm(&self) -> Result<HookLocals> {
        Ok(HookLocals {
            ints: self.ints.clone(),
            strings: self
                .strings
                .iter()
                .map(|v| v.as_ref().map(|v| native910::jstr::from_units(v)))
                .collect(),
            longs: self.longs.clone(),
        })
    }
}
/// Script state arrays retain their definitions between root invocations.
#[derive(Default)]
pub struct Arrays {
    pub values: [Vec<i32>; 5],
}
impl Arrays {
    fn index(id: i32) -> VmResult<usize> {
        let n = usize::try_from(id).ok().filter(|n| *n < 5);
        n.ok_or_else(|| VmError::BadArray {
            id,
            reason: Fault::IndexOutOfRange.message("array slot outside the script state"),
        })
    }
    pub fn define(&mut self, id: i32, n: usize) -> VmResult<()> {
        self.values[Self::index(id)?] = vec![0; n];
        Ok(())
    }
    pub fn len(&self, id: i32) -> VmResult<usize> {
        Ok(self.values[Self::index(id)?].len())
    }
    pub fn get(&self, id: i32, n: i32) -> VmResult<i32> {
        self.values[Self::index(id)?]
            .get(n as usize)
            .copied()
            .ok_or_else(|| VmError::BadArray {
                id,
                reason: "array index".into(),
            })
    }
    pub fn set(&mut self, id: i32, n: i32, v: i32) -> VmResult<()> {
        *self.values[Self::index(id)?]
            .get_mut(n as usize)
            .ok_or_else(|| VmError::BadArray {
                id,
                reason: "array index".into(),
            })? = v;
        Ok(())
    }
    /// `array_sort`: sort the first `count` keys of
    /// array `keys` and permute array `values` alongside
    /// (the client's in-place sort).
    pub fn sort(&mut self, count: i32, keys: i32, values: i32) -> VmResult<()> {
        let (ki, vi) = (Self::index(keys)?, Self::index(values)?);
        let fail = |reason: &str| VmError::BadArray {
            id: keys,
            reason: reason.into(),
        };
        if count as i64 > self.values[ki].len() as i64
            || count as i64 > self.values[vi].len() as i64
        {
            return Err(fail(
                &Fault::InvalidState.message("array_sort count exceeds an array size"),
            ));
        }
        if keys == values {
            return Err(fail(
                &Fault::InvalidState.message("array_sort on one array"),
            ));
        }
        let mut k = std::mem::take(&mut self.values[ki]);
        let mut v = std::mem::take(&mut self.values[vi]);
        quicksort_parallel(&mut k, &mut v, 0, count as i64 - 1);
        self.values[ki] = k;
        self.values[vi] = v;
        Ok(())
    }
}
/// The client's sort verbatim, including the `i32::MAX` pivot
/// guard and the `(index & guard) + pivot` comparison.
fn quicksort_parallel(keys: &mut [i32], values: &mut [i32], lo: i64, hi: i64) {
    if lo >= hi {
        return;
    }
    let mid = ((lo + hi) / 2) as usize;
    let (lo_u, hi_u) = (lo as usize, hi as usize);
    let mut store = lo_u;
    let pivot = keys[mid];
    keys.swap(mid, hi_u);
    let pivot_value = values[mid];
    values.swap(mid, hi_u);
    let guard = if pivot == i32::MAX { 0 } else { 1 };
    for i in lo_u..hi_u {
        if keys[i] < (i as i32 & guard).wrapping_add(pivot) {
            keys.swap(i, store);
            values.swap(i, store);
            store += 1;
        }
    }
    keys[hi_u] = keys[store];
    keys[store] = pivot;
    values[hi_u] = values[store];
    values[store] = pivot_value;
    quicksort_parallel(keys, values, lo, store as i64 - 1);
    quicksort_parallel(keys, values, store as i64 + 1, hi);
}
#[derive(Default)]
pub struct ExecutionContext {
    pub active: [Active; 2],
    pub arrays: Arrays,
    pub locals: Locals,
    pub longs: Vec<i64>,
    pub nested_count: i32,
}
#[derive(Default)]
pub struct Pool {
    pub contexts: Vec<ExecutionContext>,
    pub used: usize,
}
impl Pool {
    pub fn acquire(&mut self) -> usize {
        if self.used == self.contexts.len() {
            self.contexts.push(ExecutionContext::default());
        }
        let n = self.used;
        self.used += 1;
        n
    }
    /// Preparation failures precede executeScript's finally block in the original client. The
    /// acquired context therefore remains in use if local argument copying fails.
    pub fn prepare(&mut self, index: usize, script: &CompiledScript, r: &Request) -> Result<()> {
        let c = &mut self.contexts[index];
        c.locals.fill(script, r)?;
        c.nested_count = r.nested_count;
        Ok(())
    }
    pub fn session(
        &self,
        index: usize,
        id: i32,
        script: &CompiledScript,
        limit: usize,
    ) -> Result<Session> {
        let c = &self.contexts[index];
        Ok(Session::for_hook(
            id,
            script,
            c.locals.vm()?,
            &c.longs,
            limit,
            Some("interface hook".into()),
        )?)
    }
    pub fn finish(&mut self, index: usize, session: &Session) {
        assert_eq!(index + 1, self.used);
        let c = &mut self.contexts[index];
        c.longs = session.retained_longs().to_vec();
        let snapshot = session.snapshot();
        c.locals = Locals {
            ints: snapshot.int_locals,
            strings: snapshot
                .string_locals
                .into_iter()
                .map(|v| v.map(|v| native910::jstr::units(&v)))
                .collect(),
            longs: snapshot.long_locals,
        };
        c.nested_count = 0;
        self.used -= 1;
    }
}
/// Synchronous boundary: implementations execute the hook before the traversal
/// reads the next component/hook. A queued batch would change original behavior.
pub trait Executor {
    fn run(
        &mut self,
        store: &mut Store,
        state: &mut State,
        request: Request,
        limit: usize,
    ) -> Result<()>;
}
/// The main event queue, after the timer and mouse-stop
/// queues. New requests append to this same queue during execution. Detached
/// dynamic children are discarded by identity; static component references run.
/// The attach check: a dynamic child (id >= 0) runs only
/// while it is still the parent's current child at that slot.
pub fn attached(store: &mut Store, c: &Ref) -> Result<bool> {
    if let Some(attached) = crate::ui_components::runtime_child_attached(c) {
        if !attached {
            return Ok(false);
        }
        let (packed, id) = {
            let component = c.borrow();
            (component.f.parentlayer, component.f.id)
        };
        return Ok(store
            .get(packed, id)?
            .is_some_and(|current| Rc::ptr_eq(&current, c)));
    }
    let (id, layer) = {
        let c = c.borrow();
        (c.f.id, c.f.layer)
    };
    if id < 0 {
        return Ok(true);
    }
    let Some(parent) = store.get(layer, -1)? else {
        return Ok(false);
    };
    let children = parent.borrow().children.clone();
    Ok(children.is_some_and(|a| {
        a.borrow()
            .get(id as usize)
            .and_then(Option::as_ref)
            .is_some_and(|current| Rc::ptr_eq(current, c))
    }))
}
pub fn drain_main(store: &mut Store, state: &mut State, e: &mut impl Executor) -> Result<()> {
    while let Some(r) = state.layout.hooks.pop_front() {
        let c = r
            .component
            .as_ref()
            .context("queued hook has no component")?;
        if !attached(store, c)? {
            continue;
        }
        e.run(store, state, r, INTERACTIVE_LIMIT)?;
    }
    Ok(())
}
pub fn on_load(
    store: &mut Store,
    state: &mut State,
    id: i32,
    keys: crate::ui_resources::Keys,
    e: &mut impl Executor,
) -> Result<()> {
    if id == -1 || !store.open(id, keys)? {
        return Ok(());
    }
    let array = store.interfaces[&id].borrow().components.clone();
    on_load_components(store, state, &array, e)
}
pub fn on_load_components(
    store: &mut Store,
    state: &mut State,
    array: &Array,
    e: &mut impl Executor,
) -> Result<()> {
    let count = array.borrow().len();
    for n in 0..count {
        // executeOnLoadComponents dereferences holes; runHookLayer instead skips them.
        let c = array.borrow()[n].clone().context("onload component hole")?;
        let args = c.borrow().hooks.get("onload").cloned();
        if let Some(args) = args {
            e.run(store, state, Request::component(&c, args), ONLOAD_LIMIT)?;
        }
    }
    Ok(())
}
pub fn immediate(
    store: &mut Store,
    state: &mut State,
    id: i32,
    kind: i32,
    e: &mut impl Executor,
) -> Result<()> {
    if id != -1 && store.open(id, None)? {
        let a = store.interfaces[&id].borrow().components.clone();
        layer(store, state, &a, kind, e)?;
    }
    Ok(())
}
pub fn layer(
    store: &mut Store,
    state: &mut State,
    array: &Array,
    kind: i32,
    e: &mut impl Executor,
) -> Result<()> {
    let count = array.borrow().len();
    for n in 0..count {
        let Some(c) = array.borrow()[n].clone() else {
            continue;
        };
        if c.borrow().f.r#type == 0 {
            let children = c.borrow().child_drawing_order();
            if let Some(a) = children {
                layer(store, state, &a, kind, e)?;
            }
            let packed = c.borrow().f.parentlayer;
            let sub = state
                .layout
                .subs
                .iter()
                .find(|(p, _)| !c.borrow().has_runtime_parent() && *p == packed)
                .map(|p| p.1);
            if let Some(sub) = sub {
                immediate(store, state, sub, kind, e)?;
            }
        }
        let hook = match kind {
            0 => "ondialogabort",
            1 => "onsubchange",
            _ => continue,
        };
        let args = c.borrow().hooks.get(hook).cloned();
        if let Some(args) = args {
            let (packed, child) = {
                let c = c.borrow();
                (c.f.parentlayer, c.f.id)
            };
            if kind == 1 && child >= 0 && c.borrow().has_runtime_parent() {
                if !attached(store, &c)? {
                    continue;
                }
            } else if kind == 1 && child >= 0 {
                let current = store.get(packed, child)?;
                if !current.as_ref().is_some_and(|v| Rc::ptr_eq(v, &c)) {
                    continue;
                }
            }
            e.run(
                store,
                state,
                Request::component(&c, args),
                INTERACTIVE_LIMIT,
            )?;
        }
    }
    Ok(())
}

/// Variable-event requests supply zero context integers and no drop component.
/// Component words retain their missing-file and missing-child sentinels.
fn variable_event_integer(
    mut value: i32,
    tokens: native910::execution::VariableEventTokens,
    component: Option<&Ref>,
) -> i32 {
    const ABSENT: i32 = -1;
    const CONTEXT_DEFAULT: i32 = 0;
    let (parent, child) = component.map_or((ABSENT, ABSENT), |component| {
        let component = component.borrow();
        let packed = component.f.parentlayer as u32;
        let file = packed as u16;
        let child = component.f.id as u16;
        (
            if file == u16::MAX {
                ABSENT
            } else {
                packed as i32
            },
            if child == u16::MAX {
                ABSENT
            } else {
                i32::from(child)
            },
        )
    });
    let replacements = [
        CONTEXT_DEFAULT,
        CONTEXT_DEFAULT,
        parent,
        CONTEXT_DEFAULT,
        child,
        ABSENT,
        ABSENT,
        CONTEXT_DEFAULT,
        CONTEXT_DEFAULT,
        CONTEXT_DEFAULT,
        CONTEXT_DEFAULT,
    ];
    for (slot, replacement) in replacements
        .into_iter()
        .take(usize::from(tokens.count()))
        .enumerate()
    {
        if value == tokens.first() + slot as i32 {
            value = replacement;
        }
    }
    value
}

#[cfg(test)]
pub(crate) fn verify_variable_event_recording() {
    use crate::ui_components::Component;
    use native910::{
        execution::VariableEventTokens,
        script::{CompiledScript, Counts},
    };
    use std::cell::RefCell;
    let record: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/variable-event-locals.json")).unwrap();
    let policy: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../revisions/950/cs2/950-1-to-910.json"
    ))
    .unwrap();
    let tokens = policy["callback_policy"]["int_event_tokens"]
        .as_array()
        .unwrap();
    let tokens =
        VariableEventTokens::new(tokens[0].as_i64().unwrap() as i32, tokens.len() as u8).unwrap();
    for case in record["cases"].as_array().unwrap() {
        let input = &case["input"];
        let result = &case["result"];
        let arguments = input["arguments"].as_array().unwrap();
        let mut component = Component::default();
        component.f.parentlayer = input["parent"].as_u64().unwrap() as u32 as i32;
        component.f.id = input["child"].as_u64().unwrap() as u16 as i32;
        let component = Rc::new(RefCell::new(component));
        let request = Request {
            variable_event_tokens: Some(tokens),
            component: (input["component"] == true).then_some(component),
            args: Some(
                std::iter::once(Arg::Int(i32::default()))
                    .chain(
                        arguments
                            .iter()
                            .map(|value| Arg::Int(value.as_i64().unwrap() as i32)),
                    )
                    .collect(),
            ),
            ..Request::default()
        };
        let counts = Counts {
            int: arguments.len() as u16,
            ..Counts::default()
        };
        let script = CompiledScript {
            name: None,
            args: counts,
            locals: counts,
            code: Vec::new(),
        };
        let mut locals = Locals::default();
        locals.fill(&script, &request).unwrap();
        assert_eq!(
            locals.ints,
            result["locals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap() as i32)
                .collect::<Vec<_>>(),
            "{}",
            case["name"]
        );
        assert!(result["request_integer_defaults"]
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value == &serde_json::json!(i32::default())));
    }
}
