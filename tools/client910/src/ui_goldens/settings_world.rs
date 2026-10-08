//! A logged-in retained-interface session for the settings tests: the
//! recorded login of `settings-tabs`, the ordinary interface operations and
//! mouse clicks the cache scripts expect, keyboard presses, and a scripted
//! stand-in for the native window (the shell's `NativeWindow` over winit
//! cannot run in a test).
use crate::cache::Pack;
use crate::client_game::ClientGame;
use crate::test_support::{ReclaimedAtExit, SimClock};
use crate::ui_preferences::metric::{Backdrop, Benchmark, RendererProbe};
use crate::ui_runtime::{FullscreenMode, Runtime};
use crate::ui_window::NativeWindow;
use rs910_symbols::{component, ComponentId, InterfaceId};
use std::sync::{Arc, Mutex};

/// What the window was asked to do, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowCall {
    Windowed,
    Resizable(bool),
    Fullscreen([i32; 2]),
}

/// A monitor with fixed video modes that records every call. `refuse`
/// makes exclusive fullscreen fail the way a monitor without the mode does.
pub struct ScriptedWindow {
    pub calls: Mutex<Vec<WindowCall>>,
    pub modes: Vec<FullscreenMode>,
    pub refuse: bool,
}

impl ScriptedWindow {
    pub fn new(modes: &[[i32; 2]]) -> Arc<Self> {
        Self::with_refusal(modes, false)
    }

    /// A monitor whose exclusive fullscreen fails.
    pub fn refusing(modes: &[[i32; 2]]) -> Arc<Self> {
        Self::with_refusal(modes, true)
    }

    fn with_refusal(modes: &[[i32; 2]], refuse: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            modes: modes
                .iter()
                .map(|&[width, height]| FullscreenMode {
                    width,
                    height,
                    bit_depth: 32,
                    refresh: 60,
                })
                .collect(),
            refuse,
        })
    }

    pub fn calls(&self) -> Vec<WindowCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl NativeWindow for ScriptedWindow {
    fn video_modes(&self) -> Vec<FullscreenMode> {
        self.modes.clone()
    }
    fn set_windowed(&self) {
        self.calls.lock().unwrap().push(WindowCall::Windowed);
    }
    fn set_resizable(&self, resizable: bool) {
        self.calls
            .lock()
            .unwrap()
            .push(WindowCall::Resizable(resizable));
    }
    fn enter_fullscreen(&self, size: [i32; 2]) -> Option<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(WindowCall::Fullscreen(size));
        Some(!self.refuse)
    }
}

/// What the profiling commands asked the renderer for, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeEvent {
    /// A message box over `backdrop`; `drawn` when it has pixels.
    Box { backdrop: Backdrop, drawn: bool },
    /// A device benchmark.
    Benchmark { budget_ms: i64, canvas: [u32; 2] },
}

/// A renderer that times nothing: it records what it is asked and answers a
/// benchmark with the next scripted rate (an error when the script has run
/// out, which the command reports as -1).
pub struct ScriptedProbe {
    pub events: Vec<ProbeEvent>,
    pub rates: std::collections::VecDeque<i32>,
    pub canvas: [u32; 2],
}

impl RendererProbe for ScriptedProbe {
    fn canvas_size(&self) -> [u32; 2] {
        self.canvas
    }
    fn frame_size(&self) -> [i32; 2] {
        self.canvas.map(|v| v as i32)
    }
    fn present_message_box(
        &mut self,
        plan: crate::ui_paint::Plan,
        backdrop: Backdrop,
    ) -> anyhow::Result<()> {
        self.events.push(ProbeEvent::Box {
            backdrop,
            drawn: !plan.quads.is_empty(),
        });
        Ok(())
    }
    fn benchmark(&mut self, request: &Benchmark<'_>) -> anyhow::Result<i32> {
        self.events.push(ProbeEvent::Benchmark {
            budget_ms: request.budget_ms,
            canvas: request.canvas,
        });
        self.rates
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("no scripted rate"))
    }
}

