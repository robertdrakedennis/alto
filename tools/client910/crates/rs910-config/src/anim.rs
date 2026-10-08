//! Rev-910 animation data: animation bases and classic frames, skeletal
//! keyframe sets, and the tick/frame-advance playback contract.
//!
//! Scope is DATA ONLY: decode + pure helpers. No wgpu, no render imports, no
//! posing of meshes here. `animation_assets`, `animation_skeletal` and
//! `gpumodel_anim` consume these records in the verified scenery renderer;
//! `model::pose_vertices`/`avatar::pose_kit` remain the legacy avatar path.
//!
//! # Archives
//!
//! Anim archives: `anims` (0, classic frames), `bases` (1, skeletons),
//! `anims.keyframes` (56, skeletal curves), plus the sequence config archive
//! (20). The RT7 anim/model archives (`animsrt7`, `modelsrt7`) are not read by
//! the 910 client.
//!
//! - Classic: group = frame-set id, file = frame index. Each frame's bytes
//!   `[1..3]` name its base id; base bytes come from the `bases` archive by
//!   file id (group 0 / file `baseId` in a single-group archive, else group
//!   `baseId` / file 0).
//! - Skeletal: one file from the `anims.keyframes` archive; bytes `[1..3]` name
//!   the base id, the base comes from `bases`, then
//!   [`decode`](KeyFrameSet::decode).
//! - Seq frame ids split as `set = frameId >>> 16`, `frame = frameId & 0xFFFF`
//!   (the low words are read first, then the high word is added shifted by
//!   16); see [`split_frame_id`].
//!
//! # Classic versus skeletal
//!
//! The 910 GPU path poses players from the CLASSIC archives (bases and frames
//! over archives 0 + 1). Archive 56 serves only seqs flagged skeletal (config
//! opcode 25). Which player seqs are skeletal is settled empirically by the
//! ignored `real_pack_stance_census` test in this module: stance/walk/run carry
//! `frameIds`, so players pose through [`crate::model::pose_vertices`]; the
//! skeletal application now lives in `animation_skeletal`.
//!
//! # Playback contract (for the wiring crew)
//!
//! Per-node tick/frame advance:
//! - Classic advance: a per-tick delay counts down first; then the ticks spent
//!   in the current frame grow by the elapsed ticks and frames turn over while
//!   that count exceeds the current frame's length (`Seq.frame_lengths`). The
//!   node tracks the current frame index and the tween target (next index,
//!   `-1` at the end).
//! - Looping: `replayoff` (`-1` = play once) rewinds the current frame index by
//!   `replayoff` at the end; `replaycount` caps replays; a node in the
//!   "play once" mode (`2`) never replays. Total duration is the sum of the
//!   frame lengths.
//! - Posing: a single frame poses with `tick` = ticks spent in the frame and
//!   `frame_len` = the current frame's length (single or tweened); a masked
//!   tween adds a per-bodypart mask; a two-node blend has classic/classic,
//!   mixed and skeletal/skeletal variants. Tweening between consecutive frames
//!   additionally needs the seq's `tweenFrames` flag.
//! - Skeletal advance: the tick count steps +1 per tick and wraps at the
//!   keyframe set's end tick, replaying from its start tick minus `replayoff`.
//! - Kit application (NOT frame advance): the wardrobe script `baseidkit` maps
//!   a bodypart id to a kit slot and swaps the identity kit; frame advance is
//!   the classic advance above.
//!
//! # Deliberate deviations from the original decoders
//!
//! 1. The original frame decoder catches every exception and yields an EMPTY
//!    frame; corrupt frames (over-long op count, trailing bytes, truncated
//!    smarts) are hard [`AnimError`]s here - an empty pose would only move the
//!    failure into the renderer.
//! 2. Keyframe-set enum lookups fall back to defaults in the original client;
//!    unknown ids are [`AnimError::Unknown`] here (same strict-input
//!    philosophy as `model.rs` deviation 2). The ignored census decodes every
//!    real keyframeset, so a false positive fails loudly there.
//! 3. The original curve decode precomputes the per-tick value table through
//!    the curve evaluator; decode here retains the keyframes and evaluates them
//!    in `animation_curve` when the skeletal resource is loaded.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use thiserror::Error;

