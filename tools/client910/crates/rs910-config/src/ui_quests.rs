//! Quest definitions (config group 35, preloaded) and the quest script
//! commands. Requirement checks
//! read the local player's varps/varbits and stats through [`Player`].
use crate::cache::Pack;
use crate::opcode_table::{
    at, decode_record, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown,
};
use anyhow::{anyhow, Context, Result};
use native910::vm::{Value, VmError, VmResult};
use rs910_core::fault::Fault;
use std::collections::BTreeMap;

/// The var and stat reads a quest requirement check needs from the local player.
pub trait Player {
    /// Player var value.
    fn varp(&self, id: i32) -> Result<i32>;
    /// Player varbit value (an error unless the base var is a player var).
    fn varbit(&self, id: i32) -> Result<i32>;
    /// Maximum level of a stat.
    fn stat_level_max(&self, stat: i32) -> Result<i32>;
}

pub use crate::config::ParamValue;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quest {
    pub name: Option<String>,
    pub sortname: Option<String>,
    /// Opcode 3: `[varp, startedValue, finishedValue]`.
    pub varp_progress: Option<Vec<[i32; 3]>>,
    /// Opcode 4: `[varbit, startedValue, finishedValue]`.
    pub varbit_progress: Option<Vec<[i32; 3]>>,
    pub kind: i32,
    pub difficulty: i32,
    pub members: bool,
    pub points: i32,
    /// Opcode 10: a list of ints that is decoded and kept but not consumed.
    pub extra_ints: Option<Vec<i32>>,
    pub questrequirements: Option<Vec<i32>>,
    pub statrequirements: Option<Vec<[i32; 2]>>,
    pub pointsrequirement: i32,
    pub varps_requirement: Option<Vec<i32>>,
    pub varps_min: Vec<i32>,
    pub varps_max: Vec<i32>,
    pub varps_descriptions: Vec<String>,
    pub varbits_requirement: Option<Vec<i32>>,
    pub varbits_min: Vec<i32>,
    pub varbits_max: Vec<i32>,
    pub varbits_descriptions: Vec<String>,
    /// Opcode 249; a lookup returns the first entry put for a key.
    pub params: Option<Vec<(i32, ParamValue)>>,
    /// Opcode 17: sprite shown after the quest name; `-1` for none.
    pub icon_sprite: i32,
}
/// Quest opcodes of this revision.
static QUEST_OPCODES: Table<Quest, anyhow::Error> = Table::new(
    &[
        Entry::new(
            at(1),
            Rule::Custom(|s, q, _| {
                q.name = Some(read_marked_text(s)?);
                Ok(())
            }),
        ),
        Entry::new(
            at(2),
            Rule::Custom(|s, q, _| {
                q.sortname = Some(read_marked_text(s)?);
                Ok(())
            }),
        ),
        Entry::new(span(3, 4), Rule::Custom(read_progress)),
        Entry::new(at(5), Rule::Skip(&[Field::Short])),
        Entry::new(at(6), Rule::Byte(|q, _, v| q.kind = i32::from(v))),
        Entry::new(at(7), Rule::Byte(|q, _, v| q.difficulty = i32::from(v))),
        Entry::new(at(8), Rule::Flag(|q, _| q.members = true)),
        Entry::new(at(9), Rule::Byte(|q, _, v| q.points = i32::from(v))),
        Entry::new(
            at(10),
            Rule::Custom(|s, q, _| {
                let count = s.byte()?;
                let mut values = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    values.push(s.int()?);
                }
                q.extra_ints = Some(values);
                Ok(())
            }),
        ),
        Entry::new(at(12), Rule::Skip(&[Field::Int])),
        Entry::new(
            at(13),
            Rule::Custom(|s, q, _| {
                let count = s.byte()?;
                let mut values = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    values.push(i32::from(s.short()?));
                }
                q.questrequirements = Some(values);
                Ok(())
            }),
        ),
        Entry::new(
            at(14),
            Rule::Custom(|s, q, _| {
                let count = s.byte()?;
                let mut rows = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    rows.push([i32::from(s.byte()?), i32::from(s.byte()?)]);
                }
                q.statrequirements = Some(rows);
                Ok(())
            }),
        ),
        Entry::new(
            at(15),
            Rule::Short(|q, _, v| q.pointsrequirement = i32::from(v)),
        ),
        Entry::new(at(17), Rule::SmartId(|q, _, v| q.icon_sprite = v)),
        Entry::new(span(18, 19), Rule::Custom(read_range_requirements)),
        Entry::new(
            at(249),
            Rule::Custom(|s, q, _| {
                let params = q.params.get_or_insert_with(Vec::new);
                crate::config::read_params(s, params)
            }),
        ),
    ],
    Unknown::Fail(|_, opcode| anyhow!("quest opcode {opcode} is not decoded by the quest type")),
);

