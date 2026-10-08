//! Input telemetry: the pointer logger (mouse moves and clicks), the pointer
//! event queue it reads, and the per-cycle report of keyboard, camera, focus,
//! preferences and compressed-texture-format state sent to the server.
//!
//! Cadence: each main-loop cycle flips the pointer queue and routes every
//! polled event: moves go to the pointer logger (logged only while the
//! client is in the game state), click actions join the click list (at most
//! ten); after the state update the list head is removed. The telemetry report
//! is built once per logic cycle and written onto the ordinary game
//! connection queue.
//!
//! The native pointer logger needs a native ICMP service library; without it
//! (this port loads no such library) no native event is ever queued, so that
//! logger's flush and click send produce no packet.
use crate::proto::client as cp;
use std::collections::VecDeque;

/// Pointer event actions as queued by the window owner.
pub mod action {
    pub const MOVE: i32 = -1;
    pub const LEFT: i32 = 0;
    pub const MIDDLE: i32 = 1;
    pub const RIGHT: i32 = 2;
    #[allow(dead_code, reason = "complete pointer action table")]
    pub const LEFT_UP: i32 = 3;
    #[allow(dead_code, reason = "complete pointer action table")]
    pub const MIDDLE_UP: i32 = 4;
    #[allow(dead_code, reason = "complete pointer action table")]
    pub const RIGHT_UP: i32 = 5;
    #[allow(dead_code, reason = "complete pointer action table")]
    pub const WHEEL: i32 = 6;
}

/// A pointer event: button action, canvas position, monotonic time stamp and
/// click count / wheel rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerEvent {
    pub action: i32,
    pub x: i32,
    pub y: i32,
    pub time: i64,
    pub count: i32,
}

impl PointerEvent {
    /// Whether this is a left, middle or right button press.
    pub fn is_click(&self) -> bool {
        matches!(self.action, action::LEFT | action::MIDDLE | action::RIGHT)
    }
}

/// Pointer move/click logger of the applet build: no per-move extra bytes,
/// `EVENT_MOUSE_MOVE` / `EVENT_MOUSE_CLICK`.
#[derive(Clone, Debug)]
pub struct PointerLogger {
    move_queue: VecDeque<PointerEvent>,
    last_click_time: i64,
    last_move_time: i64,
    last_x: i32,
    last_y: i32,
}

impl Default for PointerLogger {
    fn default() -> Self {
        Self {
            move_queue: VecDeque::new(),
            last_click_time: -1,
            last_move_time: -1,
            last_x: -1,
            last_y: -1,
        }
    }
}

impl PointerLogger {
    /// Forgets all queued moves and the last positions and times.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Queues one move event.
    pub fn log_move(&mut self, event: PointerEvent) {
        self.move_queue.push_back(event);
    }

    /// Time since the previous click, capped at `max` (`max` for the first).
    fn click_delay(&mut self, event: &PointerEvent, max: i64) -> i32 {
        let delay = if self.last_click_time == -1 {
            max
        } else {
            (event.time - self.last_click_time).min(max)
        };
        self.last_click_time = event.time;
        delay as i32
    }

    /// Moves flush with a click or once the last flushed move is 2 s old.
    fn should_flush(&self, click_head: Option<&PointerEvent>, now: i64) -> bool {
        click_head.is_some() || self.last_move_time < now - 2000
    }