use crate::cache::{CacheError, Pack};
use rs910_core::reader::{Eof, Reader as CoreReader};

/// Js5 archive holding classic frames (id 0).
pub const ANIMS_ARCHIVE: &str = "anims";
/// Js5 archive holding skeletons (id 1).
pub const BASES_ARCHIVE: &str = "bases";
/// Js5 archive holding skeletal curves (id 56).
pub const ANIMS_KEYFRAMES_ARCHIVE: &str = "anims.keyframes";

/// Failures animation decoding can produce. Public functions surface these as
/// `anyhow::Error`; only a CLI boundary converts further.
#[derive(Debug, Error)]
pub enum AnimError {
    /// A read ran past the end of the buffer.
    #[error("unexpected end of anim data reading {what}")]
    Truncated {
        /// What was being read (e.g. `"base skins"`, `"frame flags"`).
        what: &'static str,
    },
    /// The bytes are shaped wrong for the layout.
    #[error("invalid anim data: {0}")]
    Invalid(String),
    /// Unknown enum discriminator (strictness deviation, see module docs).
    #[error("{table} {id}: unknown {kind} {value}")]
    Unknown {
        /// Which archive object (e.g. `"keyframeset"`).
        table: &'static str,
        /// Object id under decode.
        id: u32,
        /// Which byte (e.g. `"transform type"`).
        kind: &'static str,
        /// Offending value.
        value: u8,
    },
    /// The requested file id is absent from the unpacked group.
    #[error("group {group} in archive {archive:?} has no file {file}")]
    MissingFile {
        /// Archive name, e.g. `"anims"`.
        archive: String,
        /// Requested group id (= frame-set id for classic frames).
        group: u32,
        /// Requested file id (= frame index, or base id in group 0).
        file: u32,
    },
    /// Pack IO / decompression failure.
    #[error(transparent)]
    Cache(#[from] CacheError),
}

// ---------------------------------------------------------------------------
// Byte reader (the reads live in `rs910_core::reader`; this cursor keeps
// the anim errors)
// ---------------------------------------------------------------------------

/// Byte cursor over an anim blob.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn at(data: &'a [u8], pos: usize) -> Result<Self, AnimError> {
        if pos > data.len() {
            return Err(AnimError::Truncated {
                what: "stream start",
            });
        }
        Ok(Self { data, pos })
    }

    /// One `rs910_core::reader` read at `pos` (the arithmetic
    /// lives there); `pos` follows it, a failure is `Truncated`.
    fn read<T>(
        &mut self,
        what: &'static str,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> Result<T, AnimError> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|_| AnimError::Truncated { what })
    }

    /// `g1` (unsigned byte).
    fn g1(&mut self, what: &'static str) -> Result<u8, AnimError> {
        self.read(what, CoreReader::g1)
    }

    /// `g2` (big-endian unsigned short).
    fn g2(&mut self, what: &'static str) -> Result<u32, AnimError> {
        self.read(what, CoreReader::g2).map(u32::from)
    }

    /// `g2s` (big-endian signed short).
    fn g2s(&mut self, what: &'static str) -> Result<i32, AnimError> {
        self.read(what, CoreReader::g2s).map(i32::from)
    }

    /// `gFloat` (the float with the bits of a `g4s`).
    fn gfloat(&mut self, what: &'static str) -> Result<f32, AnimError> {
        self.read(what, CoreReader::gfloat)
    }

    /// `gSmart1or2s` (signed delta smart).
    fn gsmart1or2s(&mut self, what: &'static str) -> Result<i32, AnimError> {
        self.read(what, CoreReader::gsmart1or2s)
    }

    /// `gSmart1or2` (unsigned smart).
    fn gsmart1or2(&mut self, what: &'static str) -> Result<i32, AnimError> {
        self.read(what, CoreReader::gsmart1or2)
    }

    /// Skip `count` bytes (used for the frame header base-id word, which the
    /// frame-set grouping - not the frame - owns). All or nothing.
    fn skip(&mut self, count: usize, what: &'static str) -> Result<(), AnimError> {
        self.read(what, |r| r.skip(count))
    }
}