/// Progress rows, `[var, started value, finished value]`: for varps (first
/// opcode) or varbits.
fn read_progress(source: Input<anyhow::Error>, q: &mut Quest, slot: Slot) -> Result<()> {
    let count = source.byte()?;
    let mut rows = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        rows.push([i32::from(source.short()?), source.int()?, source.int()?]);
    }
    if slot == 0 {
        q.varp_progress = Some(rows);
    } else {
        q.varbit_progress = Some(rows);
    }
    Ok(())
}

/// Range requirements: per entry a var id, a minimum, a maximum and a
/// description; for varps (first opcode) or varbits.
fn read_range_requirements(source: Input<anyhow::Error>, q: &mut Quest, slot: Slot) -> Result<()> {
    let count = usize::from(source.byte()?);
    let mut ids = Vec::with_capacity(count);
    let mut min = Vec::with_capacity(count);
    let mut max = Vec::with_capacity(count);
    let mut descriptions = Vec::with_capacity(count);
    for _ in 0..count {
        ids.push(source.int()?);
        min.push(source.int()?);
        max.push(source.int()?);
        descriptions.push(source.text()?);
    }
    if slot == 0 {
        q.varps_requirement = Some(ids);
        q.varps_min = min;
        q.varps_max = max;
        q.varps_descriptions = descriptions;
    } else {
        q.varbits_requirement = Some(ids);
        q.varbits_min = min;
        q.varbits_max = max;
        q.varbits_descriptions = descriptions;
    }
    Ok(())
}

/// Text preceded by a marker byte that must be 0.
fn read_marked_text(source: Input<anyhow::Error>) -> Result<String> {
    let marker = source.byte()?;
    anyhow::ensure!(marker == 0, "text marker {marker}");
    source.text()
}

impl Quest {
    fn new() -> Self {
        Self {
            icon_sprite: -1,
            ..Self::default()
        }
    }

