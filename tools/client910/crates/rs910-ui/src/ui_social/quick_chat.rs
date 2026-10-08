//! Quick-chat phrase and category decoding, searches and dynamic values.

#[derive(Clone, Debug, Default)]
pub(super) struct QuickChatPhrase {
    text: Vec<String>,
    dynamics: Vec<QuickChatDynamic>,
    auto_responses: Vec<u16>,
    searchable: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct QuickChatDynamic {
    command: u16,
    /// Bytes consumed on receive.
    width: u8,
    /// Bytes emitted when transmitting the dynamic values (see
    /// `QuickChatStore::transmit_values`).
    transmit_width: u8,
    parameters: Vec<i32>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct QuickChatCategory {
    description: String,
    sub_categories: Vec<(u16, char)>,
    phrases: Vec<(u16, char)>,
}

#[derive(Clone, Debug, Default)]
pub struct QuickChatStore {
    pub(super) phrases: std::collections::BTreeMap<u16, QuickChatPhrase>,
    pub(super) categories: std::collections::BTreeMap<u16, QuickChatCategory>,
}

impl QuickChatStore {
    pub fn load(pack: &crate::cache::Pack) -> Self {
        let mut store = Self::default();
        for (archive, global) in [("quickchat", false), ("global.quickchat", true)] {
            if let Ok(files) = pack.read_group(archive, 0) {
                for (file_id, bytes) in files {
                    if let Some(category) = Self::decode_category(&bytes) {
                        let id = if global { file_id | 0x8000 } else { file_id };
                        let mut category = category;
                        if global {
                            for (entry, _) in &mut category.sub_categories {
                                *entry |= 0x8000;
                            }
                            for (entry, _) in &mut category.phrases {
                                *entry |= 0x8000;
                            }
                        }
                        store.categories.insert(id as u16, category);
                    }
                }
            }
            let Ok(files) = pack.read_group(archive, 1) else {
                continue;
            };
            for (file_id, bytes) in files {
                if let Some(phrase) = Self::decode_phrase(&bytes) {
                    let id = if global { file_id | 0x8000 } else { file_id };
                    let mut phrase = phrase;
                    if global {
                        for response in &mut phrase.auto_responses {
                            *response |= 0x8000;
                        }
                    }
                    store.phrases.insert(id as u16, phrase);
                }
            }
        }
        store
    }

    pub(super) fn decode_phrase(bytes: &[u8]) -> Option<QuickChatPhrase> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let mut phrase = QuickChatPhrase {
            searchable: true,
            ..Default::default()
        };
        loop {
            let opcode = reader.g1().ok()?;
            match opcode {
                0 => break,
                1 => phrase.text = reader.gjstr().ok()?.split('<').map(str::to_owned).collect(),
                2 => {
                    let count = reader.g1().ok()?;
                    for _ in 0..count {
                        phrase.auto_responses.push(reader.g2().ok()?);
                    }
                }
                3 => {
                    let count = reader.g1().ok()?;
                    for _ in 0..count {
                        let command = reader.g2().ok()?;
                        let (width, transmit_width, parameter_count) = match command {
                            // (receive width, transmit width, parameter count) per command.
                            0 => (2, 2, 1),  // LISTDIALOG
                            1 => (2, 2, 0),  // OBJDIALOG
                            2 => (4, 4, 0),  // COUNTDIALOG
                            4 => (1, 1, 1),  // STAT_BASE
                            6 => (4, 0, 2),  // ENUM_STRING
                            7 => (1, 0, 1),  // ENUM_STRING_CLAN
                            8 => (4, 0, 1),  // TOSTRING_VARP
                            9 => (4, 0, 1),  // TOSTRING_VARBIT
                            10 => (2, 2, 0), // OBJTRADEDIALOG
                            11 => (1, 0, 2), // ENUM_STRING_STATBASE
                            12 => (1, 0, 0), // ACC_GETCOUNT_WORLD
                            13 => (1, 0, 0), // ACC_GETMEANCOMBATLEVEL
                            14 => (4, 0, 1), // TOSTRING_SHARED
                            15 => (1, 0, 0), // ACTIVECOMBATLEVEL
                            16 => (4, 0, 2), // ENUM_STRING_VARBIT
                            _ => return None,
                        };
                        let mut parameters = Vec::with_capacity(parameter_count);
                        for _ in 0..parameter_count {
                            parameters.push(i32::from(reader.g2().ok()?));
                        }
                        phrase.dynamics.push(QuickChatDynamic {
                            command,
                            width,
                            transmit_width,
                            parameters,
                        });
                    }
                }
                4 => phrase.searchable = false,
                _ => return None,
            }
        }
        if phrase.text.is_empty() {
            phrase.text.push(String::new());
        }
        while phrase.text.len() < phrase.dynamics.len() + 1 {
            phrase.text.push(String::new());
        }
        Some(phrase)
    }

    pub(super) fn decode_category(bytes: &[u8]) -> Option<QuickChatCategory> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let mut category = QuickChatCategory::default();
        loop {
            match reader.g1().ok()? {
                0 => break,
                1 => category.description = reader.gjstr().ok()?,
                2 => {
                    let count = reader.g1().ok()?;
                    for _ in 0..count {
                        let id = reader.g2().ok()?;
                        let shortcut = reader.g1().ok()?;
                        category.sub_categories.push((
                            id,
                            if shortcut == 0 {
                                '\0'
                            } else {
                                crate::ui_dialogue::cp1252_decode_byte(shortcut)
                            },
                        ));
                    }
                }
                3 => {
                    let count = reader.g1().ok()?;
                    for _ in 0..count {
                        let id = reader.g2().ok()?;
                        let shortcut = reader.g1().ok()?;
                        category.phrases.push((
                            id,
                            if shortcut == 0 {
                                '\0'
                            } else {
                                crate::ui_dialogue::cp1252_decode_byte(shortcut)
                            },
                        ));
                    }
                }
                4 => {}
                _ => return None,
            }
        }
        Some(category)
    }

