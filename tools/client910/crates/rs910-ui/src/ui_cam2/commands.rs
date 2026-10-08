//! Scripted-camera command dispatch and stack validation.

pub use crate::cam2_scene::*;

use native910::vm::{Value, VmError, VmResult};

use rs910_core::fault::Fault;

pub use rs910_core::vector_math::*;

use super::{
    atan2_positive, decode_coord_fine, encode_coord_fine, sphere_intersections, Cam2, Effect,
    EntityFocus, EntityPlacement, EntityPosition, Focus, Position, TrackableRef, MODE_ENTITY,
    MODE_ORIENTATION, MODE_POINT, RADIANS_TO_UNITS,
};

impl Cam2 {
    /// A failed camera command; the reason names its fault class.
    pub(super) fn trap(command: &str, reason: &str) -> VmError {
        VmError::TrapFailed {
            command: command.into(),
            reason: reason.into(),
        }
    }
    /// The command needs a camera mode the camera is not in.
    pub(super) fn wrong_mode(command: &str) -> VmError {
        Self::trap(
            command,
            &Fault::InvalidState.message("camera mode does not match the command"),
        )
    }
    /// The mode is set but its owner is of another kind.
    pub(super) fn wrong_owner(command: &str) -> VmError {
        Self::trap(
            command,
            &Fault::WrongValueType.message("camera owner does not match the mode"),
        )
    }
    /// The camera setting is refused in the current control or movement mode.
    pub(super) fn refused(command: &str) -> VmError {
        Self::trap(
            command,
            &Fault::InvalidState.message("camera setting refused"),
        )
    }
    /// An object-stack value that is not a coordinate.
    pub(super) fn not_a_coordinate(command: &str) -> VmError {
        Self::trap(
            command,
            &Fault::WrongValueType.message("object is not a coordinate"),
        )
    }
    /// The next effect id; ids wrap to 0 at 126.
    pub(super) fn next_effect_id(&mut self) -> i32 {
        self.effect_id_counter += 1;
        if self.effect_id_counter == 126 {
            self.effect_id_counter = 0;
        }
        self.effect_id_counter
    }
    /// `None` when `command` is not a camera command handled here.
    #[cfg_attr(not(test), allow(dead_code, reason = "exercised by tests only"))]
    pub fn dispatch(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
    ) -> Option<VmResult<Option<Value>>> {
        let mut objects = Vec::new();
        self.dispatch_active_with_objects(command, ints, &mut objects, None)
    }
    /// Dispatch a camera command while an active-sub/process trigger owns an
    /// NPC. The `cam2_*entity_npc` commands resolve through that active entity
    /// rather than the local-player slot.
    #[cfg_attr(not(test), allow(dead_code, reason = "exercised by tests only"))]
    pub fn dispatch_active(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        active_npc: Option<i32>,
    ) -> Option<VmResult<Option<Value>>> {
        let mut objects = Vec::new();
        self.dispatch_active_with_objects(command, ints, &mut objects, active_npc)
    }
    /// Dispatch a camera command with access to the heterogeneous object lane.
    /// Coordinate values use the tagged representation above until the native VM
    /// grows typed object-stack storage.
    pub fn dispatch_active_with_objects(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        objects: &mut Vec<String>,
        active_npc: Option<i32>,
    ) -> Option<VmResult<Option<Value>>> {
        if !(command.starts_with("cam2_") || command == "cam_modeisfollowplayer") {
            return None;
        }
        let args = ints.clone();
        let result = self.run(command, ints, objects, active_npc);
        if crate::ui_debug_flags::flags().ui_cam2_trace {
            let tail = &args[args.len().saturating_sub(8)..];
            log::info!("[trace] cam2 {command} args{tail:?} -> {result:?} state={} modes=({:?},{:?}) ready={} player={:?}", self.camera_state, self.lookat_mode, self.position_mode, self.ready(), self.scene.local_player.map(|p| (p.index, p.level, p.coord)));
        }
        Some(result)
    }
    pub(super) fn pop(ints: &mut Vec<i32>, n: usize) -> VmResult<Vec<i32>> {
        if ints.len() < n {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        Ok(ints.split_off(ints.len() - n))
    }
    pub(super) fn infinite(v: [f32; 3]) -> [f32; 3] {
        v.map(|c| if c == -1.0 { f32::INFINITY } else { c })
    }
    /// Converts radians to angle units (16384 per turn), wrapped to one turn.
    pub(super) fn units(radians: f32) -> i32 {
        (f64::from(radians) * RADIANS_TO_UNITS) as i32 & 0x3FFF
    }
    /// Converts angle units (16384 per turn) to radians.
    pub(super) fn radians_of(units: i32) -> f32 {
        (f64::from(units) * std::f64::consts::PI * 2.0 / 16384.0) as f32
    }
    pub(super) fn entity_owners(
        &mut self,
        command: &str,
    ) -> VmResult<(&mut EntityPosition, &mut EntityFocus)> {
        if self.position_mode != Some(MODE_ENTITY) || self.lookat_mode != Some(MODE_ENTITY) {
            return Err(Self::wrong_mode(command));
        }
        match (self.position.as_mut(), self.lookat.as_mut()) {
            (Some(Position::Entity(p)), Some(Focus::Entity(l))) => Ok((p, l)),
            _ => Err(Self::wrong_owner(command)),
        }
    }
    pub(super) fn run(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        objects: &mut Vec<String>,
        active_npc: Option<i32>,
    ) -> VmResult<Option<Value>> {
        let refused = || Self::refused(command);
        let wrong_mode = || Self::wrong_mode(command);
        let unit = Ok(None);
        match command {
            "cam_modeisfollowplayer" => Ok(Some(Value::Int(i32::from(self.camera_state == 2)))),
            "cam2_legacycam_ready" => Ok(Some(Value::Int(1))),
            "cam2_enable" => {
                let on = Self::pop(ints, 1)?[0] == 1;
                self.camera_reset(if on { 3 } else { 2 });
                unit
            }
            "cam2_isenabled" => Ok(Some(Value::Int(i32::from(self.camera_state == 3)))),
            // Mode setters skip the server-control guard; a known mode creates its owner
            // and any other mode keeps the old owner.
            "cam2_setlookatmode" => {
                let mode = Self::pop(ints, 1)?[0];
                self.lookat_mode = (0..=6).contains(&mode).then_some(mode);
                if let Some(mode) = self.lookat_mode {
                    self.lookat = Some(Focus::for_mode(mode));
                }
                self.changed = true;
                unit
            }
            "cam2_setpositionmode" => {
                let mode = Self::pop(ints, 1)?[0];
                self.position_mode = (0..=4).contains(&mode).then_some(mode);
                if let Some(mode) = self.position_mode {
                    self.position = Some(Position::for_mode(mode));
                }
                self.changed = true;
                unit
            }
            // Orientation focus commands: vector, rotation and movement updates.
            "cam2_setlookatorientation_vector" => {
                let a = Self::pop(ints, 3)?;
                if self.lookat_mode != Some(MODE_ORIENTATION) {
                    return Err(wrong_mode());
                }
                let Some(Focus::Orientation(orientation)) = self.lookat.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                orientation.set_vector(a[0], a[1], a[2]);
                self.changed = true;
                unit
            }
            "cam2_setlookatorientation_xrotation"
            | "cam2_setlookatorientation_yrotation"
            | "cam2_setlookatorientation_xmovement"
            | "cam2_setlookatorientation_zmovement" => {
                let value = Self::pop(ints, 1)?[0];
                if self.lookat_mode != Some(MODE_ORIENTATION) {
                    return Err(wrong_mode());
                }
                let Some(Focus::Orientation(orientation)) = self.lookat.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                match command {
                    "cam2_setlookatorientation_xrotation" => orientation.rotation_x = value,
                    "cam2_setlookatorientation_yrotation" => orientation.rotation_y = value,
                    "cam2_setlookatorientation_xmovement" => orientation.movement_x = value,
                    "cam2_setlookatorientation_zmovement" => orientation.movement_z = value,
                    _ => unreachable!(),
                }
                self.changed = true;
                unit
            }
            "cam2_getcontrolmode" => Ok(Some(Value::Int(self.control_mode))),
            "cam2_getlookatmode" => Ok(Some(Value::Int(self.lookat_mode.unwrap_or(-1)))),
            "cam2_getpositionmode" => Ok(Some(Value::Int(self.position_mode.unwrap_or(-1)))),
            // Follows the local player or the active NPC.
            "cam2_setlookatentity_player" | "cam2_setlookatentity_npc" => {
                let a = Self::pop(ints, 4)?;
                if self.lookat_mode != Some(MODE_ENTITY) {
                    return Err(wrong_mode());
                }
                let Some(Focus::Entity(l)) = self.lookat.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                let trackable = if command.ends_with("_npc") {
                    active_npc.map(|index| TrackableRef {
                        kind: TRACKABLE_NPC,
                        index,
                    })
                } else {
                    self.scene.local_player.map(|p| TrackableRef {
                        kind: p.kind,
                        index: p.index,
                    })
                };
                if let Some(trackable) = trackable {
                    l.set(
                        trackable,
                        Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32),
                        a[3] == 1,
                    );
                }
                self.changed = true;
                unit
            }
            // Places the eye on the local player or the active NPC.
            "cam2_setpositionentity_player" | "cam2_setpositionentity_npc" => {
                let a = Self::pop(ints, 7)?;
                if self.position_mode != Some(MODE_ENTITY) {
                    return Err(wrong_mode());
                }
                let mut offset_rotation = Quat::rotation(0.0, 1.0, 0.0, Self::radians_of(a[4]));
                let mut pitch_axis = Vec3::new(1.0, 0.0, 0.0);
                pitch_axis.rotate(&offset_rotation);
                pitch_axis.negate();
                let pitch_rotation = Quat::rotation_axis(&pitch_axis, Self::radians_of(a[3]));
                offset_rotation.multiply(&pitch_rotation);
                let collision = self.collision;
                let Some(Position::Entity(p)) = self.position.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                let trackable = if command.ends_with("_npc") {
                    active_npc.map(|index| TrackableRef {
                        kind: TRACKABLE_NPC,
                        index,
                    })
                } else {
                    self.scene.local_player.map(|t| TrackableRef {
                        kind: t.kind,
                        index: t.index,
                    })
                };
                if let Some(trackable) = trackable {
                    p.set(
                        EntityPlacement {
                            trackable,
                            offset: Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32),
                            rotation: offset_rotation,
                            follow_yaw: a[5] == 1,
                            min_distance: a[6],
                        },
                        &self.scene,
                        collision,
                    )
                    .map_err(|e| Self::trap(command, &e))?;
                }
                self.changed = true;
                unit
            }
            "cam2_getpositionentity_angleoffsets" => {
                if self.position_mode != Some(MODE_ENTITY) {
                    return Err(wrong_mode());
                }
                let Some(Position::Entity(p)) = self.position.as_ref() else {
                    return Err(Self::wrong_owner(command));
                };
                ints.push(Self::units(p.pitch()));
                ints.push(Self::units(p.yaw()));
                unit
            }
            "cam2_getpositionentity_lookatangleoffsets" => {
                let height_offset = Self::pop(ints, 1)?[0];
                let scene = self.scene.clone();
                let (p, l) = self.entity_owners(command)?;
                let centre = p.current;
                let mut sight_from = centre;
                sight_from.y += height_offset as f32;
                let lookat = Focus::Entity(l.clone())
                    .point(&scene)
                    .map_err(|e| Self::trap(command, &e))?;
                let mut direction = Vec3::diff(&lookat, &sight_from);
                direction.normalise();
                let radius = p.offset.length();
                let hits = sphere_intersections(&lookat, &direction, &centre, radius);
                let Some(first) = hits[0] else {
                    return Err(Self::trap(
                        command,
                        &Fault::InvalidState.message("no sphere intersection"),
                    ));
                };
                let far_hit = match hits[1] {
                    None => first,
                    Some(second) => {
                        if Vec3::diff(&lookat, &first).length()
                            < Vec3::diff(&lookat, &second).length()
                        {
                            second
                        } else {
                            first
                        }
                    }
                };
                let yaw = atan2_positive(centre.x - far_hit.x, centre.z - far_hit.z);
                ints.push(0);
                ints.push(Self::units(yaw));
                unit
            }
            "cam2_getpositionentity_lookatangle" => {
                let (p, l) = self.entity_owners(command)?;
                let mut delta = p.current;
                delta.sub(&l.current);
                let angle = atan2_positive(delta.x, delta.z);
                Ok(Some(Value::Int(Self::units(angle))))
            }
            "cam2_getpositionentity_lookatdistance" => {
                let (p, l) = self.entity_owners(command)?;
                let mut delta = p.current;
                delta.sub(&l.current);
                Ok(Some(Value::Int(delta.length() as i32)))
            }
            // The point commands take and return a coordinate on the object stack.
            "cam2_setlookatpoint_point" => {
                let value = objects
                    .pop()
                    .ok_or(VmError::StackUnderflow { stack: "object" })?;
                if self.lookat_mode != Some(MODE_POINT) {
                    return Err(wrong_mode());
                }
                let Some((_, coord)) = decode_coord_fine(&value) else {
                    return Err(Self::not_a_coordinate(command));
                };
                let Some(Focus::Point(point)) = self.lookat.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                point.set(coord);
                self.changed = true;
                unit
            }
            "cam2_setpositionpoint_point" => {
                let value = objects
                    .pop()
                    .ok_or(VmError::StackUnderflow { stack: "object" })?;
                if self.position_mode != Some(MODE_POINT) {
                    return Err(wrong_mode());
                }
                let Some((level, coord)) = decode_coord_fine(&value) else {
                    return Err(Self::not_a_coordinate(command));
                };
                let Some(Position::Point(point)) = self.position.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                point.set(level, coord);
                self.changed = true;
                unit
            }
            "cam2_getpositionpoint_point" => {
                if self.position_mode != Some(MODE_POINT) {
                    return Err(wrong_mode());
                }
                let Some(Position::Point(point)) = self.position.as_ref() else {
                    return Err(Self::wrong_owner(command));
                };
                if point.current.x.is_nan() {
                    return Err(Self::trap(
                        command,
                        &Fault::MissingValue.message("camera position"),
                    ));
                }
                objects.push(encode_coord_fine(
                    point.level,
                    [
                        point.current.x as i32,
                        point.current.y as i32,
                        point.current.z as i32,
                    ],
                ));
                unit
            }
            // Acceleration setters, then the position angular interpolation.
            "cam2_setlookatacceleration"
            | "cam2_setpositionacceleration"
            | "cam2_setlookatacceleration_axis"
            | "cam2_setpositionacceleration_axis" => {
                let axis = command.ends_with("_axis");
                let a = Self::pop(ints, if axis { 4 } else { 2 })?;
                let v = if axis {
                    [a[0] as f32, a[1] as f32, a[2] as f32]
                } else {
                    [a[0] as f32; 3]
                };
                let interp = *a.last().unwrap() as f32 / 1000.0;
                if !self.client_owned() || !self.acceleration_mode() {
                    return Err(refused());
                }
                if command.contains("lookat") {
                    self.lookat_acceleration = Self::infinite(v);
                    // The focus angular interpolation is not stored.
                } else {
                    self.position_acceleration = Self::infinite(v);
                    self.position_angular_interpolation = interp;
                }
                unit
            }
            // The plain forms pop two ints but use one.
            "cam2_setlookatmaxspeed"
            | "cam2_setpositionmaxspeed"
            | "cam2_setlookatmaxspeed_axis"
            | "cam2_setpositionmaxspeed_axis" => {
                let axis = command.ends_with("_axis");
                let a = Self::pop(ints, if axis { 4 } else { 2 })?;
                let v = if axis {
                    [a[0] as f32, a[1] as f32, a[2] as f32]
                } else {
                    [a[0] as f32; 3]
                };
                if !self.client_owned() || !self.acceleration_mode() {
                    return Err(refused());
                }
                if command.contains("lookat") {
                    self.lookat_max_speed = v
                } else {
                    self.position_max_speed = v
                }
                unit
            }
            // Depth planes: values below 1 fall back to the defaults.
            "cam2_setdepthplanes" => {
                let a = Self::pop(ints, 2)?;
                if !self.client_owned() {
                    return Err(refused());
                }
                let mut near = a[0] as f32;
                let mut far = a[1] as f32;
                if near < 1.0 {
                    near = 50.0;
                }
                if far < 1.0 {
                    far = 10000.0;
                }
                if near >= far {
                    return Err(refused());
                }
                self.depth_planes = [near, far];
                unit
            }
            // Field of view from angle units.
            "cam2_setfieldofview" => {
                let a = Self::pop(ints, 2)?;
                if !self.client_owned() {
                    return Err(refused());
                }
                self.fov = a[..2]
                    .iter()
                    .map(|&v| (f64::from(v) * std::f64::consts::PI * 2.0 / 16384.0) as f32)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
                unit
            }
            "cam2_setfieldofviewscreen" => {
                let a = Self::pop(ints, 3)?;
                if !self.client_owned() {
                    return Err(refused());
                }
                self.fov = std::array::from_fn(|i| {
                    ((f64::from(a[i] as f32 / 2.0 / a[2] as f32)).atan() * 2.0) as f32
                });
                unit
            }
            // Collision flags.
            "cam2_setcollisionmode" => {
                let a = Self::pop(ints, 2)?;
                if !self.client_owned() {
                    return Err(refused());
                }
                self.collision = [a[0] == 1, a[1] == 1];
                unit
            }
            // Trail distance.
            "cam2_settraildistance" => {
                let a = Self::pop(ints, 2)?;
                if !self.client_owned() || !self.acceleration_mode() {
                    return Err(refused());
                }
                self.trail = (a[0], a[1] as f32 / 1000.0);
                unit
            }
            // Linear movement mode.
            "cam2_setlinearmovementmode" => {
                let mode = Self::pop(ints, 1)?[0];
                if !(0..=2).contains(&mode) {
                    return Err(wrong_mode());
                }
                if !self.client_owned() {
                    return Err(refused());
                }
                self.linear_movement_mode = mode;
                unit
            }
            // Spring properties (spring modes only).
            "cam2_setspringproperties"
            | "cam2_setlookatspringproperties"
            | "cam2_setpositionspringproperties" => {
                let a = Self::pop(ints, 4)?;
                if !self.client_owned() || self.acceleration_mode() {
                    return Err(refused());
                }
                let v = [a[0] as f32, a[1] as f32, a[2] as f32];
                let d = a[3] as f32 / 1000.0;
                if command != "cam2_setpositionspringproperties" {
                    self.lookat_spring = v;
                    self.lookat_spring_damping = d;
                }
                if command != "cam2_setlookatspringproperties" {
                    self.position_spring = v;
                    self.position_spring_damping = d;
                }
                unit
            }
            // Angular interpolation (only the eye's is stored).
            "cam2_setlookatangularinterpolation" | "cam2_setpositionangularinterpolation" => {
                let v = Self::pop(ints, 1)?[0] as f32 / 1000.0;
                if !self.client_owned() {
                    return Err(refused());
                }
                if command.contains("position") {
                    self.position_angular_interpolation = v;
                }
                unit
            }
            // Snap distances.
            "cam2_setsnapdistances" => {
                let a = Self::pop(ints, 3)?;
                if !self.client_owned() {
                    return Err(refused());
                }
                self.snap = [a[0] as f32, a[1] as f32, a[2] as f32];
                unit
            }
            "cam2_resetsnapdistances" => {
                if !self.client_owned() {
                    return Err(refused());
                }
                self.snap = [5120.0, 10.0, 1.0];
                unit
            }
            // Effects.
            "cam2_addeffect_shake" => {
                let a = Self::pop(ints, 3)?;
                if !(0..=5).contains(&a[0]) {
                    // An unknown shake mode is a missing value.
                    return Err(Self::trap(
                        command,
                        &Fault::MissingValue.message("shake mode"),
                    ));
                }
                let magnitude = if (3..=5).contains(&a[0]) {
                    crate::trig::radians(a[1])
                } else {
                    a[1] as f32
                };
                let id = self.next_effect_id();
                self.effects.push((
                    id,
                    Effect::Shake {
                        mode: a[0],
                        magnitude,
                        duration: a[2] as f32 / 1000.0,
                        phase: 0.0,
                    },
                ));
                Ok(Some(Value::Int(id)))
            }
            "cam2_addeffect_ztilt" => {
                let a = Self::pop(ints, 1)?;
                let id = self.next_effect_id();
                self.effects
                    .push((id, Effect::Tilt(crate::trig::radians(a[0]))));
                Ok(Some(Value::Int(id)))
            }
            "cam2_updateeffect_ztilt" => {
                let a = Self::pop(ints, 2)?;
                match self.effects.iter_mut().find(|(id, _)| *id == a[0]) {
                    Some((_, Effect::Tilt(angle))) => {
                        *angle = crate::trig::radians(a[1]);
                        unit
                    }
                    _ => Err(wrong_mode()),
                }
            }
            "cam2_removeeffect" => {
                let id = Self::pop(ints, 1)?[0];
                self.effects.retain(|(e, _)| *e != id);
                unit
            }
            "cam2_removealleffects" => {
                self.effects.clear();
                unit
            }
            // Appends the null spline: the command takes it from a slot no client path
            // assigns.
            "cam2_setpositionspline_spline" => {
                if !matches!(self.position_mode, Some(2..=4)) {
                    return Err(wrong_mode());
                }
                let Some(Position::Spline(spline)) = self.position.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                spline.append(None, 0.0);
                self.changed = true;
                unit
            }
            // Look-at mode 5 (the coupled spline) passes the mode guard but is not a
            // spline owner.
            "cam2_setlookatspline_spline" => {
                if !matches!(self.lookat_mode, Some(2 | 4 | 5 | 6)) {
                    return Err(wrong_mode());
                }
                let Some(Focus::Spline(spline)) = self.lookat.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                spline.append(None, 0.0);
                self.changed = true;
                unit
            }
            // Point eye collision flag.
            "cam2_setpositionpointcollision" => {
                if self.position_mode != Some(MODE_POINT) {
                    return Err(wrong_mode());
                }
                let Some(Position::Point(point)) = self.position.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                point.collision = Self::pop(ints, 1)?[0] == 1;
                unit
            }
            // Clamp bounds around a coordinate, kept by the live owner.
            "cam2_setlookatorientation_maxdistanceclamping" => {
                let a = Self::pop(ints, 6)?;
                let coord = objects
                    .pop()
                    .ok_or(VmError::StackUnderflow { stack: "object" })?;
                if self.lookat_mode != Some(MODE_ORIENTATION) {
                    return Err(wrong_mode());
                }
                let Some(Focus::Orientation(orientation)) = self.lookat.as_mut() else {
                    return Err(Self::wrong_owner(command));
                };
                let (_, [x, y, z]) =
                    decode_coord_fine(&coord).ok_or_else(|| Self::not_a_coordinate(command))?;
                let (x, y, z) = (x as f32, -(y as f32), z as f32);
                // `-1` disables a bound; others are `centre -/+ tiles * 512`.
                let clamp = |tiles: i32, centre: f32, sign: f32| {
                    if tiles == -1 {
                        -1.0
                    } else {
                        centre + sign * (tiles * 512) as f32
                    }
                };
                orientation.clamps = [
                    clamp(a[0], x, -1.0),
                    clamp(a[1], x, 1.0),
                    clamp(a[4], y, -1.0),
                    clamp(a[5], y, 1.0),
                    clamp(a[2], z, -1.0),
                    clamp(a[3], z, 1.0),
                ];
                unit
            }
            // Every `cam2_*` command in the 910 opcode book is handled above.
            _ => Err(VmError::UnknownCommand {
                command: command.into(),
            }),
        }
    }
}
