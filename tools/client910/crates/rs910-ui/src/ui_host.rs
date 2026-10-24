//! Installed engine queries. Extend from the original command behaviour as the
//! component host replaces the remaining explicitly marked preview dispatcher.
use crate::ui_text_compare::Language;
use native910::vm::{Value, VmError, VmResult};
use rs910_core::fault::Fault;

pub struct Queries {
    pub preferences: crate::ui_preferences::Preferences,
    pub client_type: i32,
    /// Current launch language; the existing login wire advertises EN (0).
    pub language: Option<Language>,
    ///  fresh client values, owned independently of varps.
    pub run_energy: i32,
    pub run_weight: i32,
    pub minimenu_option_count: i32,
    pub minimenu_submenu_count: i32,
}
impl Default for Queries {
    fn default() -> Self {
        // defaultValue; ClientOptions.
        // The live login packet currently advertises clientType & 1 == 0.
        Self {
            preferences: Default::default(),
            client_type: 0,
            language: Some(Language::En),
            run_energy: 0,
            run_weight: 0,
            minimenu_option_count: 0,
            minimenu_submenu_count: 0,
        }
    }
}
impl Queries {
    pub fn dispatch(
        &mut self,
        command: &str,
        cycle: i32,
        ints: &mut Vec<i32>,
        strings: &mut Vec<String>,
        profiler: Option<&mut crate::ui_preferences::Profiler<'_>>,
    ) -> Option<VmResult<Option<Value>>> {
        if let Some(result) = self.preferences.dispatch_profiling(command, ints, profiler) {
            return Some(result);
        }
        // Formatted in GMT with the language id, suffixed " UTC".
        if command == "format_datetime_from_minutes" {
            let Some(minutes) = ints.pop() else {
                return Some(Err(VmError::StackUnderflow { stack: "int" }));
            };
            let Some(language) = self.language else {
                return Some(Err(VmError::TrapFailed {
                    command: command.into(),
                    reason: Fault::MissingValue.message("language"),
                }));
            };
            return Some(
                crate::ui_time::format_datetime_utc(i64::from(minutes) * 60_000, language as i32)
                    .map(|s| Some(Value::Str(format!("{s} UTC"))))
                    .ok_or_else(|| VmError::TrapFailed {
                        command: command.into(),
                        reason: Fault::IndexOutOfRange.message("month name table"),
                    }),
            );
        }
        if command == "compare" {
            return Some((|| {
                if strings.len() < 2 {
                    return Err(VmError::StackUnderflow { stack: "object" });
                }
                let b = strings.pop().unwrap();
                let a = strings.pop().unwrap();
                Ok(Some(Value::Int(crate::ui_text_compare::compare(
                    &native910::jstr::units(&a),
                    &native910::jstr::units(&b),
                    self.language,
                ))))
            })());
        }
        if command == "map_lang" {
            return Some(
                self.language
                    .map(|l| Some(Value::Int(l as i32)))
                    .ok_or_else(|| VmError::TrapFailed {
                        command: command.into(),
                        reason: Fault::MissingValue.message("language"),
                    }),
            );
        }
        if command == "get_minimenu_length" {
            ints.extend([self.minimenu_option_count, self.minimenu_submenu_count]);
            return Some(Ok(None));
        }
        let value = match command {
            // The cycle is supplied by the
            // logic owner at the hook boundary, never advanced by a VM query.
            "clientclock" => Value::Int(cycle),
            "runenergy_visible" => Value::Int(self.run_energy),
            "runweight_visible" => Value::Int(self.run_weight),
            // This client is not the NXT client.
            "has_nxt" => Value::Int(0),
            "clienttype" => Value::Int(self.client_type & 1),
            _ => return None,
        };
        Some(Ok(Some(value)))
    }
}