    pub fn render(
        &self,
        id: u16,
        bytes: &[u8],
        pos: &mut usize,
        configs: &crate::ui_configs::Configs,
        objs: Option<&crate::config::ObjStore>,
    ) -> String {
        let Some(phrase) = self.phrases.get(&id) else {
            return format!("[quickchat:{id}]");
        };
        let mut out = String::with_capacity(80);
        for (index, dynamic) in phrase.dynamics.iter().enumerate() {
            out.push_str(phrase.text.get(index).map_or("", String::as_str));
            let value = read_var_long(bytes, pos, dynamic.width).unwrap_or(0);
            out.push_str(&format_dynamic(dynamic, value, configs, objs));
        }
        out.push_str(phrase.text.last().map_or("", String::as_str));
        out
    }

    pub fn dynamic_count(&self, id: u16) -> Option<usize> {
        self.phrases.get(&id).map(|phrase| phrase.dynamics.len())
    }

    pub fn category_description(&self, id: u16) -> Option<&str> {
        self.categories
            .get(&id)
            .map(|category| category.description.as_str())
    }

    pub fn category_sub_count(&self, id: u16) -> usize {
        self.categories
            .get(&id)
            .map_or(0, |category| category.sub_categories.len())
    }

    pub fn category_sub(&self, id: u16, index: usize) -> Option<u16> {
        self.categories
            .get(&id)?
            .sub_categories
            .get(index)
            .map(|entry| entry.0)
    }

    pub fn category_phrase_count(&self, id: u16) -> usize {
        self.categories
            .get(&id)
            .map_or(0, |category| category.phrases.len())
    }

    pub fn category_phrase(&self, id: u16, index: usize) -> Option<u16> {
        self.categories
            .get(&id)?
            .phrases
            .get(index)
            .map(|entry| entry.0)
    }

    pub fn category_sub_shortcut(&self, id: u16, index: usize) -> Option<char> {
        self.categories
            .get(&id)?
            .sub_categories
            .get(index)
            .map(|entry| entry.1)
    }

    pub fn category_phrase_shortcut(&self, id: u16, index: usize) -> Option<char> {
        self.categories
            .get(&id)?
            .phrases
            .get(index)
            .map(|entry| entry.1)
    }

    pub fn category_find_sub_by_shortcut(&self, id: u16, shortcut: char) -> i32 {
        self.categories
            .get(&id)
            .and_then(|category| {
                category
                    .sub_categories
                    .iter()
                    .find(|entry| entry.1 == shortcut)
                    .map(|entry| i32::from(entry.0))
            })
            .unwrap_or(-1)
    }

    pub fn category_find_phrase_by_shortcut(&self, id: u16, shortcut: char) -> i32 {
        self.categories
            .get(&id)
            .and_then(|category| {
                category
                    .phrases
                    .iter()
                    .find(|entry| entry.1 == shortcut)
                    .map(|entry| i32::from(entry.0))
            })
            .unwrap_or(-1)
    }

