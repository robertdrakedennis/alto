//! Deterministic input injectors, parsed once at startup.
//!
//! The diagnostic `CLIENT910_*` injectors used by headless runs and replays
//! used to be re-read and string-parsed from the environment every logic
//! cycle in `ViewerApp::about_to_wait`. [`InputScript`] parses them once
//! (keeping each injector's exact parse rules: trimming, dropped fields,
//! arity checks) and yields, per logic cycle, the same ordered events the
//! inline parsers produced. `about_to_wait` applies them at the same
//! sub-phases as before:
//!
//! 1. [`InputScript::window_resizes`] right after `logic_cycle++`
//!    (`CLIENT910_WINDOW_RESIZES`);
//! 2. [`InputScript::ui_injections`] after `client_watch.mainloop`, in the
//!    fixed order KEY_INPUT, TYPE_INPUT, WHEEL_INPUT, TOOLKIT_INPUT,
//!    UI_OPERATIONS, UI_INPUT, UI_CLICK, UI_HOVER, UI_CLICKS;
//! 3. [`InputScript::drop_connection`] in `inject_connection_drop`;
//! 4. [`InputScript::cutscene_fixture`] after `poll_live_rebuild`.

use std::ffi::OsString;

/// One injected UI input, in the order `about_to_wait` applies it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiInjection {
    /// `CLIENT910_KEY_INPUT=cycle,code,mode;...`: `keyboard.key(code, mode)`.
    Key { code: i32, mode: i32 },
    /// `CLIENT910_TYPE_INPUT=cycle:text;...` with `\n`/`\t` already unescaped.
    Type { text: String },
    /// `CLIENT910_WHEEL_INPUT=cycle,delta;...`.
    Wheel { delta: i32 },
    /// `CLIENT910_TOOLKIT_INPUT=cycle,id;...`: `detail_toolkit(id)`.
    Toolkit { cycle: i32, id: i32 },
    /// `CLIENT910_UI_OPERATIONS=cycle,parent,child,op;...` (op 0: the
    /// component's pause button).
    Operation { parent: i32, child: i32, op: i32 },
    /// `CLIENT910_UI_INPUT=cycle,x,y,held,press;...`.
    Gesture {
        cycle: i32,
        x: i32,
        y: i32,
        held: i32,
        press: i32,
    },
    /// `CLIENT910_UI_CLICK=x,y,first[,last[,action]];...` while
    /// `first <= cycle <= last`; `first` marks the press cycle.
    Click {
        x: i32,
        y: i32,
        action: i32,
        first: bool,
    },
    /// A `CLIENT910_UI_CLICK` entry without 3..=5 numbers (reported on
    /// cycles `% 100 == 1`).
    ClickMalformed { spec: String },
    /// `CLIENT910_UI_HOVER=x,y,first[,last];...` while in range.
    Hover { x: i32, y: i32 },
    /// `CLIENT910_UI_CLICKS=b,x,y,cycle;...` press at `cycle`.
    ClicksPress {
        button: String,
        x: i32,
        y: i32,
        cycle: i32,
        action: i32,
    },
    /// `CLIENT910_UI_CLICKS` release on `cycle + 1`.
    ClicksRelease,
}

#[derive(Clone, Debug)]
enum ClickSpec {
    Valid {
        x: i32,
        y: i32,
        first: i32,
        last: i32,
        action: i32,
    },
    Malformed(String),
}

#[derive(Clone, Debug)]
struct ClicksSpec {
    button: String,
    x: i32,
    y: i32,
    cycle: i32,
    action: i32,
}

/// The `CLIENT910_CUTSCENE=id,cycle[,capacity]` fixture request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CutsceneFixture {
    pub id: i32,
    pub cycle: i32,
    pub capacity: Option<i32>,
}

/// All injectors, pre-parsed. Empty (the normal case) costs nothing per cycle.
#[derive(Clone, Debug, Default)]
pub struct InputScript {
    window_resizes: Vec<Vec<i32>>,
    key: Vec<Vec<i32>>,
    type_text: Vec<(Option<i32>, String)>,
    wheel: Vec<Vec<i32>>,
    toolkit: Vec<Vec<i32>>,
    operations: Vec<Vec<i32>>,
    gestures: Vec<Vec<i32>>,
    click: Vec<ClickSpec>,
    hover: Vec<Vec<i32>>,
    clicks: Vec<ClicksSpec>,
    cutscene: Option<Vec<i32>>,
    drop_connection: Option<i32>,
    any_ui: bool,
}