    /// Writes the queued moves (when due), then the click at the list head.
    /// `click_head` is the head of the click list.
    pub fn flush(&mut self, click_head: Option<&PointerEvent>, now: i64, out: &mut Vec<u8>) {
        if self.should_flush(click_head, now) {
            // Message body: [p1 size][p1 avg][p1 rem][moves...].
            let mut message: Option<Vec<u8>> = None;
            let mut remainder_sum = 0i32;
            let mut count = 0i32;
            // 252 minus the per-move extra size (0 here) and 6 header bytes.
            let limit = 252 - 6;
            while let Some(event) = self.move_queue.front().copied() {
                if let Some(m) = &message {
                    // Bytes after the size byte.
                    if m.len() as i32 - 2 >= limit {
                        break;
                    }
                }
                self.move_queue.pop_front();
                let y = event.y.clamp(-1, 65534);
                let x = event.x.clamp(-1, 65534);
                if self.last_x == x && self.last_y == y {
                    continue;
                }
                let m = message.get_or_insert_with(|| {
                    remainder_sum = 0;
                    count = 0;
                    vec![cp::EVENT_MOUSE_MOVE, 0, 0, 0]
                });
                let (mut dx, mut dy, dt);
                if self.last_move_time == -1 {
                    dx = x;
                    dy = y;
                    dt = i32::MAX;
                } else {
                    dx = x - self.last_x;
                    dy = y - self.last_y;
                    dt = ((event.time - self.last_move_time) / 20) as i32;
                    remainder_sum =
                        (i64::from(remainder_sum) + (event.time - self.last_move_time) % 20) as i32;
                }
                self.last_x = x;
                self.last_y = y;
                if dt < 8 && (-32..=31).contains(&dx) && (-32..=31).contains(&dy) {
                    dx += 32;
                    dy += 32;
                    m.extend_from_slice(&(((dx << 6) + (dt << 12) + dy) as u16).to_be_bytes());
                } else if dt < 32 && (-128..=127).contains(&dx) && (-128..=127).contains(&dy) {
                    dx += 128;
                    dy += 128;
                    m.push((dt + 128) as u8);
                    m.extend_from_slice(&(((dx << 8) + dy) as u16).to_be_bytes());
                } else if dt < 32 {
                    m.push((dt + 192) as u8);
                    let packed = if x == -1 || y == -1 {
                        i32::MIN
                    } else {
                        x | y << 16
                    };
                    m.extend_from_slice(&packed.to_be_bytes());
                } else {
                    m.extend_from_slice(&(((dt & 0x1FFF) + 57344) as u16).to_be_bytes());
                    let packed = if x == -1 || y == -1 {
                        i32::MIN
                    } else {
                        x | y << 16
                    };
                    m.extend_from_slice(&packed.to_be_bytes());
                }
                count += 1;
                // No per-move extra bytes for the applet logger.
                self.last_move_time = event.time;
            }
            if let Some(mut m) = message {
                // Size byte, then the average and remainder of the time deltas.
                m[1] = (m.len() - 2) as u8;
                m[2] = (remainder_sum / count) as u8;
                m[3] = (remainder_sum % count) as u8;
                out.extend(m);
            }
        }
        self.send_click(click_head, out);
    }

    /// Writes the click at the list head:
    /// `EVENT_MOUSE_CLICK`, `p4(x | y << 16)`, `p2_alt3(delay | right << 15)`.
    fn send_click(&mut self, click_head: Option<&PointerEvent>, out: &mut Vec<u8>) {
        let Some(event) = click_head else {
            return;
        };
        let delay = self.click_delay(event, 32767);
        let y = event.y.clamp(0, 65535);
        let x = event.x.clamp(0, 65535);
        let right = i32::from(event.action == action::RIGHT);
        let value = delay | right << 15;
        out.push(cp::EVENT_MOUSE_CLICK);
        out.extend_from_slice(&(x | y << 16).to_be_bytes());
        out.push((value + 128) as u8);
        out.push((value >> 8) as u8);
    }
}

/// Camera inputs of the telemetry report, sampled before the cycle's camera
/// update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraSample {
    /// Orbit camera: pitch and yaw truncated to int, then `>> 3`.
    Orbit { pitch: f32, yaw: f32 },
    /// Free camera following an entity position: its rotation quaternion.
    Entity(crate::ui_cam2::Quat),
    /// Free camera with any other position mode.
    Other,
}