    pub fn auto_response_count(&self, id: u16) -> usize {
        self.phrases
            .get(&id)
            .map_or(0, |phrase| phrase.auto_responses.len())
    }

    pub fn auto_response(&self, id: u16, index: usize) -> Option<u16> {
        self.phrases.get(&id)?.auto_responses.get(index).copied()
    }

    pub fn dynamic_command(&self, id: u16, index: usize) -> Option<u16> {
        self.phrases
            .get(&id)?
            .dynamics
            .get(index)
            .map(|dynamic| dynamic.command)
    }

    /// Replaces each dynamic slot
    /// with the three-dot placeholder used by phrase-selection interfaces.
    pub fn text_display(&self, id: u16) -> Option<String> {
        let phrase = self.phrases.get(&id)?;
        let mut out = phrase.text.first().cloned().unwrap_or_default();
        for text in phrase.text.iter().skip(1) {
            out.push_str("...");
            out.push_str(text);
        }
        Some(out)
    }

    /// Searches one local/global phrase archive,
    /// cap the result set at fifty, then order ids by their display text.
    pub fn find_phrases(&self, query: &str, global: bool) -> Option<Vec<u16>> {
        let query = query.to_lowercase();
        let lower: u32 = if global { 0x8000 } else { 0 };
        let upper = lower + 0x8000;
        let mut matches = Vec::new();
        for (&id, phrase) in &self.phrases {
            if u32::from(id) < lower || u32::from(id) >= upper || !phrase.searchable {
                continue;
            }
            let display = self.text_display(id).unwrap_or_default();
            if display.to_lowercase().contains(&query) {
                if matches.len() >= 50 {
                    return None;
                }
                matches.push((display.to_lowercase(), id));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        Some(matches.into_iter().map(|(_, id)| id).collect())
    }

    /// Returns a phrase dynamic's encoded parameter after the caller has checked its command kind.
    pub fn dynamic_parameter(
        &self,
        id: u16,
        dynamic: usize,
        parameter: usize,
    ) -> Option<(u16, i32)> {
        let dynamic = self.phrases.get(&id)?.dynamics.get(dynamic)?;
        dynamic
            .parameters
            .get(parameter)
            .copied()
            .map(|value| (dynamic.command, value))
    }

    /// Values are written as whole big-endian byte fields using each
    /// dynamic's transmit width; receive widths can be
    /// larger for enum/object-backed dynamic text.
    pub fn transmit_values(&self, id: u16, values: &[i32]) -> Option<Vec<u8>> {
        let phrase = self.phrases.get(&id)?;
        let mut out = Vec::new();
        for (index, dynamic) in phrase.dynamics.iter().enumerate() {
            let width = usize::from(dynamic.transmit_width);
            if width == 0 {
                continue;
            }
            let value = values.get(index).copied().unwrap_or(0) as u32;
            for shift in (0..width).rev() {
                out.push((value >> (shift * 8)) as u8);
            }
        }
        Some(out)
    }
}

pub(super) fn read_var_long(bytes: &[u8], pos: &mut usize, width: u8) -> Option<u64> {
    if !(1..=8).contains(&width) {
        return None;
    }
    let width = usize::from(width);
    let end = pos.checked_add(width)?;
    if end > bytes.len() {
        return None;
    }
    let mut value = 0_u64;
    for byte in &bytes[*pos..end] {
        value = (value << 8) | u64::from(*byte);
    }
    *pos = end;
    Some(value)
}

pub(super) fn format_dynamic(
    dynamic: &QuickChatDynamic,
    value: u64,
    configs: &crate::ui_configs::Configs,
    objs: Option<&crate::config::ObjStore>,
) -> String {
    let value_i32 = value as i32;
    match dynamic.command {
        // Enum-backed dynamic text.
        0 | 6 | 7 | 11 | 16 => configs
            .enumeration(dynamic.parameters.first().copied().unwrap_or(-1))
            .string(value_i32)
            .map(String::from_utf16_lossy)
            .unwrap_or_else(|_| value.to_string()),
        // Object-backed dynamic text.
        1 | 10 => objs
            .and_then(|store| store.get(value as u32))
            .map(|obj| obj.name.clone())
            .unwrap_or_else(|| "null".into()),
        _ => value.to_string(),
    }
}
