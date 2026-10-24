//! Fine/grid coordinate helpers (`coord*_fine`, `coord_gridtofine`,
//! `getgridcoordrelativetocamera`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Operands;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;
use rs910_core::fault::Fault;

impl Engine {
    // A fine coordinate is a real object-stack value (coord_fine,
    // coord_*_fine, movecoord_fine).  The native VM's object
    // lane is string-backed, so the camera owner carries a private tagged
    // value through that lane until typed object storage is available.
    pub(super) fn cmd_coord_fine(
        &mut self,
        c: &InstructionContext<'_>,
        s: &mut Operands<'_>,
    ) -> VmResult<Option<Value>> {
        let Some(player) = self.camera.cam2.scene.local_player else {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: "local player coordinate unavailable".into(),
            });
        };
        s.objs.push(crate::ui_cam2::encode_coord_fine(
            player.level,
            player.coord,
        ));
        Ok(None)
    }

    pub(super) fn cmd_coordx_fine(
        &mut self,
        c: &InstructionContext<'_>,
        s: &mut Operands<'_>,
    ) -> VmResult<Option<Value>> {
        let value = s.obj()?;
        let Some((mut level, mut coord)) = crate::ui_cam2::decode_coord_fine(&value) else {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: Fault::WrongValueType.message("fine coordinate"),
            });
        };
        match c.command {
            "coordx_fine" => s.ints.push(coord[0]),
            "coordy_fine" => s.ints.push(coord[1]),
            "coordz_fine" => s.ints.push(coord[2]),
            "coordlevel_fine" => s.ints.push(level),
            "coord_finetogrid" => s
                .ints
                .push((level << 28) | ((coord[0] >> 9) << 14) | (coord[2] >> 9)),
            "movecoord_fine" => {
                let delta = s.int_args::<4>()?;
                level = level.wrapping_add(delta[0]);
                coord[0] = coord[0].wrapping_add(delta[1]);
                coord[1] = coord[1].wrapping_add(delta[2]);
                coord[2] = coord[2].wrapping_add(delta[3]);
                s.objs.push(crate::ui_cam2::encode_coord_fine(level, coord));
            }
            _ => unreachable!(),
        }
        Ok(None)
    }

    pub(super) fn cmd_coord_gridtofine(
        &mut self,
        _c: &InstructionContext<'_>,
        s: &mut Operands<'_>,
    ) -> VmResult<Option<Value>> {
        let args = s.int_args::<2>()?;
        let packed = args[0];
        let mut coord = [((packed >> 14) & 0x3fff) << 9, 0, (packed & 0x3fff) << 9];
        if args[0] == -1 {
            s.objs
                .push(crate::ui_cam2::encode_coord_fine(-1, [0, 0, 0]));
        } else {
            if args[1] == 1 {
                coord[0] = coord[0].wrapping_add(256);
                coord[2] = coord[2].wrapping_add(256);
            }
            s.objs
                .push(crate::ui_cam2::encode_coord_fine((packed >> 28) & 3, coord));
        }
        Ok(None)
    }

    // getgridcoordrelativetocamera pops its
    // unused operand and returns localPlayerEntity.coord().pack(). The
    // retained camera trackable already stores absolute fine coordinates.
    pub(super) fn cmd_getgridcoordrelativetocamera(
        &mut self,
        _c: &InstructionContext<'_>,
        s: &mut Operands<'_>,
    ) -> VmResult<Option<Value>> {
        s.int()?;
        let packed = self.camera.cam2.scene.local_player.map_or(-1, |player| {
            (player.level << 28) | ((player.coord[0] >> 9) << 14) | (player.coord[2] >> 9)
        });
        s.ints.push(packed);
        Ok(None)
    }
}