// ---------------------------------------------------------------------------
// AnimBase — skeleton
// ---------------------------------------------------------------------------

/// One joint: parent link + the per-pose matrices.
///
/// Read as a `g2s` parent, then `matrix_count` x (16-float matrix + 3-float
/// offset). Only the skeletal path consumes these; classic posing ignores them, but they
/// must be parsed wire-exactly to reach the trailing `skin_order` table.
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    /// Parent joint index as stored (`g2s`); `-1` = root.
    pub parent: i32,
    /// Per-pose 4x4 matrices, row-major `entries[16]`.
    pub matrices: Vec<[f32; 16]>,
    /// Per-pose translation offsets (triples).
    pub offsets: Vec<[f32; 3]>,
    /// Resolved parent link: `Some(index)` when `0 <= parent < joints.len()`,
    /// else `None` (the original client leaves the link null for negative ids;
    /// out-of-range ids would throw there - `None` here, never a panic).
    pub parent_link: Option<usize>,
}

/// Skeleton binding vertex labels to op types.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimBase {
    /// Base id (the file id in `bases`, NOT a group).
    pub id: u32,
    /// Per-op transform type: 0 pivot, 1 translate, 2
    /// rotate, 3 scale, 5 alpha, 7 colour, 8/9/10 billboard. Stored value 6 is
    /// remapped to 2 on decode.
    pub op_types: Vec<u8>,
    /// Per-op opaque flag (`g1() == 1`).
    pub opaque: Vec<bool>,
    /// Per-op skin mask (`g2`; applied as `vertex_label & mask` when posing).
    pub priorities: Vec<i32>,
    /// Per-op skin label lists: `gSmart1or2` count
    /// then that many `gSmart1or2` label ids. Applied against the model's
    /// inverted label table.
    pub skins: Vec<Vec<i32>>,
    /// Joints.
    pub joints: Vec<Joint>,
    /// Trailing skin-order table (`g2s` count then `g2s` entries): the type-0
    /// joint walk order of skeletal posing.
    pub skin_order: Vec<i32>,
}

/// Decode one `AnimBase` blob (a `bases`-archive file payload).
pub fn decode_base(id: u32, data: &[u8]) -> anyhow::Result<AnimBase> {
    let mut cur = Cursor::new(data);
    // Op types, opaque flags.
    let op_count = cur.g2("base op count")? as usize;
    if op_count > data.len() {
        return Err(AnimError::Invalid(format!(
            "base {id}: op count {op_count} exceeds {}-byte blob",
            data.len()
        ))
        .into());
    }
    let mut op_types = Vec::with_capacity(op_count);
    for _ in 0..op_count {
        let mut kind = cur.g1("base op type")?;
        // A stored 6 behaves as 2.
        if kind == 6 {
            kind = 2;
        }
        op_types.push(kind);
    }
    // Opaque flags.
    let mut opaque = Vec::with_capacity(op_count);
    for _ in 0..op_count {
        opaque.push(cur.g1("base opaque flag")? == 1);
    }
    // Priorities.
    let mut priorities = Vec::with_capacity(op_count);
    for _ in 0..op_count {
        priorities.push(cur.g2("base priority")? as i32);
    }
    // TWO loops — all skin lengths first, then all
    // entries (NOT interleaved per op).
    let mut skin_lens = Vec::with_capacity(op_count);
    for _ in 0..op_count {
        let len = cur.gsmart1or2("base skin length")?;
        if len < 0 {
            return Err(
                AnimError::Invalid(format!("base {id}: negative skin length {len}")).into(),
            );
        }
        skin_lens.push(len);
    }
    let mut skins = Vec::with_capacity(op_count);
    for len in skin_lens {
        let mut labels = Vec::with_capacity(len as usize);
        for _ in 0..len {
            labels.push(cur.gsmart1or2("base skin label")?);
        }
        skins.push(labels);
    }
    // Joints (matrix count shared by all joints).
    let joint_count = cur.g2("base joint count")? as usize;
    let matrix_count = cur.g1("base matrix count")? as usize;
    if joint_count > data.len() {
        return Err(AnimError::Invalid(format!(
            "base {id}: joint count {joint_count} exceeds {}-byte blob",
            data.len()
        ))
        .into());
    }
    let mut joints = Vec::with_capacity(joint_count);
    for _ in 0..joint_count {
        let parent = cur.g2s("joint parent")?;
        let mut matrices = Vec::with_capacity(matrix_count);
        let mut offsets = Vec::with_capacity(matrix_count);
        for _ in 0..matrix_count {
            let mut mat = [0.0_f32; 16];
            for slot in mat.iter_mut() {
                *slot = cur.gfloat("joint matrix")?;
            }
            matrices.push(mat);
            let mut off = [0.0_f32; 3];
            for slot in off.iter_mut() {
                *slot = cur.gfloat("joint offset")?;
            }
            offsets.push(off);
        }
        joints.push(Joint {
            parent,
            matrices,
            offsets,
            parent_link: None,
        });
    }
    // Trailing skin-order table.
    let order_count = cur.g2s("base skin-order count")?;
    if order_count < 0 {
        return Err(AnimError::Invalid(format!(
            "base {id}: negative skin-order count {order_count}"
        ))
        .into());
    }
    let mut skin_order = Vec::with_capacity(order_count as usize);
    for _ in 0..order_count {
        skin_order.push(cur.g2s("base skin-order entry")?);
    }
    // Resolve parent links.
    for index in 0..joints.len() {
        let parent = joints[index].parent;
        joints[index].parent_link = if parent >= 0 && (parent as usize) < joints.len() {
            Some(parent as usize)
        } else {
            None
        };
    }
    Ok(AnimBase {
        id,
        op_types,
        opaque,
        priorities,
        skins,
        joints,
        skin_order,
    })
}