    /// Decode a quest definition: the opcode stream, then the sort name default.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let record = Record {
            kind: "quest",
            id: -1,
        };
        let mut q = decode_record(&QUEST_OPCODES, "quest", record, bytes, Self::new())?;
        if q.sortname.is_none() {
            q.sortname = q.name.clone();
        }
        Ok(q)
    }

    fn param(&self, key: i32) -> Option<&ParamValue> {
        self.params
            .as_ref()
            .and_then(|p| p.iter().find(|(k, _)| *k == key))
            .map(|(_, v)| v)
    }
    /// An integer parameter, or `default` when absent.
    pub fn param_int(&self, key: i32, default: i32) -> Result<i32> {
        match self.param(key) {
            None => Ok(default),
            Some(ParamValue::Int(v)) => Ok(*v),
            Some(_) => {
                anyhow::bail!(Fault::WrongValueType
                    .message(format_args!("quest param {key} is not an integer")))
            }
        }
    }
    /// A string parameter, or `default` when absent.
    pub fn param_str(&self, key: i32, default: Option<String>) -> Result<Option<String>> {
        match self.param(key) {
            None => Ok(default),
            Some(ParamValue::Str(v)) => Ok(Some(v.clone())),
            Some(_) => {
                anyhow::bail!(Fault::WrongValueType
                    .message(format_args!("quest param {key} is not a string")))
            }
        }
    }
    fn progress(&self, p: &dyn Player, column: usize) -> Result<bool> {
        if let Some(rows) = &self.varp_progress {
            for row in rows {
                if p.varp(row[0])? >= row[column] {
                    return Ok(true);
                }
            }
        }
        if let Some(rows) = &self.varbit_progress {
            for row in rows {
                if p.varbit(row[0])? >= row[column] {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    /// Whether any progress row says the quest has started.
    pub fn started(&self, p: &dyn Player) -> Result<bool> {
        self.progress(p, 1)
    }
    /// Whether any progress row says the quest is finished.
    pub fn finished(&self, p: &dyn Player) -> Result<bool> {
        self.progress(p, 2)
    }
    /// Whether stat requirement `i` is met.
    pub fn stat_requirement_met(&self, p: &dyn Player, i: i32) -> Result<bool> {
        let Some(row) = self
            .statrequirements
            .as_ref()
            .and_then(|r| usize::try_from(i).ok().and_then(|i| r.get(i)))
        else {
            return Ok(false);
        };
        Ok(p.stat_level_max(row[0])? >= row[1])
    }
    /// Whether varp range requirement `i` is met.
    pub fn varps_requirement_met(&self, p: &dyn Player, i: i32) -> Result<bool> {
        let Some(&id) = self
            .varps_requirement
            .as_ref()
            .and_then(|r| usize::try_from(i).ok().and_then(|i| r.get(i)))
        else {
            return Ok(false);
        };
        let v = p.varp(id)?;
        let i = i as usize;
        Ok(v >= self.varps_min[i] && v <= self.varps_max[i])
    }
    /// Whether varbit range requirement `i` is met.
    pub fn varbits_requirement_met(&self, p: &dyn Player, i: i32) -> Result<bool> {
        let Some(&id) = self
            .varbits_requirement
            .as_ref()
            .and_then(|r| usize::try_from(i).ok().and_then(|i| r.get(i)))
        else {
            return Ok(false);
        };
        let v = p.varbit(id)?;
        let i = i as usize;
        Ok(v >= self.varbits_min[i] && v <= self.varbits_max[i])
    }
}

pub struct QuestStore {
    pub quests: BTreeMap<i32, Quest>,
    /// The preloaded quest list capacity.
    pub count: i32,
    pub params: BTreeMap<i32, native910::config::ParamConfig>,
    empty: Quest,
}
impl QuestStore {
    pub fn load(pack: &Pack) -> Result<Self> {
        // One group (35) of the shared config archive; the archive size
        // of a single-group list is the last file id + 1.
        let files = pack
            .read_group("config", 35)
            .context("quest config group 35")?;
        let count = files.keys().next_back().map_or(0, |&id| id as i32 + 1);
        let mut quests = BTreeMap::new();
        for (id, bytes) in files {
            quests.insert(
                id as i32,
                Quest::decode(&bytes).with_context(|| format!("quest {id}"))?,
            );
        }
        let mut params = BTreeMap::new();
        for (id, bytes) in pack
            .read_group("config", 11)
            .context("param config group 11")?
        {
            params.insert(
                id as i32,
                native910::config::decode_param(&bytes)
                    .map_err(|e| anyhow!("param {id}: {e:?}"))?,
            );
        }
        Ok(Self {
            quests,
            count,
            params,
            empty: Quest::new(),
        })
    }
    /// Absent files decode to a default type,
    /// ids at or past the capacity fail.
    pub fn quest(&self, id: i32) -> Result<&Quest> {
        anyhow::ensure!(
            id < self.count,
            "{}",
            Fault::IndexOutOfRange.message(format_args!("quest {id} of {}", self.count))
        );
        Ok(self.quests.get(&id).unwrap_or(&self.empty))
    }
    /// Total quest points over every finished quest.
    pub fn points(&self, p: &dyn Player) -> Result<i32> {
        let mut sum = 0;
        for q in self.quests.values() {
            if q.finished(p)? {
                sum += q.points;
            }
        }
        Ok(sum)
    }
    /// Whether every requirement of the quest is met.
    pub fn all_requirements_met(&self, q: &Quest, p: &dyn Player) -> Result<bool> {
        if self.points(p)? < q.pointsrequirement {
            return Ok(false);
        }
        if let Some(rows) = &q.statrequirements {
            for row in rows {
                if p.stat_level_max(row[0])? < row[1] {
                    return Ok(false);
                }
            }
        }
        if let Some(ids) = &q.questrequirements {
            for &id in ids {
                if !self.quest(id)?.finished(p)? {
                    return Ok(false);
                }
            }
        }
        if let Some(ids) = &q.varps_requirement {
            for (i, &id) in ids.iter().enumerate() {
                let v = p.varp(id)?;
                if v < q.varps_min[i] || v > q.varps_max[i] {
                    return Ok(false);
                }
            }
        }
        if let Some(ids) = &q.varbits_requirement {
            for (i, &id) in ids.iter().enumerate() {
                let v = p.varbit(id)?;
                if v < q.varbits_min[i] || v > q.varbits_max[i] {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    /// Run a quest script command; `None` when `command` is not a quest command.
    pub fn dispatch(
        &self,
        p: &dyn Player,
        command: &str,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
    ) -> Option<VmResult<Option<Value>>> {
        let arity = match command {
            "quest_getname"
            | "quest_getsortname"
            | "quest_type"
            | "quest_getdifficulty"
            | "quest_getmembers"
            | "quest_points"
            | "quest_questreq_count"
            | "quest_pointsreq"
            | "quest_pointsreq_met"
            | "quest_statreq_count"
            | "quest_varpreq_count"
            | "quest_varbitreq_count"
            | "quest_allreqmet"
            | "quest_started"
            | "quest_finished" => 1,
            "quest_questreq"
            | "quest_questreq_met"
            | "quest_statreq_stat"
            | "quest_statreq_level"
            | "quest_statreq_met"
            | "quest_varpreq_desc"
            | "quest_varpreq_met"
            | "quest_varbitreq_desc"
            | "quest_varbitreq_met"
            | "quest_param" => 2,
            _ => return None,
        };
        if ints.len() < arity {
            return Some(Err(VmError::StackUnderflow { stack: "int" }));
        }
        let args = ints.split_off(ints.len() - arity);
        Some(
            self.run(p, command, &args, objs)
                .map_err(|e| VmError::TrapFailed {
                    command: command.into(),
                    reason: format!("{e:#}"),
                }),
        )
    }
    fn run(
        &self,
        p: &dyn Player,
        command: &str,
        a: &[i32],
        objs: &mut Vec<String>,
    ) -> Result<Option<Value>> {
        let string = |v: Option<String>| -> Result<Option<Value>> {
            // Quest type fields and the param default string may be null on the
            // original's object stack. The native VM string lane has no null object
            // variant; the string join renders that object as the literal `null`
            // when a script joins it, so preserve that observable value here.
            Ok(Some(Value::Str(v.unwrap_or_else(|| "null".into()))))
        };
        let boolean = |v: bool| Ok(Some(Value::Int(i32::from(v))));
        let index = |v: &Option<Vec<i32>>, i: i32| -> Result<i32> {
            v.as_ref()
                .with_context(|| Fault::MissingValue.message("quest requirement array"))?
                .get(usize::try_from(i).map_err(|_| anyhow!(Fault::IndexOutOfRange.message(i)))?)
                .copied()
                .with_context(|| Fault::IndexOutOfRange.message(i))
        };
        if command == "quest_param" {
            let (quest, key) = (a[0], a[1]);
            let param = self
                .params
                .get(&key)
                .with_context(|| format!("param {key} absent"))?;
            let is_string = param.kind == Some(36) || param.kind_legacy == Some(b's');
            if !is_string {
                let default = param.default_int.unwrap_or(0);
                return Ok(Some(Value::Int(if quest == -1 {
                    default
                } else {
                    self.quest(quest)?.param_int(key, default)?
                })));
            }
            let default = param.default_string.clone();
            return string(if quest == -1 {
                default
            } else {
                self.quest(quest)?.param_str(key, default)?
            });
        }
        let q = self.quest(a[0])?;
        match command {
            "quest_getname" => string(q.name.clone()),
            "quest_getsortname" => string(q.sortname.clone()),
            "quest_type" => Ok(Some(Value::Int(q.kind))),
            "quest_getdifficulty" => Ok(Some(Value::Int(q.difficulty))),
            "quest_getmembers" => boolean(q.members),
            "quest_points" => Ok(Some(Value::Int(q.points))),
            "quest_questreq_count" => Ok(Some(Value::Int(
                q.questrequirements.as_ref().map_or(0, |v| v.len() as i32),
            ))),
            "quest_questreq" => Ok(Some(Value::Int(index(&q.questrequirements, a[1])?))),
            "quest_questreq_met" => {
                let Some(ids) = &q.questrequirements else {
                    return boolean(false);
                };
                let Some(&id) = usize::try_from(a[1]).ok().and_then(|i| ids.get(i)) else {
                    return boolean(false);
                };
                boolean(self.quest(id)?.finished(p)?)
            }
            "quest_pointsreq" => Ok(Some(Value::Int(q.pointsrequirement))),
            "quest_pointsreq_met" => boolean(self.points(p)? >= q.pointsrequirement),
            "quest_statreq_count" => Ok(Some(Value::Int(
                q.statrequirements.as_ref().map_or(0, |v| v.len() as i32),
            ))),
            "quest_statreq_stat" | "quest_statreq_level" => {
                let rows = q
                    .statrequirements
                    .as_ref()
                    .with_context(|| Fault::MissingValue.message("stat requirements"))?;
                let row = usize::try_from(a[1])
                    .ok()
                    .and_then(|i| rows.get(i))
                    .with_context(|| Fault::IndexOutOfRange.message(a[1]))?;
                Ok(Some(Value::Int(if command == "quest_statreq_stat" {
                    row[0]
                } else {
                    row[1]
                })))
            }
            "quest_statreq_met" => boolean(q.stat_requirement_met(p, a[1])?),
            "quest_varpreq_count" => Ok(Some(Value::Int(
                q.varps_requirement.as_ref().map_or(0, |v| v.len() as i32),
            ))),
            "quest_varpreq_desc" => {
                let i = usize::try_from(a[1])
                    .map_err(|_| anyhow!(Fault::IndexOutOfRange.message(a[1])))?;
                let s = q
                    .varps_descriptions
                    .get(i)
                    .cloned()
                    .context("varpsDescriptions index")?;
                objs.push(s);
                Ok(None)
            }
            "quest_varpreq_met" => boolean(q.varps_requirement_met(p, a[1])?),
            "quest_varbitreq_count" => Ok(Some(Value::Int(
                q.varbits_requirement.as_ref().map_or(0, |v| v.len() as i32),
            ))),
            "quest_varbitreq_desc" => {
                let i = usize::try_from(a[1])
                    .map_err(|_| anyhow!(Fault::IndexOutOfRange.message(a[1])))?;
                let s = q
                    .varbits_descriptions
                    .get(i)
                    .cloned()
                    .context("varbitsDescriptions index")?;
                objs.push(s);
                Ok(None)
            }
            "quest_varbitreq_met" => boolean(q.varbits_requirement_met(p, a[1])?),
            "quest_allreqmet" => boolean(self.all_requirements_met(q, p)?),
            "quest_started" => boolean(q.started(p)?),
            "quest_finished" => boolean(q.finished(p)?),
            _ => unreachable!("arity table"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(BTreeMap<i32, i32>);
    impl Player for Fixture {
        fn varp(&self, id: i32) -> Result<i32> {
            Ok(*self.0.get(&id).unwrap_or(&0))
        }
        fn varbit(&self, _: i32) -> Result<i32> {
            anyhow::bail!("no varbits")
        }
        fn stat_level_max(&self, _: i32) -> Result<i32> {
            Ok(1)
        }
    }

    #[test]
    fn progress_rows_decide_started_and_finished() {
        // name "Q", varp progress [7 -> started 1, finished 3], points 2.
        let bytes = [1, 0, b'Q', 0, 3, 1, 0, 7, 0, 0, 0, 1, 0, 0, 0, 3, 9, 2, 0];
        let q = Quest::decode(&bytes).unwrap();
        assert_eq!(q.sortname.as_deref(), Some("Q"));
        let mut store = QuestStore {
            quests: BTreeMap::new(),
            count: 1,
            params: BTreeMap::new(),
            empty: Quest::new(),
        };
        store.quests.insert(0, q);
        let p = Fixture([(7, 1)].into_iter().collect());
        let mut ints = vec![0];
        assert_eq!(
            store
                .dispatch(&p, "quest_started", &mut ints, &mut vec![])
                .unwrap()
                .unwrap(),
            Some(Value::Int(1))
        );
        let mut ints = vec![0];
        assert_eq!(
            store
                .dispatch(&p, "quest_finished", &mut ints, &mut vec![])
                .unwrap()
                .unwrap(),
            Some(Value::Int(0))
        );
        let p = Fixture([(7, 3)].into_iter().collect());
        let mut ints = vec![0];
        assert_eq!(
            store
                .dispatch(&p, "quest_finished", &mut ints, &mut vec![])
                .unwrap()
                .unwrap(),
            Some(Value::Int(1))
        );
        assert_eq!(store.points(&p).unwrap(), 2);
        let mut ints = vec![1];
        assert!(store
            .dispatch(&p, "quest_finished", &mut ints, &mut vec![])
            .unwrap()
            .is_err());
    }

    #[test]
    fn null_string_param_reaches_the_vm_as_null_text() {
        let store = QuestStore {
            quests: BTreeMap::new(),
            count: 1,
            params: BTreeMap::from([(
                7,
                native910::config::ParamConfig {
                    kind: Some(36),
                    kind_legacy: None,
                    default_int: None,
                    default_string: None,
                    autodisable: true,
                },
            )]),
            empty: Quest::new(),
        };
        let player = Fixture(BTreeMap::new());
        let mut ints = vec![0, 7];
        let value = store
            .dispatch(&player, "quest_param", &mut ints, &mut vec![])
            .expect("quest_param owner")
            .expect("null default is a valid result")
            .expect("quest_param pushes a string");
        assert_eq!(value, Value::Str("null".into()));
        assert!(ints.is_empty());
    }
}