pub struct SettingsWorld {
    /// The renderer the profiling commands measure on (see [`ScriptedProbe`]).
    pub probe: ScriptedProbe,
    pub pack: Pack,
    pub game: ReclaimedAtExit<ClientGame>,
    pub clock: SimClock,
    pub ui: ReclaimedAtExit<Runtime>,
    pub window: Arc<ScriptedWindow>,
    rows: serde_json::Value,
    next_row: usize,
}

impl SettingsWorld {
    /// Logs in from the recorded frames of `settings-tabs` on a 1280x720
    /// canvas, with a scripted window of the given video modes and the
    /// preferences saved to `<scratch>/<name>/preferences.dat`.
    pub fn login(name: &str, modes: &[[i32; 2]]) -> anyhow::Result<Self> {
        Self::login_on(name, ScriptedWindow::new(modes))
    }

    /// [`Self::login`] over a given scripted window.
    pub fn login_on(name: &str, window: Arc<ScriptedWindow>) -> anyhow::Result<Self> {
        let pack = crate::test_support::require_pack("client.config.js5");
        let rows = crate::test_support::replay_json("settings-tabs", "frames.json");
        let mut game = ClientGame::login(
            &pack,
            1,
            crate::protocol910::live::Feed::default(),
            910,
            true,
        )?;
        let mut ui = Runtime::new(pack.clone())?;
        ui.diagnostics.capture = true;
        ui.resize([1280, 720])?;
        ui.engine.account.logged_in_members = true;
        ui.engine.account.player_is_members = true;
        let dir = crate::test_support::proof_dir("settings-world").join(name);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("preferences.dat");
        let _ = std::fs::remove_file(&path);
        let preferences = &mut game.ui_variables.queries.preferences;
        preferences.install(path);
        preferences.anti_aliasing = true;
        preferences.bloom = true;
        preferences.window.native = Some(window.clone());
        preferences.window.install_size([1280, 720], 1.0);
        let mut world = Self {
            probe: ScriptedProbe {
                events: Vec::new(),
                rates: Default::default(),
                canvas: [1280, 720],
            },
            pack,
            game: ReclaimedAtExit::new(game),
            clock: SimClock::default(),
            ui: ReclaimedAtExit::new(ui),
            window,
            rows,
            next_row: 0,
        };
        world.replay_rows(None)?;
        Ok(world)
    }

    /// Replays the recorded server frames of the next row (the login on the
    /// first call), or up to and including the first row of `tab`.
    fn replay_rows(&mut self, tab: Option<u64>) -> anyhow::Result<()> {
        loop {
            let row = self.rows[self.next_row].clone();
            self.apply(&row["frames"])?;
            self.next_row += 1;
            if tab.is_none_or(|tab| row["tab"].as_u64() == Some(tab)) {
                return Ok(());
            }
        }
    }

