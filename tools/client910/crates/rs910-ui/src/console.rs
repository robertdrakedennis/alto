//! Developer console: editing/history, timed paste, redaction and drawing use
//! UTF-16 indices (as the 910 client does). Runtime effects go through Host so the
//! console cannot mutate player coordinates or own a second network stream.
use crate::font_metrics::Metrics;
pub type Text = Vec<u16>;
pub fn text(s: &str) -> Text {
    s.encode_utf16().collect()
}
#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub code: i32,
    pub ch: u16,
    pub modifiers: i32,
}
pub trait Host {
    fn now(&mut self) -> i64;
    fn command(&mut self, console: &mut Console, command: Text, suggest: bool);
    fn paste(&mut self) -> Option<Text> {
        None
    }
    fn copy(&mut self, _text: Text) {}
    fn log(&mut self, _text: Text) {}
}
#[derive(Clone, Debug)]
pub struct Console {
    pub open: bool,
    pub opacity: i32,
    pub entry: Text,
    pub cursor: usize,
    pub history: usize,
    pub lines: Option<Vec<Text>>,
    pub count: usize,
    pub scroll: usize,
    pub row_height: i32,
    pub entry_height: i32,
    pub script: Option<Vec<Text>>,
    pub script_next: i32,
    pub resume_at: i64,
}
impl Default for Console {
    fn default() -> Self {
        Self {
            open: false,
            opacity: 0,
            entry: vec![],
            cursor: 0,
            history: 0,
            lines: None,
            count: 0,
            scroll: 0,
            row_height: 0,
            entry_height: 0,
            script: None,
            script_next: -1,
            resume_at: 0,
        }
    }
}
impl Console {
    pub fn init(&mut self, host: &mut impl Host, sizes: [i32; 2]) {
        if self.lines.is_some() {
            return;
        }
        self.row_height = sizes[0];
        self.entry_height = sizes[1];
        self.lines = Some(vec![vec![]; 500]);
        self.add(host, &text(rs910_core::texts::Msg::DebugConsoleInfo.get()));
    }
    pub fn toggle(&mut self, host: &mut impl Host, sizes: [i32; 2]) {
        if self.open {
            self.open = false;
        } else {
            self.init(host, sizes);
            self.open = true;
            self.opacity = 0;
        }
    }
    pub fn set_entry(&mut self, entry: Text) {
        self.cursor = entry.len();
        self.entry = entry;
    }
    pub fn add(&mut self, host: &mut impl Host, message: &[u16]) {
        assert!(
            self.lines.is_some(),
            "console font sizes must be initialized"
        );
        // The log timestamp is explicitly GMT.
        let seconds = host.now().div_euclid(1000).rem_euclid(86400);
        let prefix = text(&format!(
            "{:02}:{:02}:{:02}: ",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        ));
        for line in message.split(|c| *c == 10) {
            let rows = self.lines.as_mut().unwrap();
            for i in (1..=self.count).rev() {
                rows[i] = rows[i - 1].clone();
            }
            rows[0] = [prefix.as_slice(), line].concat();
            host.log([rows[0].as_slice(), &[10]].concat());
            if self.count < 499 {
                self.count += 1;
                if self.scroll > 0 {
                    self.scroll += 1;
                }
            }
        }
    }
    pub fn submit(&mut self, host: &mut impl Host, suggest: bool) {
        let start = self
            .entry
            .iter()
            .position(|c| *c > 32)
            .unwrap_or(self.entry.len());
        let end = self
            .entry
            .iter()
            .rposition(|c| *c > 32)
            .map_or(start, |n| n + 1);
        self.entry = self.entry[start..end].to_vec();
        self.history = 0;
        if self.entry.is_empty() {
            self.cursor = 0;
            return;
        }
        self.add(host, &[text("--> "), self.entry.clone()].concat());
        host.command(self, self.entry.clone(), suggest);
        if suggest {
            self.cursor = self.entry.len();
        } else {
            self.cursor = 0;
            self.entry.clear();
        }
    }
    fn recall(&mut self) {
        if self.history == 0 {
            self.entry.clear();
            return;
        }
        let mut count = 0;
        for row in self.lines.as_ref().unwrap() {
            if contains(row, &text("--> ")).is_some() {
                count += 1;
                if self.history == count {
                    let p = row.iter().position(|c| *c == 62).unwrap();
                    self.entry = row[p + 2..].to_vec();
                    break;
                }
            }
        }
    }
    fn pause(&mut self, host: &mut impl Host, line: &[u16], next: usize) -> bool {
        if !line.starts_with(&text("pause")) {
            return false;
        }
        let seconds = line
            .get(6..)
            .and_then(|s| String::from_utf16(s).ok())
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(5);
        self.add(host, &text(&format!("Pausing for {seconds} seconds...")));
        self.script_next = next as i32;
        self.resume_at = host.now().wrapping_add(seconds.wrapping_mul(1000) as i64);
        true
    }
    pub fn paste_lines(&mut self, host: &mut impl Host, lines: Vec<Text>) {
        if lines.len() <= 1 {
            let line = lines.first().expect("paste requires a line");
            self.entry.extend(line);
            self.cursor += line.len();
            return;
        }
        for (i, line) in lines.iter().enumerate() {
            if self.pause(host, line, i + 1) {
                self.script = Some(lines);
                return;
            }
            self.entry = line.clone();
            self.submit(host, false);
        }
    }
    pub fn tick(&mut self, host: &mut impl Host, keys: &[Key], wheel: i32) -> bool {
        if self.opacity < 102 {
            self.opacity += 6;
        }
        if self.script_next != -1 && self.resume_at < host.now() {
            let lines = self.script.clone().unwrap();
            for (i, line) in lines.iter().enumerate().skip(self.script_next as usize) {
                if self.pause(host, line, i + 1) {
                    return false;
                }
                self.entry = line.clone();
                self.submit(host, false);
            }
            self.script_next = -1;
        }
        if wheel != 0 {
            let s = (self.scroll as i32).wrapping_sub(wheel.wrapping_mul(5));
            self.scroll = s.min(self.count as i32 - 1).max(0) as usize;
        }
        for key in keys {
            if key.code == 84 {
                self.submit(host, false);
            }
            if key.code == 80 {
                self.submit(host, true);
            } else if key.code == 66 && key.modifiers & 4 != 0 {
                let mut all = Vec::new();
                for row in self.lines.as_ref().unwrap().iter().rev() {
                    if !row.is_empty() {
                        all.extend(row);
                        all.push(10);
                    }
                }
                host.copy(all);
            } else if key.code == 67 && key.modifiers & 4 != 0 {
                if let Some(p) = host.paste() {
                    self.paste_lines(host, p.split(|c| *c == 10).map(|s| s.to_vec()).collect());
                }
            } else if key.code == 85 && self.cursor > 0 {
                self.entry.remove(self.cursor - 1);
                self.cursor -= 1;
            } else if key.code == 101 && self.cursor < self.entry.len() {
                self.entry.remove(self.cursor);
            } else if key.code == 96 && self.cursor > 0 {
                self.cursor -= 1;
            } else if key.code == 97 && self.cursor < self.entry.len() {
                self.cursor += 1;
            } else if key.code == 102 {
                self.cursor = 0;
            } else if key.code == 103 {
                self.cursor = self.entry.len();
            } else if key.code == 104 && self.history < 500 {
                self.history += 1;
                self.recall();
                self.cursor = self.entry.len();
            } else if key.code == 105 && self.history > 0 {
                self.history -= 1;
                self.recall();
                self.cursor = self.entry.len();
            } else if matches!(key.ch,48..=57|65..=90|97..=122)
                || text("\\/.:, _-+[]~@").contains(&key.ch)
            {
                self.entry.insert(self.cursor, key.ch);
                self.cursor += 1;
            }
        }
        true
    }
}
fn contains(s: &[u16], p: &[u16]) -> Option<usize> {
    s.windows(p.len()).position(|v| v == p)
}
pub fn redact(entry: &[u16]) -> Text {
    let split = contains(entry, &text("--> ")).map_or(0, |n| n + 4);
    let tail = &entry[split..];
    if tail.starts_with(&text("directlogin ")) {
        if let Some(i) = tail[12..].iter().position(|c| *c == 32) {
            let end = split + 12 + i;
            let mut result = entry[..end + 1].to_vec();
            result.resize(entry.len(), 42);
            return result;
        }
    }
    entry.to_vec()
}
pub use crate::console_draw::*;
impl Console {
    /// Version text inherits the final log-column
    /// clip; input and caret then restore the full console rectangle.
    pub fn draw(
        &self,
        width: i32,
        cycle: i32,
        focused: bool,
        fonts: [&Metrics; 3],
    ) -> anyhow::Result<Vec<Draw>> {
        let [p11, p12, b12] = fonts;
        let _ = p11;
        let mut out = vec![
            Draw::Clip([0, 0, width, 350]),
            Draw::Fill {
                x: 0,
                y: 0,
                width,
                height: 350,
                colour: self.opacity << 24 | 0x332277,
                blend: 1,
            },
        ];
        anyhow::ensure!(self.row_height != 0, "console font line height");
        let visible = 350 / self.row_height;
        if self.count > 0 {
            let track = 346 - self.row_height - 4;
            let thumb = visible * track / (self.count as i32 + visible - 1);
            let y = if self.count > 1 {
                4 + (self.count as i32 - 1 - self.scroll as i32) * (track - thumb)
                    / (self.count as i32 - 1)
            } else {
                4
            };
            out.push(Draw::Fill {
                x: width - 16,
                y,
                width: 12,
                height: thumb,
                colour: self.opacity << 24 | 0x332277,
                blend: 2,
            });
            for i in self.scroll..(self.scroll + visible as usize).min(self.count) {
                let pieces: Vec<_> = self.lines.as_ref().unwrap()[i].split(|c| *c == 8).collect();
                let col = (width - 24) / pieces.len() as i32;
                for (n, piece) in pieces.into_iter().enumerate() {
                    let x = col * n as i32 + 8;
                    out.push(Draw::Clip([x, 0, col + x - 8, 350]));
                    out.push(Draw::Text {
                        font: Font::P12,
                        text: redact(piece),
                        x,
                        y: 350
                            - self.entry_height
                            - 2
                            - p12.descent
                            - self.row_height * (i - self.scroll) as i32,
                        right: false,
                        colour: -1,
                        shadow: 0xff000000u32 as i32,
                    });
                }
            }
        }
        out.push(Draw::Text {
            font: Font::P11,
            text: text("910 1"),
            x: width - 25,
            y: 330,
            right: true,
            colour: -1,
            shadow: 0xff000000u32 as i32,
        });
        out.push(Draw::Clip([0, 0, width, 350]));
        out.push(Draw::Line {
            x: 0,
            y: 350 - self.entry_height,
            length: width,
            vertical: false,
            colour: -1,
        });
        let hidden = redact(&self.entry);
        out.push(Draw::Text {
            font: Font::B12,
            text: [text("--> "), hidden.clone()].concat(),
            x: 10,
            y: 350 - b12.descent - 1,
            right: false,
            colour: -1,
            shadow: 0xff000000u32 as i32,
        });
        if focused {
            let prefix = hidden
                .get(..self.cursor)
                .ok_or_else(|| anyhow::anyhow!("console caret index"))?;
            let caret = [text("--> "), prefix.to_vec()].concat();
            out.push(Draw::Line {
                x: b12.width_utf16(&caret, None)? + 10,
                y: 350 - b12.descent - 11,
                length: 12,
                vertical: true,
                colour: if cycle % 30 > 15 { 0xffffff } else { -1 },
            });
        }
        out.push(Draw::ResetClip);
        Ok(out)
    }
}

impl ConsoleView for Console {
    fn open(&self) -> bool {
        self.open
    }
    fn draw(
        &self,
        width: i32,
        cycle: i32,
        focused: bool,
        fonts: [&Metrics; 3],
    ) -> anyhow::Result<Vec<Draw>> {
        Console::draw(self, width, cycle, focused, fonts)
    }
}