// ---------------------------------------------------------------------------
// AnimFrame — one classic pose
// ---------------------------------------------------------------------------

/// One posed op: the per-frame values for base op `skin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameOp {
    /// Base op index.
    pub skin: usize,
    /// X value.
    pub x: i32,
    /// Y value.
    pub y: i32,
    /// Z value.
    pub z: i32,
    /// Pivot skin: the most recent type-0 base-op index at decode time, or
    /// `-1`.
    pub pivot: i32,
    /// Blend-mode bits (`flag >>> 3 & 0x3`).
    pub blend: u8,
}

/// One decoded classic pose frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimFrame {
    /// Base id named by the frame header (`bytes[1..3]`); the caller checks it
    /// against the [`AnimBase`] it pairs this frame with (the
    /// frame-set loader groups frames by it).
    pub base_id: u32,
    /// Frame format version (first byte; `>= 2` selects the double-smart
    /// type-7 reads).
    pub version: u8,
    /// Posed ops, in base-op order.
    pub ops: Vec<FrameOp>,
    /// A type-5 (alpha) op is present (render flag `0x100`).
    pub has_alpha_op: bool,
    /// A type-7 (colour) op is present (render flag `0x80`).
    pub has_colour_op: bool,
    /// A type-8/9/10 (billboard) op is present (render flag `0x400`).
    pub has_billboard_op: bool,
}