impl CameraSample {
    /// `(yaw, pitch)` as `EVENT_CAMERA_POSITION` writes them.
    pub fn angles(&self) -> (i32, i32) {
        match self {
            Self::Orbit { pitch, yaw } => ((*yaw as i32) >> 3, (*pitch as i32) >> 3),
            Self::Entity(q) => {
                let pitch = camera_pitch(q) >> 3;
                let yaw = (1024 * 3 - (camera_yaw(q) >> 3)) % (1024 * 2);
                (yaw, pitch)
            }
            Self::Other => (0, 0),
        }
    }
}

/// Pitch of a rotation quaternion as a 14-bit angle.
pub fn camera_pitch(q: &crate::ui_cam2::Quat) -> i32 {
    let mut v = crate::ui_cam2::Vec3::new(0.0, 0.0, 1.0);
    v.rotate(q);
    let mut a = std::f64::consts::FRAC_PI_2 - f64::from(v.y).acos();
    if a < 0.0 {
        a = a + std::f64::consts::PI + std::f64::consts::PI;
    }
    (a / std::f64::consts::TAU * 16384.0) as i32 & 0x3FFF
}

/// Yaw of a rotation quaternion as a 14-bit angle.
pub fn camera_yaw(q: &crate::ui_cam2::Quat) -> i32 {
    let mut v = crate::ui_cam2::Vec3::new(0.0, 0.0, 1.0);
    v.rotate(q);
    let mut a = f64::from(v.x).atan2(f64::from(v.z));
    if a < 0.0 {
        a = a + std::f64::consts::PI + std::f64::consts::PI;
    }
    (a / std::f64::consts::TAU * 16384.0) as i32 & 0x3FFF
}

/// Per-cycle inputs of the telemetry report that are owned elsewhere.
pub struct Telemetry<'a> {
    pub now: i64,
    /// This cycle's keyboard presses.
    pub keyboard_events: &'a [crate::ui_keyboard::Event],
    /// Set when the camera moved; cleared when the camera position is sent.
    pub camera_changed: &'a mut bool,
    pub camera: CameraSample,
    /// Whether the window has focus.
    pub focus: bool,
    /// Whether the server was told the current preferences, and the
    /// preferences block to send when it was not.
    pub preferences_notified: &'a mut bool,
    pub preferences_block: &'a dyn Fn() -> Vec<u8>,
    /// Whether the compressed-texture-format report was already sent.
    pub texture_formats_sent: &'a mut bool,
    /// The toolkit preference value.
    pub toolkit: i32,
    /// The active toolkit's texture format codes.
    pub texture_formats: Option<&'a [i32]>,
}

/// Telemetry state plus the click list and the pointer back queue it is
/// filled from.
#[derive(Clone, Debug)]
pub struct InputTelemetry {
    /// The pointer move/click logger.
    pub logger: PointerLogger,
    /// Time of the last reported key event.
    last_key_time: i64,
    /// Camera report cooldown.
    camera_cooldown: i32,
    /// Last reported focus state.
    focus: bool,
    /// Events since the last queue flip.
    back: Vec<PointerEvent>,
    /// Click actions, at most ten.
    pub mouse_events: VecDeque<PointerEvent>,
    /// The previous cycle's click-list head removal is pending.
    head_pending: bool,
}

impl Default for InputTelemetry {
    fn default() -> Self {
        Self {
            logger: PointerLogger::default(),
            last_key_time: -1,
            camera_cooldown: 0,
            focus: true,
            back: Vec::new(),
            mouse_events: VecDeque::new(),
            head_pending: false,
        }
    }
}

impl InputTelemetry {
    /// Session reset: clears the logger, the key clock and the focus state.
    pub fn reset(&mut self) {
        self.logger.reset();
        self.last_key_time = -1;
        self.focus = true;
    }

    /// The window owner calls this for every move (when moves are tracked),
    /// press, release and wheel event.
    pub fn queue_event(&mut self, action: i32, x: i32, y: i32, time: i64, count: i32) {
        self.back.push(PointerEvent {
            action,
            x,
            y,
            time,
            count,
        });
    }

