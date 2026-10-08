//! Hook execution borrows the same variable domains and delayed queue as game
//! scripts. Component traps borrow that queue only during their own dispatch.
use crate::{
    ui_changes::Changes,
    ui_components::{ScriptHost, Store},
    ui_hooks::{ExecutionContext, Executor, Pool, Request},
    ui_properties::{Context, State},
    ui_vars::Variables,
};
use anyhow::{Context as _, Result};
use native910::{
    script::CompiledScript,
    vm::{self, Host, ScriptProvider, Snapshot, Vm},
};
pub enum Domains<'a, 'd> {
    Game(&'a mut Variables<'d>),
    Plain {
        changes: &'a mut Changes,
        now: &'a mut dyn FnMut() -> i64,
    },
}
impl<'d> Domains<'_, 'd> {
    pub fn reborrow(&mut self) -> Domains<'_, 'd> {
        match self {
            Self::Game(v) => Domains::Game(v),
            Self::Plain { changes, now } => Domains::Plain {
                changes,
                now: &mut **now,
            },
        }
    }
}
pub struct HookHost<'a, 'd, H> {
    pub engine: &'a mut H,
    pub store: &'a mut Store,
    pub properties: &'a mut State,
    pub context: &'a mut ExecutionContext,
    pub domains: Domains<'a, 'd>,
}
fn error(e: anyhow::Error) -> vm::VmError {
    vm::VmError::TrapFailed {
        command: "hook domain".into(),
        reason: format!("{e:#}"),
    }
}
impl<H: Host> Host for HookHost<'_, '_, H> {
    fn trap_resource_context(
        &mut self,
        operation: native910::execution::HostOperation,
        context: &native910::vm::InstructionContext<'_>,
        resource: Option<&native910::execution::Resource>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        if matches!(
            operation,
            native910::execution::HostOperation::ComponentText { .. }
        ) {
            let mut dispatch = |changes: &mut Changes, now: &mut dyn FnMut() -> i64| {
                Context {
                    state: self.properties,
                    changes,
                    now,
                    nested_count: self.context.nested_count,
                }
                .text_component(
                    self.store,
                    &mut self.context.active,
                    context,
                    operation,
                    resource,
                    ints,
                )
            };
            let result = match &mut self.domains {
                Domains::Game(variables) => {
                    dispatch(&mut variables.state.delayed, &mut variables.now)
                }
                Domains::Plain { changes, now } => dispatch(changes, now),
            };
            return result
                .map_err(|error| crate::ui_components::component_error(context.command, error));
        }
        if matches!(
            operation,
            native910::execution::HostOperation::CreateFlatTextChild { .. }
        ) {
            return crate::ui_components::component_creation_operation(
                self.store,
                &mut self.context.active,
                operation,
                context,
                resource,
                ints,
            );
        }
        if operation.needs_resource() {
            self.engine
                .trap_resource_context(operation, context, resource, ints, objects, longs)
        } else {
            self.trap_operation_context(operation, context, ints, objects, longs)
        }
    }
    fn trap_operation_context(
        &mut self,
        operation: native910::execution::HostOperation,
        c: &vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> vm::VmResult<Option<vm::Value>> {
        if let native910::execution::HostOperation::RetainedPlayerTransmit {
            event_tokens, ..
        } = operation
        {
            return crate::ui_properties::install_retained_player(
                &mut self.context.active,
                c,
                event_tokens,
                ints,
                objects,
                longs,
            )
            .map(|()| None)
            .map_err(|error| crate::ui_components::component_error(c.command, error));
        }
        if matches!(
            operation,
            native910::execution::HostOperation::ComponentText { .. }
        ) {
            return self.trap_resource_context(operation, c, None, ints, objects, longs);
        }
        if let native910::execution::HostOperation::ComponentPaint { property, explicit } =
            operation
        {
            let mut dispatch = |changes: &mut Changes, now: &mut dyn FnMut() -> i64| {
                Context {
                    state: self.properties,
                    changes,
                    now,
                    nested_count: self.context.nested_count,
                }
                .paint_component(
                    self.store,
                    &mut self.context.active,
                    c,
                    property,
                    explicit,
                    ints,
                )
            };
            let result = match &mut self.domains {
                Domains::Game(variables) => {
                    dispatch(&mut variables.state.delayed, &mut variables.now)
                }
                Domains::Plain { changes, now } => dispatch(changes, now),
            };
            return result
                .map(|()| None)
                .map_err(|error| crate::ui_components::component_error(c.command, error));
        }
        crate::ui_components::component_operation(
            self.store,
            &mut self.context.active,
            operation,
            c,
            ints,
        )
    }

    fn trap_objects_context(
        &mut self,
        c: &vm::InstructionContext<'_>,
        i: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        l: &mut Vec<i64>,
    ) -> vm::VmResult<Option<vm::Value>> {
        let mut dispatch = |changes: &mut Changes, now: &mut dyn FnMut() -> i64| {
            Context {
                state: self.properties,
                changes,
                now,
                nested_count: self.context.nested_count,
            }
            .dispatch_objects(self.store, &mut self.context.active, c, i, objects, l)
        };
        let result = match &mut self.domains {
            Domains::Game(v) => dispatch(&mut v.state.delayed, &mut v.now),
            Domains::Plain { changes, now } => dispatch(changes, now),
        };
        if let Some(result) = result {
            return result.map_err(|e| crate::ui_components::component_error(c.command, e));
        }
        vm::with_string_stack(c.command, objects, |strings| {
            self.trap_context(c, i, strings, l)
        })
    }
    fn trap_context(
        &mut self,
        c: &vm::InstructionContext<'_>,
        i: &mut Vec<i32>,
        s: &mut Vec<String>,
        l: &mut Vec<i64>,
    ) -> vm::VmResult<Option<vm::Value>> {
        use crate::ui_interaction::{self as interaction, Action};
        if c.command == "targetmode_active" {
            return Ok(Some(vm::Value::Int(i32::from(
                self.properties.interaction.target.active,
            ))));
        }
        let count = match c.command {
            "if_dragpickup" | "if_triggerop" => Some(3),
            "cc_dragpickup" | "if_debug_target" => Some(2),
            "cc_triggerop" => Some(1),
            "force_interface_drag" | "cancel_interface_drag" | "targetmode_cancel" => Some(0),
            _ => None,
        };
        if let Some(count) = count {
            if i.len() < count {
                return Err(vm::VmError::StackUnderflow { stack: "int" });
            }
            let a = i.split_off(i.len() - count);
            match c.command {
                "targetmode_cancel" => self
                    .properties
                    .interaction
                    .script_actions
                    .push_back(Action::ClearTarget),
                "force_interface_drag" => self.properties.interaction.drag.active = true,
                "cancel_interface_drag" => interaction::cancel_drag(self.properties),
                "if_dragpickup" | "cc_dragpickup" => {
                    let component = if c.command == "if_dragpickup" {
                        self.store.get(a[2], -1).map_err(error)?
                    } else {
                        self.context.active[c.secondary as usize].component.clone()
                    };
                    interaction::pickup(self.store, self.properties, component, [a[0], a[1]])
                        .map_err(error)?;
                }
                "if_debug_target" => {
                    self.properties
                        .interaction
                        .script_actions
                        .push_back(Action::Select {
                            parent: a[0],
                            child: a[1],
                        })
                }
                _ => {
                    let (op, parent, child) = if c.command == "if_triggerop" {
                        (a[2], a[0], a[1])
                    } else {
                        let component = self.context.active[c.secondary as usize]
                            .component
                            .as_ref()
                            .ok_or_else(|| error(anyhow::anyhow!("null active component")))?
                            .borrow();
                        (a[0], component.f.parentlayer, component.f.id)
                    };
                    if !(1..=10).contains(&op) {
                        return Err(error(anyhow::anyhow!("invalid interface operation {op}")));
                    }
                    self.properties
                        .interaction
                        .script_actions
                        .push_back(Action::Op {
                            op,
                            parent,
                            child,
                            base: Some(vec![]),
                        });
                }
            }
            return Ok(None);
        }
        // Diagnostic (`CLIENT910_CS2_CMD_TRACE=cmd,cmd`): command name plus
        // the top of both stacks before the host consumes them.
        let flags = crate::ui_debug_flags::flags();
        if flags.traces_cs2_command(c.command) {
            log::info!(
                "[cs2-cmd] {} script {:?} pc {} ints {:?} objs {:?}",
                c.command,
                c.script_id,
                c.pc,
                &i[i.len().saturating_sub(4)..],
                &s[s.len().saturating_sub(2)..]
            );
        }
        if flags.ui_trace_hide && c.command.ends_with("sethide") {
            let tail = &i[i.len().saturating_sub(2)..];
            log::info!(
                "[trace] {} script {:?} pc {} args{:?}",
                c.command,
                c.script_id,
                c.pc,
                tail.iter()
                    .map(|&v| if v > 65535 {
                        format!("{}:{}", v >> 16, v & 0xFFFF)
                    } else {
                        v.to_string()
                    })
                    .collect::<Vec<_>>()
            );
        }
        if let Domains::Game(v) = &mut self.domains {
            self.properties.local_player_uid = v.scene.local_player.map_or(-1, |p| p.index);
            let cycle = v.cycle;
            // The profiling commands measure on the renderer the shell lent
            // for this cycle, with the interface's own message box style.
            let mut profiler =
                v.probe
                    .as_deref_mut()
                    .map(|renderer| crate::ui_preferences::Profiler {
                        renderer,
                        fonts: self.properties.fonts.as_ref(),
                        message_box: &self.properties.message_box,
                    });
            if let Some(result) =
                v.state
                    .queries
                    .dispatch(c.command, cycle, i, s, profiler.as_mut())
            {
                if result.is_ok()
                    && matches!(
                        c.command,
                        "setwindowmode" | "fullscreen_enter" | "fullscreen_exit"
                    )
                {
                    let preferences = &v.state.queries.preferences;
                    if let Some(canvas) = preferences
                        .window
                        .canvas(preferences.options.live().screen_size)
                    {
                        self.properties.layout.canvas = canvas.size;
                        if self.properties.life.top != -1 {
                            self.properties
                                .layout
                                .interface(self.store, self.properties.life.top, canvas.size, true)
                                .map_err(error)?;
                        }
                        self.properties.life.redraw.fill(true);
                    }
                }
                return result;
            }
            if let Some(result) = crate::ui_player_state::dispatch(
                v,
                v.state.stats.as_ref(),
                v.state.quests.as_ref(),
                c.command,
                i,
                s,
            ) {
                return result;
            }
        }
        if !matches!(&self.domains, Domains::Game(_)) {
            self.properties.local_player_uid = -1;
        }
        // get_minimenu_target reads the live
        // target-mode owner, preserving the original client's int/object/object stack order.
        if c.command == "get_minimenu_target" {
            let target = &self.properties.interaction.target;
            i.push(i32::from(target.active));
            s.push(if target.active {
                target.name.clone()
            } else {
                String::new()
            });
            s.push(if target.active {
                target.verb.clone()
            } else {
                String::new()
            });
            return Ok(None);
        }
        // self_player_uid reads currentPlayerUid.
        if c.command == "self_player_uid" {
            i.push(self.properties.local_player_uid);
            return Ok(None);
        }
        // get_active_minimenu_entry,
        // get_second_minimenu_entry (pushMinimenuEntry)
        // and get_minimenu_length.
        if matches!(
            c.command,
            "get_active_minimenu_entry" | "get_second_minimenu_entry"
        ) {
            let m = &self.properties.minimenu;
            let view = m.view(if c.command == "get_active_minimenu_entry" {
                m.active
            } else {
                m.secondary
            });
            let quest_text = if let Some(quest_text) = view.quest_text {
                quest_text
            } else if let Some(obj_id) = m.object_id(if c.command == "get_active_minimenu_entry" {
                m.active
            } else {
                m.secondary
            }) {
                let obj = self
                    .properties
                    .objs
                    .as_ref()
                    .and_then(|objs| {
                        u32::try_from(obj_id)
                            .ok()
                            .and_then(|id| objs.get(id))
                            .map(|obj| (obj, objs.allow_members.get()))
                    })
                    .ok_or_else(|| vm::VmError::TrapFailed {
                        command: c.command.into(),
                        reason: "object quest config not installed".into(),
                    })?;
                // postDecode: `quests = null` for a members
                // object without allowMembers.
                let obj_quests: &[i32] = if obj.0.members && !obj.1 {
                    &[]
                } else {
                    &obj.0.inventory.quests
                };
                let quests = match &self.domains {
                    Domains::Game(v) => v.state.quests.as_ref(),
                    Domains::Plain { .. } => None,
                }
                .ok_or_else(|| vm::VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "quest config not installed".into(),
                })?;
                let mut tags = String::new();
                for &quest_id in obj_quests {
                    let quest =
                        quests
                            .quest(quest_id)
                            .map_err(|error| vm::VmError::TrapFailed {
                                command: c.command.into(),
                                reason: format!("quest {quest_id}: {error:#}"),
                            })?;
                    if quest.icon_sprite != -1 {
                        tags.push_str(&format!(" <sprite={}>", quest.icon_sprite));
                    }
                }
                tags
            } else {
                return Err(vm::VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "NPC/loc quest config not installed".into(),
                });
            };
            i.push(view.entity_type);
            s.push(view.op);
            s.push(view.op_base);
            s.push(quest_text);
            return Ok(None);
        }
        if c.command == "get_minimenu_length" {
            i.push(self.properties.minimenu.option_count);
            i.push(self.properties.minimenu.submenu_count);
            return Ok(None);
        }
        if c.command == "array_sort" {
            if i.len() < 3 {
                return Err(vm::VmError::StackUnderflow { stack: "int" });
            }
            let a = i.split_off(i.len() - 3);
            self.context.arrays.sort(a[0], a[1], a[2])?;
            return Ok(None);
        }
        // Commands whose original owners (canvas, fonts, minimenu,
        // preferences, language, local player) are lent to this host.
        if crate::ui_runtime::host_builtins::hook_handles(c.command) {
            let vars = match &mut self.domains {
                Domains::Game(v) => Some(&mut **v),
                Domains::Plain { .. } => None,
            };
            return crate::ui_runtime::host_builtins::dispatch_hook(
                c.command,
                self.properties,
                vars,
                i,
                s,
            );
        }
        let mut dispatch = |changes: &mut Changes, now: &mut dyn FnMut() -> i64| {
            let mut h = ScriptHost {
                engine: self.engine,
                store: self.store,
                active: &mut self.context.active,
                properties: Some(Context {
                    state: self.properties,
                    changes,
                    now,
                    nested_count: self.context.nested_count,
                }),
            };
            h.trap_context(c, i, s, l)
        };
        match &mut self.domains {
            Domains::Game(v) => dispatch(&mut v.state.delayed, &mut v.now),
            Domains::Plain { changes, now } => dispatch(changes, now),
        }
    }
    fn var_type(&mut self, d: native910::vars::VarScope, id: u16) -> vm::VmResult<vm::VarLane> {
        match &mut self.domains {
            Domains::Game(v) => v.var_type(d, id).map_err(error),
            _ => self.engine.var_type(d, id),
        }
    }
    fn var_integer_default(&mut self, d: native910::vars::VarScope, id: u16) -> vm::VmResult<i32> {
        match &mut self.domains {
            Domains::Game(variables) => variables.integer_default(d, id).map_err(error),
            _ => self.engine.var_integer_default(d, id),
        }
    }
    fn var_get(
        &mut self,
        d: native910::vars::VarScope,
        id: u16,
        s: bool,
    ) -> vm::VmResult<vm::Value> {
        match &mut self.domains {
            Domains::Game(v) => v.get(d, id, s).map_err(error),
            _ => self.engine.var_get(d, id, s),
        }
    }
    fn var_set(
        &mut self,
        d: native910::vars::VarScope,
        id: u16,
        s: bool,
        v: vm::Value,
    ) -> vm::VmResult<()> {
        match &mut self.domains {
            Domains::Game(vars) => vars.set(d, id, s, v).map_err(error),
            _ => self.engine.var_set(d, id, s, v),
        }
    }
    fn varbit_get(&mut self, id: u16, s: bool) -> vm::VmResult<i32> {
        match &mut self.domains {
            Domains::Game(v) => v.get_bit(id, s).map_err(error),
            _ => self.engine.varbit_get(id, s),
        }
    }
    fn varbit_set(&mut self, id: u16, s: bool, value: i32) -> vm::VmResult<()> {
        match &mut self.domains {
            Domains::Game(v) => v.set_bit(id, s, value).map_err(error),
            _ => self.engine.varbit_set(id, s, value),
        }
    }
    fn array_define(&mut self, id: i32, n: usize) -> vm::VmResult<()> {
        self.context.arrays.define(id, n)
    }
    fn array_len(&mut self, id: i32) -> vm::VmResult<usize> {
        self.context.arrays.len(id)
    }
    fn array_get(&mut self, id: i32, n: i32) -> vm::VmResult<i32> {
        self.context.arrays.get(id, n)
    }
    fn array_set(&mut self, id: i32, n: i32, v: i32) -> vm::VmResult<()> {
        self.context.arrays.set(id, n, v)
    }
    fn take_effects(&mut self) -> Vec<vm::HostEffect> {
        self.engine.take_effects()
    }
}
pub struct Execution {
    pub nested: Vec<Execution>,
    pub missing: Vec<i32>,
    pub id: i32,
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub limit: usize,
    pub result: vm::VmResult<()>,
    pub snapshot: Snapshot,
}
/// One script execution: the script, the request that starts it (locals and
/// component) and the step limit.
pub struct ScriptRun<'a> {
    pub id: i32,
    pub script: &'a CompiledScript,
    pub request: &'a Request,
    pub limit: usize,
}
/// Explicit locals of a triggered script: the int locals and, for cutscene
/// subtitles, the object (string) locals.
#[derive(Default)]
pub struct TriggerLocals<'a> {
    pub ints: &'a [i32],
    pub strings: Option<Vec<crate::ui_components::Text>>,
}
impl Pool {
    pub fn execute<H: Host, P: ScriptProvider>(
        &mut self,
        store: &mut Store,
        state: &mut State,
        run: ScriptRun<'_>,
        mut domains: Domains<'_, '_>,
        engine: &mut H,
        provider: &P,
    ) -> Result<Execution> {
        let ScriptRun {
            id,
            script,
            request,
            limit,
        } = run;
        let index = self.acquire();
        self.prepare(index, script, request)?;
        let mut session = self.session(index, id, script, limit)?;
        let mut nested = vec![];
        let mut missing = vec![];
        let result = 'execute: loop {
            let step = {
                let mut host = HookHost {
                    engine,
                    store,
                    properties: state,
                    context: &mut self.contexts[index],
                    domains: domains.reborrow(),
                };
                let mut vm = Vm::new(&mut host, provider);
                vm.step(&mut session)
            };
            // triggerOp executes its onop hook synchronously before the
            // calling script's next instruction; it borrows a fresh context.
            while let Some(action) = state.interaction.script_actions.pop_front() {
                let mut runner = Runner {
                    pool: self,
                    provider,
                    engine,
                    domains: domains.reborrow(),
                    executions: vec![],
                    missing: vec![],
                };
                let dispatched = crate::ui_interaction::dispatch(store, state, &mut runner, action);
                nested.extend(runner.executions);
                missing.extend(runner.missing);
                if let Err(e) = dispatched {
                    break 'execute Err(error(e));
                }
            }
            match step {
                Ok(true) => break Ok(()),
                Ok(false) => {}
                Err(e) => break Err(e),
            }
        };
        let snapshot = session.snapshot();
        self.finish(index, &session);
        Ok(Execution {
            nested,
            missing,
            id,
            limit,
            result,
            snapshot,
        })
    }
}
/// Prepared script cache/provider. Missing root scripts are skipped before
/// reserving a pooled context, just as getScript returns null.
pub struct Runner<'a, 'd, H, P> {
    pub pool: &'a mut Pool,
    pub provider: &'a P,
    pub engine: &'a mut H,
    pub domains: Domains<'a, 'd>,
    pub executions: Vec<Execution>,
    pub missing: Vec<i32>,
}
impl<H: Host, P: ScriptProvider> Runner<'_, '_, H, P> {
    #[cfg(any(test, feature = "test-hooks"))] // test-only drain
    pub fn drain_main(&mut self, store: &mut Store, state: &mut State) -> Result<()> {
        crate::ui_hooks::drain_main(store, state, self)
    }
    /// Client's packet lifecycle executes hooks immediately, then queues data
    /// changes in the very same domain used by those hooks' script writes.
    pub fn packet(
        &mut self,
        store: &mut Store,
        state: &mut State,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        for update in crate::ui_lifecycle::packet(store, state, event, self)? {
            match &mut self.domains {
                // A client varc-bit set merges into the base varc's type-1 node at enqueue
                // time; a new node starts from the live client value.
                Domains::Game(v) if update.kind == 24 => v.state.set_varc_bit(
                    v.definitions,
                    update.target as i32,
                    update.ints[0].context("SETVARCBIT value")?,
                )?,
                Domains::Game(v) => update.enqueue(&mut v.state.delayed),
                Domains::Plain { .. } if update.kind == 24 => {
                    anyhow::bail!("SETVARCBIT requires the client variable domain")
                }
                Domains::Plain { changes, .. } => update.enqueue(changes),
            }
        }
        Ok(())
    }