/// Decode one classic frame blob (an `anims`-archive file payload) against its base.
///
/// `id` is the frame index (file id) and names errors; `base_id` is the base
/// id the loader read from the frame header (`bytes[1..3]`) and is stored
/// unchecked on the frame (pairing is the loader's job).
pub fn decode_frame(
    id: u32,
    base_id: u32,
    base: &AnimBase,
    data: &[u8],
) -> anyhow::Result<AnimFrame> {
    let mut head = Cursor::new(data);
    // Version byte, then the 2-byte base id (owned by the frame-set grouping -
    // skipped, stored by the caller as `base_id`).
    let version = head.g1("frame version")?;
    head.skip(2, "frame base id")?;
    // Op count. Bounds-checked against the base (the original client would
    // index out of range, catch it and yield an empty frame; we name it).
    let count = head.g2("frame op count")? as usize;
    if count > base.op_types.len() {
        return Err(AnimError::Invalid(format!(
            "frame {id}: op count {count} exceeds base {} op count {}",
            base.id,
            base.op_types.len()
        ))
        .into());
    }
    // The value stream starts past ALL flag bytes.
    let mut flags = Cursor::at(data, head.pos)?;
    let mut values = Cursor::at(
        data,
        head.pos.checked_add(count).ok_or_else(|| {
            AnimError::Invalid(format!("frame {id}: op count {count} overflowed"))
        })?,
    )?;
    for _ in 0..count {
        flags.g1("frame flags")?;
    }
    let mut ops = Vec::new();
    let mut has_alpha_op = false;
    let mut has_colour_op = false;
    let mut has_billboard_op = false;
    // Last type-0 op index + last pivot target.
    let mut last_zero: i32 = -1;
    let mut last_pivot: i32 = -1;
    let mut flag_cur = Cursor::at(data, head.pos)?;
    for index in 0..count {
        let Some(kind) = base.op_types.get(index).copied() else {
            return Err(AnimError::Invalid(format!(
                "frame {id}: op {index} outside base {} ({} ops)",
                base.id,
                base.op_types.len()
            ))
            .into());
        };
        if kind == 0 {
            last_zero = index as i32;
        }
        let flag = flag_cur.g1("frame flags")?;
        if flag == 0 {
            continue;
        }
        // An explicit type-zero transform already
        // establishes this pivot. Do not insert another before its children.
        if kind == 0 {
            last_pivot = index as i32;
        }
        // Unposed default (128 for scale-ish 3/10).
        let mut x = 0_i32;
        let mut y = 0_i32;
        let mut z = 0_i32;
        if kind == 3 || kind == 10 {
            x = 128;
            y = 128;
            z = 128;
        }
        // Per-axis smarts; v2 type-7 frames carry a
        // second (discarded) smart per set axis.
        if version >= 2 && kind == 7 {
            if flag & 0x1 == 0 {
                x = 0;
            } else {
                x = i32::from(values.gsmart1or2s("frame x")? as i16);
                values.gsmart1or2s("frame x stride")?;
            }
            if flag & 0x2 == 0 {
                y = 0;
            } else {
                y = i32::from(values.gsmart1or2s("frame y")? as i16);
                values.gsmart1or2s("frame y stride")?;
            }
            if flag & 0x4 == 0 {
                z = 0;
            } else {
                z = i32::from(values.gsmart1or2s("frame z")? as i16);
                values.gsmart1or2s("frame z stride")?;
            }
        } else {
            if flag & 0x1 == 0 {
                // Keep the default.
            } else {
                x = i32::from(values.gsmart1or2s("frame x")? as i16);
            }
            if flag & 0x2 == 0 {
                // Keep the default.
            } else {
                y = i32::from(values.gsmart1or2s("frame y")? as i16);
            }
            if flag & 0x4 == 0 {
                // Keep the default.
            } else {
                z = i32::from(values.gsmart1or2s("frame z")? as i16);
            }
        }
        let blend = (flag >> 3) & 0x3;
        // Rotation-ish 2/9 values are <<2 & 0x3FFF.
        if kind == 2 || kind == 9 {
            x = (x << 2) & 0x3FFF;
            y = (y << 2) & 0x3FFF;
            z = (z << 2) & 0x3FFF;
        }
        // Pivot + render-flag classes.
        let mut pivot = -1_i32;
        if kind == 1 || kind == 2 || kind == 3 {
            if last_zero > last_pivot {
                pivot = last_zero;
                last_pivot = last_zero;
            }
        } else if kind == 5 {
            has_alpha_op = true;
        } else if kind == 7 {
            has_colour_op = true;
        } else if kind == 9 || kind == 10 || kind == 8 {
            has_billboard_op = true;
        }
        ops.push(FrameOp {
            skin: index,
            x,
            y,
            z,
            pivot,
            blend,
        });
    }
    // The value stream must end exactly at the blob end (the original client
    // throws and yields an empty frame; we name it).
    if values.pos != data.len() {
        return Err(AnimError::Invalid(format!(
            "frame {id}: {} trailing bytes after value stream",
            data.len() - values.pos
        ))
        .into());
    }
    Ok(AnimFrame {
        base_id,
        version,
        ops,
        has_alpha_op,
        has_colour_op,
        has_billboard_op,
    })
}