    /// The main loop's pointer half: flips the queue, routes each polled
    /// event and removes the previous cycle's click-list head.
    pub fn mainloop(&mut self, client_state: i32) {
        if std::mem::take(&mut self.head_pending) {
            self.mouse_events.pop_front();
        }
        for event in std::mem::take(&mut self.back) {
            self.route(event, client_state);
        }
        self.head_pending = true;
    }

    /// Routes one polled event.
    fn route(&mut self, event: PointerEvent, client_state: i32) {
        if event.action == action::MOVE {
            // Moves are logged only in the game state.
            if crate::login_state::is_game(client_state) {
                self.logger.log_move(event);
            }
        } else if event.is_click() {
            self.mouse_events.push_back(event);
            if self.mouse_events.len() > 10 {
                self.mouse_events.pop_front();
            }
        }
    }

    /// A diagnostic input injector's event, delivered as if `mainloop` polled
    /// it this cycle (the injectors run after the flip).
    pub fn inject_polled(&mut self, event: PointerEvent, client_state: i32) {
        self.route(event, client_state);
    }

    /// Builds this cycle's telemetry report onto `out`.
    pub fn send_telemetry(&mut self, t: Telemetry<'_>, out: &mut Vec<u8>) {
        // No native events (module doc).
        let head = self.mouse_events.front().copied();
        self.logger.flush(head.as_ref(), t.now, out);
        if !t.keyboard_events.is_empty() {
            out.push(cp::EVENT_KEYBOARD);
            out.extend_from_slice(&((t.keyboard_events.len() * 4) as u16).to_be_bytes());
            for event in t.keyboard_events {
                let delay = (event.time - self.last_key_time).min(16_777_215);
                self.last_key_time = event.time;
                out.push(event.code as u8);
                out.extend_from_slice(&(delay as i32).to_be_bytes()[1..]);
            }
        }
        if self.camera_cooldown > 0 {
            self.camera_cooldown -= 1;
        }
        if *t.camera_changed && self.camera_cooldown <= 0 {
            self.camera_cooldown = 20;
            *t.camera_changed = false;
            let (yaw, pitch) = t.camera.angles();
            out.push(cp::EVENT_CAMERA_POSITION);
            // p2_alt3(yaw), p2(pitch).
            out.push((yaw + 128) as u8);
            out.push((yaw >> 8) as u8);
            out.extend_from_slice(&(pitch as u16).to_be_bytes());
        }
        if self.focus != t.focus {
            self.focus = t.focus;
            out.extend([cp::EVENT_APPLET_FOCUS, u8::from(t.focus)]);
        }
        if !*t.preferences_notified {
            let block = (t.preferences_block)();
            out.push(cp::CLIENT_DETAILOPTIONS_STATUS);
            out.push(block.len() as u8);
            out.extend(block);
            *t.preferences_notified = true;
        }
        if *t.texture_formats_sent || t.toolkit != 1 {
            return;
        }
        let body = compressed_texture_format_body(t.texture_formats);
        out.push(cp::CLIENT_COMPRESSEDTEXTUREFORMAT_SUPPORT);
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
        out.extend(body);
        *t.texture_formats_sent = true;
    }
}
pub use crate::compressed_texture_format::*;

/// Body of `CLIENT_COMPRESSEDTEXTUREFORMAT_SUPPORT`:
/// `p1(0)` without formats, else `p1(1)`, the known-format bit set and the
/// unknown codes as a smart count, minimum and deltas.
pub fn compressed_texture_format_body(formats: Option<&[i32]>) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(formats) = formats.filter(|f| !f.is_empty()) else {
        out.push(0);
        return out;
    };
    out.push(1);
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for &code in formats {
        match COMPRESSED_TEXTURE_FORMATS.iter().position(|&c| c == code) {
            Some(id) => known.push(id),
            None => unknown.push(code),
        }
    }
    // The known-format set: a smart size, then that many bitmap bytes.
    let size = known.iter().copied().max().map_or(0, |max| (max + 8) / 8);
    put_smart1or2s(&mut out, size as i32);
    let start = out.len();
    out.resize(start + size, 0);
    for id in known {
        out[start + id / 8] |= 1 << (id & 7);
    }
    put_smart1or2(&mut out, unknown.len() as i32);
    if let Some(&min) = unknown.iter().min() {
        put_smart2or4(&mut out, min);
        for &code in &unknown {
            if code != min {
                put_smart2or4(&mut out, code - min);
            }
        }
    }
    out
}