    fn apply(&mut self, frames: &serde_json::Value) -> anyhow::Result<()> {
        for raw in frames.as_array().unwrap() {
            let bytes: Vec<u8> = raw
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap() as u8)
                .collect();
            let (frame, _) =
                crate::net::decode_frame(&bytes)?.ok_or_else(|| anyhow::anyhow!("frame"))?;
            if self.game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                let cycle = self.game.cycle as i64;
                self.game
                    .apply_next(cycle)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                if self.game.runtime.map_request.is_some() {
                    let map = self.game.runtime.prepare_map(&self.pack)?;
                    self.game
                        .runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                }
            } else if let Some(event) =
                crate::session::parse_ui_event(frame.opcode, &frame.payload)?
            {
                let (ui, clock) = (&mut self.ui, &self.clock);
                clock.with(&mut self.game, |vars| ui.packet(vars, &event))?;
            }
        }
        let clock = &self.clock;
        self.game
            .poll_vars(|| clock.0)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        self.ticks(40)
    }

    /// A new login of the same account: a fresh game and interface runtime
    /// built from the recorded login frames, keeping the client variables
    /// (where the key bindings live) and the saved preferences.
    pub fn relogin(&mut self) -> anyhow::Result<()> {
        let client = self.game.ui_variables.client.clone();
        let path = self.preferences().path.clone().unwrap();
        self.preferences_mut().dirty = true;
        self.preferences_mut().save()?;
        let mut game = ClientGame::login(
            &self.pack,
            1,
            crate::protocol910::live::Feed::default(),
            910,
            true,
        )?;
        let mut ui = Runtime::new(self.pack.clone())?;
        ui.diagnostics.capture = true;
        ui.resize([1280, 720])?;
        ui.engine.account.logged_in_members = true;
        ui.engine.account.player_is_members = true;
        game.ui_variables.client = client;
        let preferences = &mut game.ui_variables.queries.preferences;
        preferences.install(path);
        preferences.anti_aliasing = true;
        preferences.bloom = true;
        preferences.window.native = Some(self.window.clone());
        preferences.window.install_size([1280, 720], 1.0);
        self.game = ReclaimedAtExit::new(game);
        self.ui = ReclaimedAtExit::new(ui);
        self.next_row = 0;
        self.replay_rows(None)
    }

    /// Opens a settings tab by replaying the recorded server frames of its
    /// first visit (1 Gameplay, 2 Graphics, 3 Controls, 4 Audio).
    pub fn open_tab(&mut self, tab: u64) -> anyhow::Result<()> {
        self.replay_rows(Some(tab))
    }

    /// Runs logic cycles as the client's main loop does: the variable
    /// transmits are polled first, so a varbit a script wrote reaches the
    /// `onvartransmit` hooks, then the retained interface updates.
    pub fn ticks(&mut self, count: usize) -> anyhow::Result<()> {
        for _ in 0..count {
            self.game.cycle += 1;
            let clock = &self.clock;
            self.game
                .poll_vars(|| clock.0)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            self.clock
                .tick_probed(&mut self.game, &mut self.ui, Some(&mut self.probe))?;
        }
        Ok(())
    }

    /// A button press on `component` (`child` -1 for the component itself),
    /// run through the cache's `onop` hooks.
    pub fn op(&mut self, component: ComponentId, child: i32) -> anyhow::Result<()> {
        self.ui
            .state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: component.packed(),
                child,
                base: None,
            });
        self.ticks(6)
    }

    /// Picks item `index` of the open dropdown list.
    pub fn pick(&mut self, index: i32) -> anyhow::Result<()> {
        self.op(component::game_window::DROPDOWN_LIST_OPTIONS, index * 2 + 1)
    }

    /// Opens dropdown `control` of the graphics panel and picks `index`.
    pub fn dropdown(&mut self, control: i32, index: i32) -> anyhow::Result<()> {
        self.op(component::graphics_settings_panel::CONTROL_LIST, control)?;
        self.pick(index)
    }

    /// The screen position of a component's top-left corner: its laid-out
    /// position summed up its chain of parent layers, then the position of
    /// the component its interface is mounted on.
    fn origin(&mut self, id: i32) -> anyhow::Result<[i32; 2]> {
        let mut node = self
            .ui
            .store
            .get(id, -1)?
            .ok_or_else(|| anyhow::anyhow!("component {id:#x} missing"))?;
        let (mut x, mut y) = (0, 0);
        loop {
            let (layer, dx, dy) = {
                let c = node.borrow();
                (c.f.layer, c.f.x, c.f.y)
            };
            x += dx;
            y += dy;
            if layer == -1 {
                break;
            }
            let Some(parent) = self.ui.store.get(layer, -1)? else {
                break;
            };
            {
                let p = parent.borrow();
                x -= p.f.scrollx;
                y -= p.f.scrolly;
            }
            node = parent;
        }
        let group = id >> 16;
        let mount = self
            .ui
            .state
            .life
            .subs
            .ordered()
            .find(|n| n.borrow().id == group)
            .map(|n| n.borrow().parent);
        if let Some(mount) = mount {
            let [mx, my] = self.origin(mount)?;
            x += mx;
            y += my;
        }
        Ok([x, y])
    }

    /// The screen centre of a component, or of one of its dynamic children.
    fn centre_of(&mut self, component: ComponentId, child: i32) -> anyhow::Result<[i32; 2]> {
        let id = component.packed();
        let [mut x, mut y] = self.origin(id)?;
        if child != -1 {
            let parent = self.ui.store.get(id, -1)?.unwrap();
            let p = parent.borrow();
            x -= p.f.scrollx;
            y -= p.f.scrolly;
        }
        let (dx, dy, width, height) = {
            let c = self.ui.store.get(id, child)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "component {}:{}/{child} missing",
                    component.interface().id(),
                    component.child()
                )
            })?;
            let c = c.borrow();
            if child == -1 {
                (0, 0, c.f.width, c.f.height)
            } else {
                (c.f.x, c.f.y, c.f.width, c.f.height)
            }
        };
        Ok([x + dx + width / 2, y + dy + height / 2])
    }

    /// A left click on the centre of a component, as the window shell feeds
    /// the mouse to the retained interface.
    pub fn click(&mut self, component: ComponentId) -> anyhow::Result<()> {
        self.click_child(component, -1)
    }

    /// A left click on the centre of a dynamic child of a component.
    pub fn click_child(&mut self, component: ComponentId, child: i32) -> anyhow::Result<()> {
        let at = self.centre_of(component, child)?;
        self.click_at(at)
    }

    /// A left click at a canvas position, as the window shell feeds the mouse
    /// to the retained interface.
    pub fn click_at(&mut self, at: [i32; 2]) -> anyhow::Result<()> {
        // The pointer arrives first, so the hover pass has seen the target.
        self.ui.engine.platform.pending_mouse = at;
        self.ticks(2)?;
        self.ui.input.click = Some(at);
        self.ui.input.left_held = true;
        self.ui.input.event = Some(crate::ui_defaults::MouseEvent {
            pos: at,
            action: 0,
            count: 1,
        });
        self.ticks(1)?;
        self.ui.input.left_held = false;
        self.ticks(6)
    }

    /// Presses and releases a key by its AWT code, holding it `hold` cycles.
    pub fn press(&mut self, awt: i32, hold: usize) -> anyhow::Result<()> {
        self.ui.keyboard.key(awt, 0, self.clock.0);
        self.ticks(hold)?;
        self.ui.keyboard.key(awt, 1, self.clock.0);
        self.ticks(2)
    }

    pub fn options(&self) -> &crate::client_options::ClientOptions {
        &self.game.ui_variables.queries.preferences.options
    }

    pub fn preferences(&self) -> &crate::ui_preferences::Preferences {
        &self.game.ui_variables.queries.preferences
    }

    pub fn preferences_mut(&mut self) -> &mut crate::ui_preferences::Preferences {
        &mut self.game.ui_variables.queries.preferences
    }

    /// The text of a component as the interface shows it.
    pub fn text(&mut self, component: ComponentId) -> anyhow::Result<String> {
        let node = self.ui.store.get(component.packed(), -1)?.ok_or_else(|| {
            anyhow::anyhow!(
                "component {}:{} missing",
                component.interface().id(),
                component.child()
            )
        })?;
        let c = node.borrow();
        Ok(c.f
            .text
            .as_ref()
            .map(|t| String::from_utf16_lossy(t))
            .unwrap_or_default())
    }

    /// The ids of the sub-interfaces currently attached.
    pub fn subs(&self) -> Vec<i32> {
        self.ui
            .state
            .life
            .subs
            .ordered()
            .map(|n| n.borrow().id)
            .collect()
    }

    /// Whether `interface` is attached as a sub-interface.
    pub fn is_open(&self, interface: InterfaceId) -> bool {
        self.subs().contains(&interface.id())
    }

    /// Saves the preferences and loads them back under the same profile.
    pub fn reload_options(&mut self) -> anyhow::Result<crate::client_options::ClientOptions> {
        let preferences = self.preferences_mut();
        preferences.dirty = true;
        preferences.save()?;
        let path = preferences.path.clone().unwrap();
        Ok(crate::client_options::ClientOptions::load(
            &path,
            preferences.options.profile,
        ))
    }
}