// ---------------------------------------------------------------------------
// KeyFrameSet — skeletal curves, decode-only
// ---------------------------------------------------------------------------

/// Slot counts per transform type (ids 0-4 → slots 0/3/3/6/1).
fn transform_slots(type_id: u8) -> Option<usize> {
    match type_id {
        0 => Some(0),
        1 => Some(3),
        2 => Some(3),
        3 => Some(6),
        4 => Some(1),
        _ => None,
    }
}

/// Component slot within its transform (ids 0-16 map to the slots below;
/// anything else is unknown).
fn component_slot(component_id: u8) -> Option<usize> {
    match component_id {
        0 => Some(0),
        1 | 4 | 7 | 10 => Some(0),
        2 | 5 | 8 | 11 | 13 => Some(1),
        3 | 6 | 9 | 12 => Some(2),
        14 => Some(4),
        15 => Some(5),
        16 => Some(0),
        _ => None,
    }
}

/// One keyframe: time + value + bezier tangents (the keyframe layout does not
/// depend on the set version).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyFrame {
    /// Time (`g2s`).
    pub time: i32,
    /// Value (`gFloat`).
    pub value: f32,
    /// Incoming tangent (`tanInX/Y`, `gFloat` pair).
    pub tan_in: [f32; 2],
    /// Outgoing tangent (`tanOutX/Y`, `gFloat` pair).
    pub tan_out: [f32; 2],
}

/// One curve over a single component (decode half).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// Curve-type byte (the original client falls back on unknown values;
    /// retained raw here and evaluated by `animation_curve::EvaluatedCurve`).
    pub curve_type: u8,
    /// Pre-infinity behaviour id (transform infinity type, 0-4).
    pub pre: u8,
    /// Post-infinity behaviour id (transform infinity type, 0-4).
    pub post: u8,
    /// Bezier interpolation flag.
    pub bezier: bool,
    /// Keyframes (`g2` count, then time + 5 floats each).
    pub keyframes: Vec<KeyFrame>,
}

/// Skeletal animation: decoded curves bound to a base.
///
/// DECODE-ONLY: per-tick evaluation (`animation_curve`) and application need
/// the float joint-matrix chain supplied by `animation_skeletal`. The decoder
/// retains the original curves so their integer-time values can be checked
/// against the reference client.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyFrameSet {
    /// Keyframeset id (= `fetchFile` id in `anims.keyframes`).
    pub id: u32,
    /// Format version (first header byte).
    pub version: u8,
    /// Base id (header `g2`).
    pub base_id: u32,
    /// Resolved base (from `bases`).
    pub base: AnimBase,
    /// Start tick (`g2`).
    pub start: u16,
    /// End tick (`g2`).
    pub end: u16,
    /// Joint pose index (`g1`); historical field name.
    pub loop_point: u8,
    /// Curves per base op: `curves[op]` is
    /// `None` when the op has no curves, else the `transform_slots`-sized
    /// array with `Some` at each decoded component slot.
    pub curves: Vec<Option<Vec<Option<Curve>>>>,
    /// Render flags from transform types 2/3/4.
    pub render_flags: i32,
}