/// Signed smart, 1 or 2 bytes.
fn put_smart1or2(out: &mut Vec<u8>, v: i32) {
    if (-64..64).contains(&v) {
        out.push((v + 64) as u8);
    } else {
        out.extend_from_slice(&((v + 49152) as u16).to_be_bytes());
    }
}

/// Unsigned smart, 1 or 2 bytes.
fn put_smart1or2s(out: &mut Vec<u8>, v: i32) {
    if (0..128).contains(&v) {
        out.push(v as u8);
    } else {
        out.extend_from_slice(&((v + 32768) as u16).to_be_bytes());
    }
}

/// Smart of 2 or 4 bytes (-1 has its own form).
fn put_smart2or4(out: &mut Vec<u8>, v: i32) {
    if v == -1 {
        out.extend_from_slice(&32767u16.to_be_bytes());
    } else if v < 32767 {
        out.extend_from_slice(&(v as u16).to_be_bytes());
    } else {
        let mut b = v.to_be_bytes();
        b[0] |= 0x80;
        out.extend_from_slice(&b);
    }
}

/// Split a raw outgoing stream into `(opcode, payload)` frames using the
/// client protocol size table (`CLIENT910_OUT_TRACE`). `None` on an unknown
/// opcode or a truncated frame.
pub fn frames(mut bytes: &[u8]) -> Option<Vec<(u8, &[u8])>> {
    let mut out = Vec::new();
    while let Some((&opcode, rest)) = bytes.split_first() {
        let size = cp::size(opcode)?;
        let (len, rest) = match size {
            -1 => (usize::from(*rest.first()?), &rest[1..]),
            -2 => (
                usize::from(u16::from_be_bytes([*rest.first()?, *rest.get(1)?])),
                &rest[2..],
            ),
            n => (n as usize, rest),
        };
        if rest.len() < len {
            return None;
        }
        out.push((opcode, &rest[..len]));
        bytes = &rest[len..];
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(x: i32, y: i32, time: i64) -> PointerEvent {
        PointerEvent {
            action: action::MOVE,
            x,
            y,
            time,
            count: 0,
        }
    }

    #[test]
    fn first_move_uses_absolute_long_form_then_deltas() {
        // Traced by hand: the first event has no previous move time so
        // dt = MAX_VALUE (long form, 0xE000 | 0x1FFF);
        // the next is a 1-cycle small delta, the third a medium delta.
        let mut logger = PointerLogger::default();
        // Due to flush: last move time -1 < now - 2000 (the clock is large).
        logger.log_move(mv(100, 200, 10_000));
        logger.log_move(mv(110, 195, 10_030));
        logger.log_move(mv(200, 150, 10_075));
        let mut out = Vec::new();
        logger.flush(None, 10_100, &mut out);
        let long = ((i32::MAX & 0x1FFF) + 57344) as u16;
        let small: i32 = ((10 + 32) << 6) + (1 << 12) + (-5 + 32);
        let mut expected = vec![cp::EVENT_MOUSE_MOVE, 0, 0, 0];
        expected.extend(long.to_be_bytes());
        expected.extend((100i32 | 200 << 16).to_be_bytes());
        expected.extend((small as u16).to_be_bytes());
        expected.push((2 + 128) as u8);
        expected.extend(((((90 + 128) << 8) + (-45 + 128)) as u16).to_be_bytes());
        // Remainders: 30 % 20 + 45 % 20 = 15; three events.
        expected[1] = (expected.len() - 2) as u8;
        expected[2] = 15 / 3;
        expected[3] = 15 % 3;
        assert_eq!(out, expected);
        // Nothing queued and the last move is recent: no flush, no click.
        let mut out = Vec::new();
        logger.flush(None, 10_200, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn repeated_positions_are_dropped_and_moves_wait_for_click_or_two_seconds() {
        let mut logger = PointerLogger::default();
        logger.log_move(mv(5, 5, 10_000));
        let mut out = Vec::new();
        logger.flush(None, 10_010, &mut out);
        assert_eq!(out[0], cp::EVENT_MOUSE_MOVE);
        // A move 100 ms later is held (no click, last move not 2 s old).
        logger.log_move(mv(6, 5, 10_100));
        logger.log_move(mv(6, 5, 10_120));
        let mut out = Vec::new();
        logger.flush(None, 10_200, &mut out);
        assert!(out.is_empty());
        // A click at the list head flushes the moves and sends the click;
        // the duplicate (6,5) is released without bytes.
        let click = PointerEvent {
            action: action::RIGHT,
            x: 6,
            y: 5,
            time: 10_250,
            count: 1,
        };
        let mut out = Vec::new();
        logger.flush(Some(&click), 10_260, &mut out);
        let frames = frames(&out).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, cp::EVENT_MOUSE_MOVE);
        // One event: header (avg, rem) + one 2-byte small delta (dt 5).
        assert_eq!(
            frames[0].1,
            &[
                0,
                0,
                ((((1 + 32) << 6) + (5 << 12) + 32) >> 8) as u8,
                ((1 + 32) << 6) as u8 + 32
            ]
        );
        // EVENT_MOUSE_CLICK: p4(x | y << 16), p2_alt3(32767 | 1 << 15) (first click).
        let value = 32767 | 1 << 15;
        assert_eq!(
            frames[1],
            (
                cp::EVENT_MOUSE_CLICK,
                &[0, 5, 0, 6, (value + 128) as u8, (value >> 8) as u8][..]
            )
        );
    }

    #[test]
    fn move_message_splits_at_246_payload_bytes() {
        let mut logger = PointerLogger::default();
        // Every event is a long-form jump (6 bytes) 1 s apart.
        for i in 0..60 {
            logger.log_move(mv(i * 300 % 60000, i, i64::from(i) * 1000));
        }
        let mut out = Vec::new();
        logger.flush(None, 100_000, &mut out);
        let frames = frames(&out).unwrap();
        assert_eq!(frames.len(), 1);
        // 2 header bytes + 41 events * 6 bytes = 248 >= 246 stops the loop.
        assert_eq!(frames[0].1.len(), 2 + 41 * 6);
        assert_eq!(logger.move_queue.len(), 60 - 41);
    }

    #[test]
    fn mainloop_routes_moves_only_in_game_and_keeps_ten_clicks() {
        let mut watch = InputTelemetry::default();
        watch.queue_event(action::MOVE, 1, 1, 0, 0);
        watch.mainloop(crate::login_state::LOGIN);
        assert!(watch.logger.move_queue.is_empty());
        for i in 0..12 {
            watch.queue_event(action::LEFT, i, 0, i64::from(i), 1);
        }
        watch.queue_event(action::MOVE, 2, 2, 20, 0);
        watch.mainloop(crate::login_state::GAME);
        assert_eq!(watch.logger.move_queue.len(), 1);
        assert_eq!(watch.mouse_events.len(), 10);
        assert_eq!(watch.mouse_events.front().unwrap().x, 2);
        // One click-list head is removed per cycle.
        watch.mainloop(crate::login_state::GAME);
        assert_eq!(watch.mouse_events.front().unwrap().x, 3);
    }

    #[test]
    fn telemetry_cadence_keyboard_camera_focus_preferences() {
        let mut watch = InputTelemetry::default();
        let keys = [
            crate::ui_keyboard::Event {
                action: 0,
                ch: 65535,
                code: 33,
                time: 5000,
                modifiers: 0,
            },
            crate::ui_keyboard::Event {
                action: 0,
                ch: 65535,
                code: 34,
                time: 5040,
                modifiers: 0,
            },
        ];
        let mut changed = true;
        let mut notified = false;
        let mut formats_sent = false;
        let block = || vec![38u8, 1, 2];
        let mut out = Vec::new();
        watch.send_telemetry(
            Telemetry {
                now: 5100,
                keyboard_events: &keys,
                camera_changed: &mut changed,
                camera: CameraSample::Orbit {
                    pitch: 1088.0,
                    yaw: 2048.0,
                },
                focus: false,
                preferences_notified: &mut notified,
                preferences_block: &block,
                texture_formats_sent: &mut formats_sent,
                toolkit: 1,
                texture_formats: None,
            },
            &mut out,
        );
        let frames = frames(&out).unwrap();
        let names: Vec<_> = frames.iter().map(|f| cp::name(f.0)).collect();
        // The first flush is due (last move time -1) but there are no moves.
        assert_eq!(
            names,
            [
                "EVENT_KEYBOARD",
                "EVENT_CAMERA_POSITION",
                "EVENT_APPLET_FOCUS",
                "CLIENT_DETAILOPTIONS_STATUS",
                "CLIENT_COMPRESSEDTEXTUREFORMAT_SUPPORT"
            ]
        );
        // First key delay = 5000 - (-1), then 40 ms.
        assert_eq!(frames[0].1, &[33, 0, 0x13, 0x89, 34, 0, 0, 40]);
        // p2_alt3(2048 >> 3 = 256), p2(1088 >> 3 = 136).
        assert_eq!(frames[1].1, &[128, 1, 0, 136]);
        assert_eq!(frames[2].1, &[0]);
        assert_eq!(frames[3].1, &[38, 1, 2]);
        assert_eq!(frames[4].1, &[0]);
        assert!(!changed && notified && formats_sent);
        // The camera cooldown (20 cycles) holds the next report.
        let mut changed = true;
        let mut out = Vec::new();
        for _ in 0..19 {
            watch.send_telemetry(
                Telemetry {
                    now: 5200,
                    keyboard_events: &[],
                    camera_changed: &mut changed,
                    camera: CameraSample::Other,
                    focus: false,
                    preferences_notified: &mut notified,
                    preferences_block: &block,
                    texture_formats_sent: &mut formats_sent,
                    toolkit: 1,
                    texture_formats: None,
                },
                &mut out,
            );
        }
        assert!(out.is_empty() && changed);
        watch.send_telemetry(
            Telemetry {
                now: 5200,
                keyboard_events: &[],
                camera_changed: &mut changed,
                camera: CameraSample::Other,
                focus: false,
                preferences_notified: &mut notified,
                preferences_block: &block,
                texture_formats_sent: &mut formats_sent,
                toolkit: 1,
                texture_formats: None,
            },
            &mut out,
        );
        assert_eq!(out, [cp::EVENT_CAMERA_POSITION, 128, 0, 0, 0]);
    }

    #[test]
    fn compressed_texture_format_body_encodes_known_and_unknown_codes() {
        assert_eq!(compressed_texture_format_body(None), [0]);
        // S3TC DXT1-5 (ids 0-3) + ETC2 RGB8 (id 14) + two unknown codes.
        let body = compressed_texture_format_body(Some(&[
            33776, 33777, 33778, 33779, 37492, 36283, 36492,
        ]));
        // Known set: size (14 + 8) / 8 = 2 -> bytes 0x0F, 0x40; two unknown:
        // pSmart1or2(2) = 66, min 36283 (>= 32767: p4 with the top bit), delta 209.
        assert_eq!(body, [1, 2, 0x0F, 0x40, 66, 0x80, 0, 0x8D, 0xBB, 0, 209]);
    }
}