    /// Execute a script resolved through the original client's trigger-group lookup. Trigger
    /// scripts have no component hook request, but still use the same pooled
    /// locals, variable domains, VM limit and execution diagnostics.
    pub fn run_compiled(
        &mut self,
        store: &mut Store,
        state: &mut State,
        script_id: i32,
        script: &CompiledScript,
        limit: usize,
    ) -> Result<()> {
        self.run_compiled_with_ints(store, state, script_id, script, limit, &[])
    }

    pub fn run_compiled_with_ints(
        &mut self,
        store: &mut Store,
        state: &mut State,
        script_id: i32,
        script: &CompiledScript,
        limit: usize,
        trigger_ints: &[i32],
    ) -> Result<()> {
        self.run_compiled_with_locals(
            store,
            state,
            script_id,
            script,
            limit,
            TriggerLocals {
                ints: trigger_ints,
                strings: None,
            },
        )
    }

    /// Trigger execution with explicit int and object locals (cutscene
    /// triggers).
    pub fn run_compiled_with_locals(
        &mut self,
        store: &mut Store,
        state: &mut State,
        script_id: i32,
        script: &CompiledScript,
        limit: usize,
        locals: TriggerLocals<'_>,
    ) -> Result<()> {
        let request = Request {
            args: Some(vec![crate::ui_components::Arg::Int(script_id)]),
            trigger_ints: Some(locals.ints.to_vec()),
            trigger_strings: locals.strings,
            ..Request::default()
        };
        let execution = self.pool.execute(
            store,
            state,
            ScriptRun {
                id: script_id,
                script,
                request: &request,
                limit,
            },
            self.domains.reborrow(),
            self.engine,
            self.provider,
        )?;
        self.executions.push(execution);
        Ok(())
    }
}
impl<H: Host, P: ScriptProvider> Executor for Runner<'_, '_, H, P> {
    fn run(
        &mut self,
        store: &mut Store,
        state: &mut State,
        r: Request,
        limit: usize,
    ) -> Result<()> {
        let id = r.script_id()?;
        let trace = crate::ui_debug_flags::flags().hook_trace;
        let started = trace.then(std::time::Instant::now);
        if trace {
            log::info!(
                "[hook] start script={id} component={:?} depth={} queues={},{},{} canvas={:?}",
                r.component.as_ref().map(|c| {
                    let c = c.borrow();
                    (c.f.parentlayer, c.f.id)
                }),
                r.nested_count,
                state.layout.hooks.len(),
                state.layout.hooks_timer.len(),
                state.layout.hooks_mouse_stop.len(),
                state.layout.canvas
            );
        }
        let Some(script) = self.provider.resolve(id)? else {
            self.missing.push(id);
            return Ok(());
        };
        let execution = self.pool.execute(
            store,
            state,
            ScriptRun {
                id,
                script: &script,
                request: &r,
                limit,
            },
            self.domains.reborrow(),
            self.engine,
            self.provider,
        )?;
        if let Some(started) = started {
            log::info!(
                "[hook] end script={id} steps={} ms={} result={:?}",
                execution.snapshot.steps,
                started.elapsed().as_millis(),
                execution.result
            );
        }
        self.executions.push(execution);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_interaction::Action;
    use native910::script::{Counts, Instruction, Operand};
    use native910::vm::{Host, InstructionContext};

    fn context<'a>(command: &'a str, operand: &'a Operand) -> InstructionContext<'a> {
        InstructionContext {
            script_name: Some("hook-host-test"),
            script_id: None,
            event: None,
            pc: 0,
            command,
            operand,
            secondary: false,
            int_locals: &[],
        }
    }

    #[test]
    fn minimenu_target_query_reads_live_target_owner() {
        let mut store = Store::default();
        let mut state = State::default();
        state.interaction.target.active = true;
        state.interaction.target.name = "Potion".into();
        state.interaction.target.verb = "Use".into();
        let mut execution = ExecutionContext::default();
        let mut engine = crate::ui_runtime::Engine::default();
        let mut changes = Changes::default();
        let mut now = || 0;
        let mut host = HookHost {
            engine: &mut engine,
            store: &mut store,
            properties: &mut state,
            context: &mut execution,
            domains: Domains::Plain {
                changes: &mut changes,
                now: &mut now,
            },
        };
        let operand = Operand::Byte(0);
        let mut ints = Vec::new();
        let mut strings = Vec::new();
        let mut longs = Vec::new();
        assert_eq!(
            host.trap_context(
                &context("get_minimenu_target", &operand),
                &mut ints,
                &mut strings,
                &mut longs,
            )
            .unwrap(),
            None
        );
        assert_eq!(ints, [1]);
        assert_eq!(strings, ["Potion", "Use"]);
    }

    #[test]
    fn script_actions_are_synchronous_but_menu_followups_wait_for_the_hook() {
        let mut store = Store::default();
        let mut state = State::default();
        let mut pool = Pool::default();
        let mut engine = crate::ui_runtime::Engine::default();
        let mut changes = Changes::default();
        let mut now = || 0;
        let c = std::rc::Rc::new(std::cell::RefCell::new(
            crate::ui_components::Component::default(),
        ));
        c.borrow_mut().f.parentlayer = 65536;
        store
            .interfaces
            .insert(1, crate::ui_components::Interface::new(vec![Some(c)]));
        state.interaction.target.active = true;
        state.interaction.target.parent = 65536;
        state.interaction.target.child = -1;
        // useMenuOption queues this after its current operation. The VM must
        // not consume it at the first instruction of that operation's hook.
        state.interaction.actions.push_back(Action::ClearTarget);
        for cancel in [false, true] {
            let commands: Vec<_> = if cancel {
                vec!["targetmode_cancel", "return"]
            } else {
                vec!["return"]
            };
            let script = CompiledScript {
                name: None,
                locals: Counts::default(),
                args: Counts::default(),
                code: commands
                    .into_iter()
                    .map(|command| Instruction {
                        opcode: 0,
                        command: command.into(),
                        operand: Operand::Byte(0),
                    })
                    .collect(),
            };
            let request = Request {
                args: Some(vec![crate::ui_components::Arg::Int(1)]),
                ..Default::default()
            };
            let execution = pool
                .execute(
                    &mut store,
                    &mut state,
                    ScriptRun {
                        id: 1,
                        script: &script,
                        request: &request,
                        limit: 100,
                    },
                    Domains::Plain {
                        changes: &mut changes,
                        now: &mut now,
                    },
                    &mut engine,
                    &(),
                )
                .unwrap();
            assert!(execution.result.is_ok(), "{:?}", execution.result);
            assert_eq!(state.interaction.target.active, !cancel);
            assert_eq!(state.interaction.actions.len(), 1);
            assert!(state.interaction.script_actions.is_empty());
        }
    }
}