/// Decode one keyframeset body (everything after the 3-byte
/// version+base-id header) against its base.
pub fn decode_keyframeset_body(
    id: u32,
    base: &AnimBase,
    version: u8,
    data: &[u8],
) -> anyhow::Result<KeyFrameSet> {
    let _ = version;
    let mut cur = Cursor::new(data);
    let start = cur.g2("keyframeset start")? as u16;
    let end = cur.g2("keyframeset end")? as u16;
    let loop_point = cur.g1("keyframeset loop point")?;
    let entry_count = cur.g2("keyframeset entry count")? as usize;
    let mut render_flags = 0;
    let mut curves: Vec<Option<Vec<Option<Curve>>>> = vec![None; base.op_types.len()];
    for _ in 0..entry_count {
        // Transform type of this entry.
        let type_byte = cur.g1("keyframeset transform type")?;
        render_flags |= match type_byte {
            2 => 0x400,
            3 => 0x80,
            4 => 0x100,
            _ => 0,
        };
        let Some(slots) = transform_slots(type_byte) else {
            return Err(AnimError::Unknown {
                table: "keyframeset",
                id,
                kind: "transform type",
                value: type_byte,
            }
            .into());
        };
        let skin = cur.gsmart1or2s("keyframeset skin")?;
        let component_byte = cur.g1("keyframeset component")?;
        let Some(slot) = component_slot(component_byte) else {
            return Err(AnimError::Unknown {
                table: "keyframeset",
                id,
                kind: "transform component",
                value: component_byte,
            }
            .into());
        };
        // Curve: count, curve type, pre, post,
        // bezier flag, then keyframes.
        let keyframe_count = cur.g2("curve keyframe count")? as usize;
        let curve_type = cur.g1("curve type")?;
        let pre = cur.g1("curve pre-infinity")?;
        if pre > 4 {
            return Err(AnimError::Unknown {
                table: "keyframeset",
                id,
                kind: "infinity type",
                value: pre,
            }
            .into());
        }
        let post = cur.g1("curve post-infinity")?;
        if post > 4 {
            return Err(AnimError::Unknown {
                table: "keyframeset",
                id,
                kind: "infinity type",
                value: post,
            }
            .into());
        }
        let bezier = cur.g1("curve bezier flag")? != 0;
        let mut keyframes = Vec::with_capacity(keyframe_count);
        for _ in 0..keyframe_count {
            // One keyframe.
            let time = cur.g2s("keyframe time")?;
            let value = cur.gfloat("keyframe value")?;
            let tan_in = [
                cur.gfloat("keyframe tanInX")?,
                cur.gfloat("keyframe tanInY")?,
            ];
            let tan_out = [
                cur.gfloat("keyframe tanOutX")?,
                cur.gfloat("keyframe tanOutY")?,
            ];
            keyframes.push(KeyFrame {
                time,
                value,
                tan_in,
                tan_out,
            });
        }
        // Per-op slot array, sized by the
        // transform-type slot count.
        if skin < 0 || (skin as usize) >= curves.len() {
            return Err(AnimError::Invalid(format!(
                "keyframeset {id}: skin {skin} outside base {} ({} ops)",
                base.id,
                base.op_types.len()
            ))
            .into());
        }
        if slot >= slots {
            return Err(AnimError::Invalid(format!(
                "keyframeset {id}: component {component_byte} (slot {slot}) outside \
                 transform type {type_byte} ({slots} slots)"
            ))
            .into());
        }
        let entry = curves[skin as usize].get_or_insert_with(|| vec![None; slots]);
        if entry.len() != slots {
            return Err(AnimError::Invalid(format!(
                "keyframeset {id}: op {skin} mixes transform types ({} vs {slots} slots)",
                entry.len()
            ))
            .into());
        }
        entry[slot] = Some(Curve {
            curve_type,
            pre,
            post,
            bezier,
            keyframes,
        });
    }
    Ok(KeyFrameSet {
        id,
        version,
        base_id: base.id,
        base: base.clone(),
        start,
        end,
        loop_point,
        curves,
        render_flags,
    })
}

// ---------------------------------------------------------------------------
// Frame ids + pack loaders
// ---------------------------------------------------------------------------

/// Split a seq frame id into `(set_id, frame_index)` (the seq decoder reads the
/// low words, then adds the high word shifted by 16; the set is `>>> 16`, the
/// frame `& 0xFFFF`).
#[must_use]
pub fn split_frame_id(frame_id: u32) -> (u32, u32) {
    (frame_id >> 16, frame_id & 0xFFFF)
}

