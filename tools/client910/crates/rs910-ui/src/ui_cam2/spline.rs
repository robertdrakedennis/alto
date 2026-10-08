//! Camera spline decoding, sampling and progression.

use rs910_core::fault::Fault;

pub use rs910_core::vector_math::*;

pub(super) fn read_float(reader: &mut crate::ui_bytes::Cursor<'_>) -> anyhow::Result<f32> {
    Ok(f32::from_bits(reader.g4s()? as u32))
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CameraSplinePath {
    controls: Vec<[[f32; 3]; 4]>,
    lengths: Vec<Vec<f32>>,
    total_length: f32,
}

impl CameraSplinePath {
    pub(super) fn new(mut controls: Vec<[[f32; 3]; 4]>) -> Self {
        if controls.is_empty() {
            controls.push([[0.0; 3]; 4]);
        }
        let mut lengths = Vec::with_capacity(controls.len());
        let mut total_length = 0.0;
        for segment in &controls {
            let mut previous = Self::evaluate(segment, 0.0);
            let mut length = 0.0;
            for step in 1..=20 {
                let current = Self::evaluate(segment, step as f32 / 20.0);
                let dx = current[0] - previous[0];
                let dy = current[1] - previous[1];
                let dz = current[2] - previous[2];
                length += (dx * dx + dy * dy + dz * dz).sqrt();
                previous = current;
            }
            let count = (length / 20.0).max(1.0) as usize;
            let mut table = Vec::with_capacity(count);
            let mut previous = Self::evaluate(segment, 0.0);
            for step in 1..=count {
                let current = Self::evaluate(segment, step as f32 / count as f32);
                let dx = current[0] - previous[0];
                let dy = current[1] - previous[1];
                let dz = current[2] - previous[2];
                table.push((dx * dx + dy * dy + dz * dz).sqrt());
                previous = current;
            }
            let segment_length = table.iter().sum::<f32>();
            total_length += segment_length;
            lengths.push(table);
        }
        Self {
            controls,
            lengths,
            total_length,
        }
    }

    pub(super) fn evaluate(control: &[[f32; 3]; 4], t: f32) -> [f32; 3] {
        let t2 = t * t;
        let t3 = t2 * t;
        let mut out = [0.0; 3];
        for axis in 0..3 {
            let a = (control[1][axis] - control[0][axis]) * 3.0;
            let b = (control[2][axis] - control[1][axis]) * 3.0 - a;
            let c = control[3][axis] - control[0][axis] - a - b;
            out[axis] = t * a + t2 * b + t3 * c + control[0][axis];
        }
        out
    }

    pub(super) fn sample_segment(&self, position: f32) -> Vec3 {
        let mut segment = position.floor() as usize;
        let mut t = position - segment as f32;
        if segment >= self.controls.len() {
            segment = self.controls.len() - 1;
            t = 1.0;
        }
        let point = Self::evaluate(&self.controls[segment], t.clamp(0.0, 1.0));
        Vec3::new(point[0], point[1], point[2])
    }

    pub(super) fn sample_arc(&self, distance: f32) -> Vec3 {
        let mut remaining = distance.clamp(0.0, self.total_length);
        for (index, table) in self.lengths.iter().enumerate() {
            let segment_length = table.iter().sum::<f32>();
            if remaining <= segment_length || index + 1 == self.lengths.len() {
                let mut accumulated = 0.0;
                for (step, length) in table.iter().enumerate() {
                    if remaining <= accumulated + *length || step + 1 == table.len() {
                        let fraction = if *length > 0.0 {
                            (remaining - accumulated) / *length
                        } else {
                            0.0
                        };
                        let t = (step as f32 + fraction) / table.len() as f32;
                        let point = Self::evaluate(&self.controls[index], t.clamp(0.0, 1.0));
                        return Vec3::new(point[0], point[1], point[2]);
                    }
                    accumulated += *length;
                }
            }
            remaining -= segment_length;
        }
        self.sample_segment(self.controls.len() as f32)
    }

    pub(super) fn decode(reader: &mut crate::ui_bytes::Cursor<'_>) -> anyhow::Result<Self> {
        let segment_count = reader.gsmart1or2()?.max(1) as usize;
        let mut controls = Vec::with_capacity(segment_count);
        let first0 = [
            read_float(reader)?,
            read_float(reader)?,
            read_float(reader)?,
        ];
        let first1 = [
            read_float(reader)?,
            read_float(reader)?,
            read_float(reader)?,
        ];
        let _first_segment_duration = read_float(reader)?;
        let first3 = [
            read_float(reader)?,
            read_float(reader)?,
            read_float(reader)?,
        ];
        let first_mirror = [
            read_float(reader)?,
            read_float(reader)?,
            read_float(reader)?,
        ];
        let first2 = [
            first3[0] * 2.0 - first_mirror[0],
            first3[1] * 2.0 - first_mirror[1],
            first3[2] * 2.0 - first_mirror[2],
        ];
        let _first_arc_duration = read_float(reader)?;
        controls.push([first0, first1, first2, first3]);
        let mut previous3 = first3;
        for _ in 1..segment_count {
            let next3 = [
                read_float(reader)?,
                read_float(reader)?,
                read_float(reader)?,
            ];
            let next_mirror = [
                read_float(reader)?,
                read_float(reader)?,
                read_float(reader)?,
            ];
            let next2 = [
                next3[0] * 2.0 - next_mirror[0],
                next3[1] * 2.0 - next_mirror[1],
                next3[2] * 2.0 - next_mirror[2],
            ];
            let _arc_duration = read_float(reader)?;
            let previous2 = controls.last().unwrap()[2];
            controls.push([
                previous3,
                [
                    previous3[0] * 2.0 - previous2[0],
                    previous3[1] * 2.0 - previous2[1],
                    previous3[2] * 2.0 - previous2[2],
                ],
                next2,
                next3,
            ]);
            previous3 = next3;
        }
        Ok(Self::new(controls))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CameraSplineKind {
    Accelerated,
    Linear,
    Timed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplineTrack {
    kind: CameraSplineKind,
    /// The splines in play order. `None` is the null spline that
    /// `cam2_set*spline_spline` appends: the commands take it from a slot no
    /// client path assigns, and sampling it is a missing-value fault.
    paths: Vec<Option<CameraSplinePath>>,
    waits: Vec<f32>,
    starts: Vec<f32>,
    ends: Vec<f32>,
    scales: Vec<f32>,
    path_index: usize,
    pub(super) position: f32,
    speed: f32,
}

impl SplineTrack {
    pub(super) fn new(kind: CameraSplineKind) -> Self {
        Self {
            kind,
            paths: Vec::new(),
            waits: Vec::new(),
            starts: Vec::new(),
            ends: Vec::new(),
            scales: Vec::new(),
            path_index: 0,
            position: 0.0,
            speed: 0.0,
        }
    }

    pub(super) fn decode(
        &mut self,
        reader: &mut crate::ui_bytes::Cursor<'_>,
    ) -> anyhow::Result<()> {
        let count = reader.g1()? as usize;
        self.paths.clear();
        self.waits.clear();
        for _ in 0..count {
            self.paths.push(Some(CameraSplinePath::decode(reader)?));
            self.waits.push(read_float(reader)?);
        }
        self.starts.clear();
        self.ends.clear();
        self.scales.clear();
        match self.kind {
            CameraSplineKind::Accelerated => {}
            CameraSplineKind::Linear => {
                for _ in 0..count {
                    self.starts.push(read_float(reader)?);
                    self.ends.push(read_float(reader)?);
                }
            }
            CameraSplineKind::Timed => {
                for _ in 0..count {
                    self.starts.push(read_float(reader)?);
                    self.ends.push(read_float(reader)?);
                    self.scales.push(read_float(reader)?);
                }
            }
        }
        self.path_index = 0;
        self.position = 0.0;
        self.speed = 0.0;
        Ok(())
    }

    pub(super) fn initialised(&self) -> bool {
        !self.paths.is_empty()
    }

    /// The raw parameter the coupled look-at spline samples, including the
    /// mode-specific clock.
    pub(super) fn parameter(&self) -> f32 {
        self.position
    }

    /// Appends one spline with its wait time; the segment id is not read
    /// anywhere.
    pub(super) fn append(&mut self, path: Option<CameraSplinePath>, wait: f32) {
        self.paths.push(path);
        self.waits.push(wait);
    }

    pub(super) fn point(&self) -> Result<Vec3, String> {
        let Some(slot) = self.paths.get(self.path_index) else {
            return Ok(Vec3::NAN);
        };
        let path = slot
            .as_ref()
            .ok_or_else(|| Fault::MissingValue.message("camera spline slot"))?;
        Ok(match self.kind {
            CameraSplineKind::Linear => path.sample_segment(self.position),
            CameraSplineKind::Accelerated | CameraSplineKind::Timed => {
                path.sample_arc(self.position)
            }
        })
    }

    pub(super) fn update(
        &mut self,
        dt: f32,
        max_speed: [f32; 3],
        acceleration: [f32; 3],
    ) -> Result<(), String> {
        let Some(slot) = self.paths.get(self.path_index) else {
            return Ok(());
        };
        let path = slot
            .as_ref()
            .ok_or_else(|| Fault::MissingValue.message("camera spline slot"))?;
        if let Some(wait) = self.waits.get_mut(self.path_index) {
            if *wait > 0.0 {
                *wait = (*wait - dt).max(0.0);
                if *wait > 0.0 {
                    return Ok(());
                }
            }
        }
        let limit = match self.kind {
            CameraSplineKind::Linear => path.controls.len() as f32,
            CameraSplineKind::Accelerated | CameraSplineKind::Timed => path.total_length,
        };
        let step = match self.kind {
            CameraSplineKind::Accelerated => {
                let acceleration = Vec3::from_array(acceleration).length();
                let max_speed = Vec3::from_array(max_speed).length();
                if acceleration.is_infinite() {
                    self.speed = max_speed;
                } else {
                    let remaining = (limit - self.position).max(0.0);
                    if self.speed * 0.5 * (self.speed / acceleration.max(f32::EPSILON)) > remaining
                    {
                        self.speed = (self.speed - acceleration * dt).max(0.0);
                    } else {
                        self.speed = (self.speed + acceleration * dt).min(max_speed);
                    }
                }
                self.speed * dt
            }
            CameraSplineKind::Linear => {
                let length = path.controls.len() as f32;
                let t = if length > 0.0 {
                    self.position / length
                } else {
                    1.0
                };
                (self.ends.get(self.path_index).copied().unwrap_or(0.0)
                    - self.starts.get(self.path_index).copied().unwrap_or(0.0))
                    * t
                    + self.starts.get(self.path_index).copied().unwrap_or(0.0)
            }
            CameraSplineKind::Timed => {
                let t = if limit > 0.0 {
                    (self.position / limit).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let start = self.starts.get(self.path_index).copied().unwrap_or(0.0);
                let end = self.ends.get(self.path_index).copied().unwrap_or(0.0);
                let scale = self
                    .scales
                    .get(self.path_index)
                    .copied()
                    .unwrap_or(1.0)
                    .max(f32::EPSILON);
                limit / scale * ((end - start) * t + start) * dt
            }
        };
        self.position += step.max(0.0);
        if self.position >= limit {
            self.position = 0.0;
            if self.path_index + 1 < self.paths.len() {
                self.path_index += 1;
                self.speed = 0.0;
            } else {
                self.position = limit;
            }
        }
        Ok(())
    }
}
