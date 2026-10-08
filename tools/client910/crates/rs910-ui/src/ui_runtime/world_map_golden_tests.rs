//! The world map against the original client's recording.
//!
//! `fixtures/recorded/world-map/cases.txt` is a script of world-map steps: an
//! area load, animation cycles, `worldmap_*` commands, and draws of the map
//! and its overview. The original client ran the same script over the same
//! pack; its trace (every toolkit draw call with a digest of the sprite it
//! draws, the element hit boxes, the animation state and the stacks the
//! commands left) is `trace.blk`. This test runs the script through the
//! production owners (`ClientWorldMap`, the engine's command table) with a
//! canvas that writes the same trace, and compares the two.
use super::*;
use crate::ui_sprites::Sprite;
use crate::world_map::Canvas;
use crate::world_map_client::LoadRoll;
use native910::{script::Operand, vm::InstructionContext};
use std::fmt::Write as _;
use std::rc::Rc;

/// FNV-1a 64 over the big-endian bytes of the pixels, the digest the
/// recording harness computes for every sprite.
fn digest(pixels: &[i32]) -> u64 {
    pixels
        .iter()
        .flat_map(|p| p.to_be_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// A sprite as the trace names it: drawn size, full size and pixel digest.
fn identity(s: &Sprite) -> String {
    let [fw, fh] = s.full_size();
    if let Ok(dir) = std::env::var("WORLD_MAP_DUMP_DIR") {
        let bytes: Vec<u8> = s.argb.iter().flat_map(|p| p.to_be_bytes()).collect();
        let _ = std::fs::write(format!("{dir}/{:016x}.bin", digest(&s.argb)), bytes);
    }
    format!(
        "{} {} {} {} {:016x}",
        s.size[0],
        s.size[1],
        fw,
        fh,
        digest(&s.argb)
    )
}

/// The three corners a sprite of `full` size rotated by `angle` about
/// `centre` covers (the toolkit's rotated blit, in single precision).
fn rotated_corners(full: [i32; 2], centre: [f32; 2], angle: i32) -> [f32; 6] {
    let [gx, gy] = [full[0] as f32, full[1] as f32];
    let (ax, ay) = (gx / 2.0, gy / 2.0);
    let radians = f64::from(angle & 0xFFFF) * 9.587_379_924_285_257E-5;
    let sin = (radians.sin() as f32) * 4096.0;
    let cos = (radians.cos() as f32) * 4096.0;
    [
        (-ax * cos + -ay * sin) / 4096.0 + centre[0],
        (ax * sin + -ay * cos) / 4096.0 + centre[1],
        ((gx - ax) * cos + -ay * sin) / 4096.0 + centre[0],
        (-(gx - ax) * sin + -ay * cos) / 4096.0 + centre[1],
        (-ax * cos + (gy - ay) * sin) / 4096.0 + centre[0],
        (ax * sin + (gy - ay) * cos) / 4096.0 + centre[1],
    ]
}

/// A canvas that writes the trace lines.
struct Trace {
    out: String,
    clip: [i32; 4],
    fonts: crate::ui_fonts::Fonts,
}
/// The canvas width label paragraphs are measured against.
const CANVAS_WIDTH: i32 = 1024;
/// A label as the trace prints it: printable ASCII as is, every other UTF-16
/// unit (and the backslash) as `\uXXXX`.
fn escaped(text: &str) -> String {
    text.encode_utf16()
        .map(|c| match c {
            32..=126 if c != 92 => char::from(c as u8).to_string(),
            _ => format!("\\u{c:04x}"),
        })
        .collect()
}
impl Canvas for Trace {
    fn sprite(&mut self, s: &Rc<Sprite>, pos: [i32; 2]) {
        let _ = writeln!(self.out, "spr {} {} {}", pos[0], pos[1], identity(s));
    }
    fn scaled(&mut self, s: &Rc<Sprite>, r: [i32; 4]) {
        let _ = writeln!(
            self.out,
            "scl {} {} {} {} {}",
            r[0],
            r[1],
            r[2],
            r[3],
            identity(s)
        );
    }
    fn sprite_tinted(&mut self, s: &Rc<Sprite>, pos: [i32; 2], colour: i32) {
        let _ = writeln!(
            self.out,
            "tint {colour} {} {} {}",
            pos[0],
            pos[1],
            identity(s)
        );
    }
    fn rotated(&mut self, s: &Rc<Sprite>, centre: [f32; 2], angle: i32) {
        let c = rotated_corners(s.full_size(), centre, angle);
        let _ = writeln!(
            self.out,
            "rot {:?} {:?} {:?} {:?} {:?} {:?} {}",
            c[0],
            c[1],
            c[2],
            c[3],
            c[4],
            c[5],
            identity(s)
        );
    }
    fn fill(&mut self, r: [i32; 4], colour: i32) {
        let _ = writeln!(
            self.out,
            "fill {} {} {} {} {colour}",
            r[0], r[1], r[2], r[3]
        );
    }
    fn outline(&mut self, r: [i32; 4], colour: i32) {
        let _ = writeln!(
            self.out,
            "rect {} {} {} {} {colour}",
            r[0], r[1], r[2], r[3]
        );
    }
    fn line(&mut self, a: [i32; 2], b: [i32; 2], colour: i32, style: [i32; 3]) {
        if style[0] <= 0 {
            let _ = writeln!(
                self.out,
                "line {} {} {} {} {colour}",
                a[0], a[1], b[0], b[1]
            );
        } else {
            let _ = writeln!(
                self.out,
                "dash {} {} {} {} {colour} {} {} {}",
                a[0], a[1], b[0], b[1], style[0], style[1], style[2]
            );
        }
    }
    fn polygon(&mut self, points: &[i32], colour: i32) {
        for span in crate::world_map_polygon::spans(points, self.clip[1], self.clip[3], None) {
            let _ = writeln!(
                self.out,
                "hline {} {} {} {colour}",
                span.x, span.y, span.len
            );
        }
    }
    fn measure(&mut self, font: i32, text: &str) -> Option<[i32; 2]> {
        self.fonts.paragraph_size(font, text, CANVAS_WIDTH)
    }
    fn text(&mut self, font: i32, text: &str, r: [i32; 4], colour: i32, shadow: i32) {
        let _ = writeln!(
            self.out,
            "text {font} {} {} {} {} {colour} {shadow} 1 0 0 0 {}",
            r[0],
            r[1],
            r[2],
            r[3],
            escaped(text)
        );
    }
    fn text_centre(&mut self, text: &str, pos: [i32; 2], colour: i32) {
        let _ = writeln!(self.out, "textc {} {} {colour} -1 {text}", pos[0], pos[1]);
    }
}

struct Run {
    engine: Engine,
    trace: Trace,
}

impl Run {
    fn new() -> Self {
        let pack = crate::test_support::require_pack("client.worldmap.js5");
        let mut engine = Engine::default();
        engine.load_cache(&pack);
        Self {
            engine,
            trace: Trace {
                out: String::new(),
                clip: [0; 4],
                fonts: crate::ui_fonts::Fonts::from_pack(pack, None).expect("fonts"),
            },
        }
    }

    fn line(&mut self, text: &str) {
        self.trace.out.push_str(text);
        self.trace.out.push('\n');
    }

    /// Loads an area the way a newly chosen map loads: the given colour
    /// jitter, the player at the source tile `player` or the area's origin.
    fn load(&mut self, area: i32, members: bool, hue: i32, light: i32, player: Option<[i32; 2]>) {
        {
            let mut wm = self.engine.world_map.borrow_mut();
            wm.reset();
            wm.set_map(area, -1, -1, false);
        }
        self.stages(members, hue, light, player);
    }

    /// The loading steps of the current map without the random choices (they
    /// do nothing once the map is loaded).
    fn stages(&mut self, members: bool, hue: i32, light: i32, player: Option<[i32; 2]>) {
        let mut wm = self.engine.world_map.borrow_mut();
        if wm.loading == 100 {
            return;
        }
        wm.members = members;
        wm.map.hue_jitter = hue;
        wm.map.lightness_jitter = light;
        wm.roll = Some(LoadRoll::default());
        let player = player.map(|[x, z]| [0, x, z]);
        for _ in 0..8 {
            wm.update_loading(player, &|_| true);
        }
        assert_eq!(wm.loading, 100, "the area did not finish loading");
        let m = wm.map.metadata().expect("current area");
        let a = wm.map.area.as_ref().expect("area");
        let line = format!(
            "loaded area {} origin {} {} size {} {} elements {}",
            m.id,
            a.origin[0],
            a.origin[1],
            a.size[0],
            a.size[1],
            a.elements.len()
        );
        drop(wm);
        self.line(&line);
    }

    /// Sets a local player variable the way the game variable store does:
    /// the map reads the varp array and the varbit definitions.
    fn set_var(&mut self, varbit: bool, id: i32, value: i32) {
        let defs = self.engine.configs.inv_varbits.clone();
        let mut wm = self.engine.world_map.borrow_mut();
        wm.varbits = defs.clone();
        let (varp, value) = if varbit {
            let defs = defs.expect("varbit definitions");
            let t = defs.get(id, false).expect("varbit");
            let base = t.binding.as_ref().expect("bound varbit").id;
            let raw = wm.varps.get(base as usize).copied().unwrap_or(0);
            let Ok(next) = t.set(raw, value) else {
                drop(wm);
                self.line("varbit overflow");
                return;
            };
            (base, next)
        } else {
            (id, value)
        };
        if wm.varps.len() <= varp as usize {
            wm.varps.resize(varp as usize + 1, 0);
        }
        wm.varps[varp as usize] = value;
    }

    fn state(&mut self) {
        let wm = self.engine.world_map.borrow();
        let mut text = format!(
            "state zoom {:?} target {:?} pos {} {} jump {} {} flashes",
            wm.map.zoom, wm.map.target_zoom, wm.position[0], wm.position[1], wm.jump[0], wm.jump[1]
        );
        for (id, f) in &wm.flash_elements {
            let _ = write!(text, " e{id}:{}:{}", f.loops, f.ticks);
        }
        for (id, f) in &wm.flash_categories {
            let _ = write!(text, " c{id}:{}:{}", f.loops, f.ticks);
        }
        drop(wm);
        self.line(&text);
    }

    fn boxes(&mut self) {
        let wm = self.engine.world_map.borrow();
        let mut text = String::new();
        if let (Some(containers), Some(area)) = (wm.containers.as_ref(), wm.map.area.as_ref()) {
            for c in containers {
                let id = area.elements[c.element].id;
                let [a, b, cc, d] = c.sprite;
                let [e, f, g, h] = c.text;
                let _ = writeln!(text, "box {id} {a} {b} {cc} {d} {e} {f} {g} {h}");
            }
        }
        drop(wm);
        self.trace.out.push_str(&text);
    }

    fn bounds(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.trace.clip = [x, y, x + w, y + h];
        let _ = writeln!(self.trace.out, "bounds {x} {y} {} {}", x + w, y + h);
    }

    fn draw(&mut self, [x, y, w, h]: [i32; 4]) {
        self.bounds(x, y, w, h);
        let mut wm = self.engine.world_map.borrow_mut();
        wm.draw(&mut self.trace, [x, y, w, h]);
        let text = format!(
            "window {} {} {} {}",
            wm.window[0], wm.window[1], wm.window[2], wm.window[3]
        );
        drop(wm);
        self.line(&text);
        self.boxes();
    }

    fn overview(&mut self, [x, y, w, h]: [i32; 4]) {
        self.bounds(x, y, w, h);
        let mut wm = self.engine.world_map.borrow_mut();
        wm.draw_overview(&mut self.trace, [x, y, w, h]);
    }

    fn command(&mut self, name: &str, args: &[i32]) {
        let operand = Operand::Byte(0);
        let context = InstructionContext {
            script_name: Some("world-map-golden"),
            script_id: None,
            event: None,
            pc: 0,
            command: name,
            operand: &operand,
            secondary: false,
            int_locals: &[],
        };
        let mut ints = args.to_vec();
        let mut objects = Vec::new();
        let result = self
            .engine
            .trap_context(&context, &mut ints, &mut objects, &mut Vec::new());
        let thrown = match result {
            Ok(Some(Value::Int(v))) => {
                ints.push(v);
                "-"
            }
            Ok(Some(Value::Str(text))) => {
                objects.push(text);
                "-"
            }
            Ok(Some(other)) => panic!("{name}: unexpected result {other:?}"),
            Ok(None) => "-",
            Err(_) => "error",
        };
        let list = |v: &[String], sep: &str| {
            let joined = v.join(sep);
            if joined.is_empty() {
                "-".to_string()
            } else {
                joined
            }
        };
        let ints_text = list(&ints.iter().map(i32::to_string).collect::<Vec<_>>(), ",");
        let objs_text = list(&objects, "|");
        self.line(&format!(
            "stack ints {ints_text} objs {objs_text} thrown {thrown}"
        ));
    }

    fn exec(&mut self, line: &str) {
        self.line(&format!("> {line}"));
        let p: Vec<&str> = line.split_whitespace().collect();
        let n = |i: usize| -> i32 { p[i].parse().expect("number") };
        match p[0] {
            "finish" => {
                let player = (p.len() > 5).then(|| [n(4), n(5)]);
                self.stages(p[1] == "1", n(2), n(3), player);
            }
            "load" => {
                let player = (p.len() > 6).then(|| [n(5), n(6)]);
                self.load(n(1), p[2] == "1", n(3), n(4), player);
            }
            "update" => {
                for _ in 0..n(1) {
                    self.engine.world_map.borrow_mut().update([-1, -1]);
                }
                self.state();
            }
            "varp" => self.set_var(false, n(1), n(2)),
            "varbit" => self.set_var(true, n(1), n(2)),
            "state" => self.state(),
            "elements" => {
                let wm = self.engine.world_map.borrow();
                let mut text = String::new();
                for e in &wm.map.area.as_ref().expect("area").elements {
                    let _ = writeln!(text, "el {} {} {}", e.id, e.x, e.z);
                }
                drop(wm);
                self.trace.out.push_str(&text);
            }
            "cs2" => {
                let args: Vec<i32> = p[2..].iter().map(|v| v.parse().expect("number")).collect();
                self.command(p[1], &args);
            }
            "draw" => self.draw([n(1), n(2), n(3), n(4)]),
            "overview" => self.overview([n(1), n(2), n(3), n(4)]),
            other => panic!("unknown step {other}"),
        }
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn world_map_matches_the_original_client() {
    let cases = match std::env::var("WORLD_MAP_CASES") {
        Ok(path) => std::fs::read_to_string(path).unwrap(),
        Err(_) => rs910_core::test_support::frozen::text("world-map/cases.txt"),
    };
    let mut run = Run::new();
    for line in cases.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        run.exec(line);
    }
    let text = run.trace.out.clone();
    if let Ok(path) = std::env::var("WORLD_MAP_TRACE_OUT") {
        std::fs::write(path, &text).unwrap();
    }
    rs910_core::test_support::frozen::assert_stream("world-map/trace", text.as_bytes());
}