/// Read one file by id with the archive's file addressing: a
/// single-group archive stores everything as group 0 / file `id`, otherwise
/// group `id` / file 0. Returns the raw file bytes.
pub fn fetch_file(pack: &Pack, archive: &str, id: u32) -> anyhow::Result<Vec<u8>> {
    let index = pack.read_archive_index(archive)?;
    // Single-group archives (bases, anims.keyframes on this pack) keep the
    // file under group 0; multi-group archives address group `id`, file 0.
    if index.group_id.len() == 1 {
        let group = index.group_id.first().copied().unwrap_or(0);
        let files = pack
            .read_group(archive, group)
            .with_context(|| format!("{archive} group {group}"))?;
        files.get(&id).cloned().ok_or_else(|| {
            AnimError::MissingFile {
                archive: archive.to_string(),
                group,
                file: id,
            }
            .into()
        })
    } else {
        let files = pack
            .read_group(archive, id)
            .with_context(|| format!("{archive} group {id}"))?;
        files.get(&0).cloned().ok_or_else(|| {
            AnimError::MissingFile {
                archive: archive.to_string(),
                group: id,
                file: 0,
            }
            .into()
        })
    }
}

/// Load one skeleton from the `bases` archive by base id.
pub fn load_base(pack: &Pack, base_id: u32) -> anyhow::Result<AnimBase> {
    let bytes = fetch_file(pack, BASES_ARCHIVE, base_id)
        .with_context(|| format!("{BASES_ARCHIVE} base {base_id}"))?;
    decode_base(base_id, &bytes)
}

/// Read the base id out of a classic frame header (`bytes[1..3]`).
pub fn frame_base_id(data: &[u8]) -> anyhow::Result<u32> {
    if data.len() < 3 {
        return Err(AnimError::Truncated {
            what: "frame header",
        }
        .into());
    }
    Ok((u32::from(data[1]) << 8) | u32::from(data[2]))
}

/// One decoded frame set: every frame in the group plus the distinct bases
/// they name.
#[derive(Clone, Debug)]
pub struct FrameSetData {
    /// Frame-set id (= group id in `anims`).
    #[allow(
        dead_code,
        reason = "decoded group id retained with the frame set; no reader yet"
    )]
    pub set_id: u32,
    /// Distinct bases by base id.
    pub bases: BTreeMap<u32, AnimBase>,
    /// Frames by frame index: `(base_id, frame)`.
    pub frames: BTreeMap<u32, (u32, AnimFrame)>,
}

impl FrameSetData {
    /// Look up one frame with its base already paired.
    pub fn frame(&self, index: u32) -> Option<(&AnimBase, &AnimFrame)> {
        let (base_id, frame) = self.frames.get(&index)?;
        Some((self.bases.get(base_id)?, frame))
    }
}

/// Load every frame in one `anims` group with the distinct bases they name.
pub fn load_frameset(pack: &Pack, set_id: u32) -> anyhow::Result<FrameSetData> {
    let files = pack
        .read_group(ANIMS_ARCHIVE, set_id)
        .with_context(|| format!("{ANIMS_ARCHIVE} group {set_id}"))?;
    let mut out = FrameSetData {
        set_id,
        bases: BTreeMap::new(),
        frames: BTreeMap::new(),
    };
    for (file_id, bytes) in &files {
        let base_id = frame_base_id(bytes)?;
        if let std::collections::btree_map::Entry::Vacant(slot) = out.bases.entry(base_id) {
            let base = load_base(pack, base_id)?;
            slot.insert(base);
        }
        let base = out.bases.get(&base_id).ok_or_else(|| {
            AnimError::Invalid(format!(
                "frameset {set_id}: base {base_id} missing after load"
            ))
        })?;
        let frame = decode_frame(*file_id, base_id, base, bytes)?;
        out.frames.insert(*file_id, (base_id, frame));
    }
    Ok(out)
}

/// Load one skeletal set from `anims.keyframes`: the file by id, header
/// version + base id, base from `bases`, then the curve table.
pub fn load_keyframeset(pack: &Pack, id: u32) -> anyhow::Result<KeyFrameSet> {
    let bytes = fetch_file(pack, ANIMS_KEYFRAMES_ARCHIVE, id)
        .with_context(|| format!("{ANIMS_KEYFRAMES_ARCHIVE} keyframeset {id}"))?;
    if bytes.len() < 3 {
        return Err(AnimError::Truncated {
            what: "keyframeset header",
        }
        .into());
    }
    let version = bytes[0];
    let base_id = (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
    let base = load_base(pack, base_id)?;
    decode_keyframeset_body(id, &base, version, &bytes[3..])
}

#[cfg(test)]
mod tests;