/// `row.split(',').filter_map(|v| v.trim().parse().ok())` per `;` row.
fn trimmed_rows(spec: &str) -> Vec<Vec<i32>> {
    spec.split(';')
        .map(|row| {
            row.split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect()
        })
        .collect()
}

/// `row.split(',').filter_map(|n| n.parse().ok())` per `;` row (untrimmed).
fn untrimmed_rows(spec: &str) -> Vec<Vec<i32>> {
    spec.split(';')
        .map(|row| row.split(',').filter_map(|n| n.parse().ok()).collect())
        .collect()
}

impl InputScript {
    /// Parse from any variable source (`std::env::var_os` in production).
    /// Variables read with `env::var` before stay UTF-8-only; `var_os` ones
    /// stay lossy.
    pub fn from_lookup(var_os: &dyn Fn(&str) -> Option<OsString>) -> Self {
        let lossy = |name: &str| var_os(name).map(|v| v.to_string_lossy().into_owned());
        let utf8 = |name: &str| var_os(name).and_then(|v| v.into_string().ok());
        let mut script = Self {
            window_resizes: utf8("CLIENT910_WINDOW_RESIZES")
                .map(|s| untrimmed_rows(&s))
                .unwrap_or_default(),
            key: lossy("CLIENT910_KEY_INPUT")
                .map(|s| trimmed_rows(&s))
                .unwrap_or_default(),
            type_text: lossy("CLIENT910_TYPE_INPUT")
                .map(|spec| {
                    spec.split(';')
                        .filter_map(|row| row.split_once(':'))
                        .map(|(cycle, text)| {
                            (
                                cycle.trim().parse::<i32>().ok(),
                                text.replace("\\n", "\n").replace("\\t", "\t"),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            wheel: lossy("CLIENT910_WHEEL_INPUT")
                .map(|s| trimmed_rows(&s))
                .unwrap_or_default(),
            toolkit: lossy("CLIENT910_TOOLKIT_INPUT")
                .map(|s| trimmed_rows(&s))
                .unwrap_or_default(),
            operations: utf8("CLIENT910_UI_OPERATIONS")
                .map(|s| untrimmed_rows(&s))
                .unwrap_or_default(),
            gestures: lossy("CLIENT910_UI_INPUT")
                .map(|s| trimmed_rows(&s))
                .unwrap_or_default(),
            click: lossy("CLIENT910_UI_CLICK")
                .map(|spec| {
                    spec.split(';')
                        .map(|spec| {
                            let nums: Vec<i32> = spec
                                .split(',')
                                .filter_map(|p| p.trim().parse().ok())
                                .collect();
                            if (3..=5).contains(&nums.len()) {
                                let first = nums[2];
                                ClickSpec::Valid {
                                    x: nums[0],
                                    y: nums[1],
                                    first,
                                    last: if nums.len() >= 4 { nums[3] } else { first },
                                    action: nums.get(4).copied().unwrap_or(0),
                                }
                            } else {
                                ClickSpec::Malformed(spec.to_string())
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            hover: lossy("CLIENT910_UI_HOVER")
                .map(|s| {
                    trimmed_rows(&s)
                        .into_iter()
                        .filter(|n| n.len() >= 3)
                        .collect()
                })
                .unwrap_or_default(),
            clicks: lossy("CLIENT910_UI_CLICKS")
                .map(|spec| {
                    spec.split(';')
                        .filter_map(|item| {
                            let parts: Vec<&str> = item.split(',').map(str::trim).collect();
                            let [b, x, y, c] = parts[..] else { return None };
                            let (Ok(x), Ok(y), Ok(cycle)) =
                                (x.parse::<i32>(), y.parse::<i32>(), c.parse::<i32>())
                            else {
                                return None;
                            };
                            let action = match b {
                                "l" => 0,
                                "m" => 1,
                                "r" => 2,
                                _ => return None,
                            };
                            Some(ClicksSpec {
                                button: b.to_string(),
                                x,
                                y,
                                cycle,
                                action,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            cutscene: lossy("CLIENT910_CUTSCENE").map(|spec| {
                spec.split(',')
                    .filter_map(|v| v.trim().parse().ok())
                    .collect()
            }),
            drop_connection: utf8("CLIENT910_DROP_CONNECTION")
                .and_then(|v| v.trim().parse::<i32>().ok()),
            any_ui: false,
        };
        script.any_ui = !(script.key.is_empty()
            && script.type_text.is_empty()
            && script.wheel.is_empty()
            && script.toolkit.is_empty()
            && script.operations.is_empty()
            && script.gestures.is_empty()
            && script.click.is_empty()
            && script.hover.is_empty()
            && script.clicks.is_empty());
        script
    }

    /// `CLIENT910_WINDOW_RESIZES=cycle,w,h;...`: logical sizes requested
    /// at `cycle`.
    pub fn window_resizes(&self, cycle: i32) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.window_resizes
            .iter()
            .filter(move |a| a.len() == 3 && a[0] == cycle && a[1] > 0 && a[2] > 0)
            .map(|a| (a[1], a[2]))
    }

    /// The UI injections of `cycle`, in the former inline order.
    pub fn ui_injections(&self, cycle: i32) -> Vec<UiInjection> {
        let mut out = Vec::new();
        if !self.any_ui {
            return out;
        }
        for a in &self.key {
            if a.len() == 3 && a[0] == cycle {
                out.push(UiInjection::Key {
                    code: a[1],
                    mode: a[2],
                });
            }
        }
        for (at, text) in &self.type_text {
            if *at == Some(cycle) {
                out.push(UiInjection::Type { text: text.clone() });
            }
        }
        for a in &self.wheel {
            if a.len() == 2 && a[0] == cycle {
                out.push(UiInjection::Wheel { delta: a[1] });
            }
        }
        for a in &self.toolkit {
            if a.len() == 2 && a[0] == cycle {
                out.push(UiInjection::Toolkit {
                    cycle: a[0],
                    id: a[1],
                });
            }
        }
        for a in &self.operations {
            if a.len() == 4 && a[0] == cycle {
                out.push(UiInjection::Operation {
                    parent: a[1],
                    child: a[2],
                    op: a[3],
                });
            }
        }
        for a in &self.gestures {
            if a.len() == 5 && a[0] == cycle {
                out.push(UiInjection::Gesture {
                    cycle: a[0],
                    x: a[1],
                    y: a[2],
                    held: a[3],
                    press: a[4],
                });
            }
        }
        for spec in &self.click {
            match spec {
                ClickSpec::Valid {
                    x,
                    y,
                    first,
                    last,
                    action,
                } => {
                    if cycle >= *first && cycle <= *last {
                        out.push(UiInjection::Click {
                            x: *x,
                            y: *y,
                            action: *action,
                            first: cycle == *first,
                        });
                    }
                }
                ClickSpec::Malformed(spec) => {
                    if cycle % 100 == 1 {
                        out.push(UiInjection::ClickMalformed { spec: spec.clone() });
                    }
                }
            }
        }
        for nums in &self.hover {
            let last = nums.get(3).copied().unwrap_or(nums[2]);
            if cycle >= nums[2] && cycle <= last {
                out.push(UiInjection::Hover {
                    x: nums[0],
                    y: nums[1],
                });
            }
        }
        for c in &self.clicks {
            if cycle == c.cycle {
                out.push(UiInjection::ClicksPress {
                    button: c.button.clone(),
                    x: c.x,
                    y: c.y,
                    cycle: c.cycle,
                    action: c.action,
                });
            } else if cycle == c.cycle + 1 {
                out.push(UiInjection::ClicksRelease);
            }
        }
        out
    }

    /// `CLIENT910_DROP_CONNECTION=cycle` (trimmed).
    pub fn drop_connection(&self) -> Option<i32> {
        self.drop_connection
    }

    /// `CLIENT910_CUTSCENE` is set (enables the fixture's CAM_RESET return).
    pub fn cutscene_enabled(&self) -> bool {
        self.cutscene.is_some()
    }

    /// The `CLIENT910_CUTSCENE` fixture to inject at `cycle`.
    pub fn cutscene_fixture(&self, cycle: i32) -> Option<CutsceneFixture> {
        let a = self.cutscene.as_ref()?;
        (a.len() >= 2 && a[1] == cycle).then(|| CutsceneFixture {
            id: a[0],
            cycle: a[1],
            capacity: a.get(2).copied(),
        })
    }
}

#[cfg(test)]
mod tests;
