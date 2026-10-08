//! Ordered transport adapter for the live packet read.
//! SERVER_TICK_END ends a read batch; it is NOT a logic-clock tick.
//! The caller supplies the actual logic cycle/config/map context for each packet.
//! TODO(#gap-G-live-context): production config, rebuild and logic-clock adapters.
//! A blocked front packet stays owned here; later deltas cannot pass it.
use super::{npc, rebuild_state, zone_state, Error, Packet, PlayerContext, Players, Result};
use crate::proto as wire;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub opcode: u8,
    pub payload: Vec<u8>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub players: Players,
    pub npcs: npc::Npcs,
    pub zones: zone_state::State,
    pub initialized: bool,
    pub world: Option<rebuild_state::World>,
    pub varps: Option<crate::entities910::varps::Varps>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feed {
    pub state: State,
    pending: VecDeque<Frame>,
    completed: VecDeque<(u8, Applied)>,
    pub blocked: Option<Error>,
}
impl Feed {
    /// Forget the frames read from a connection that is gone and not yet
    /// applied (a resumed session starts from the new connection's frames).
    pub fn discard_pending(&mut self) {
        self.pending.clear();
        self.completed.clear();
        self.blocked = None;
    }
}
pub struct Contexts<'a> {
    pub rebuild: Option<&'a rebuild_state::Config<'a>>,
    pub player: Option<&'a PlayerContext<'a>>,
    pub npc: Option<&'a npc::NpcContext<'a>>,
    pub zone: Option<&'a zone_state::Config<'a>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub bytes: usize,
    pub bit_pos: Option<usize>,
    pub random_used: usize,
    pub refresh: Vec<(i32, i32, i32)>,
    pub read_batch_end: bool,
    pub rebuild: Option<(rebuild_state::World, rebuild_state::Effects)>,
    pub varp: Option<VarpReceipt>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VarpReceipt {
    pub clock_reads: usize,
    pub ignored_overflow: bool,
    pub transmit_increment: i32,
}
impl Applied {
    fn bytes(bytes: usize) -> Self {
        Self {
            bytes,
            bit_pos: None,
            random_used: 0,
            refresh: vec![],
            read_batch_end: false,
            rebuild: None,
            varp: None,
        }
    }
}
/// Includes unsupported entity-specific operations so they cannot fall through
/// the older transport's log-and-drop branch. It covers every server and zone
/// opcode of the protocol tables plus the standalone dispatch cases.
pub fn handles(op: u8) -> bool {
    use wire::server::*;
    matches!(
        op,
        REBUILD_NORMAL
            | REBUILD_REGION
            | RESET_ANIMS
            | NPC_ANIM_SPECIFIC
            | NPC_HEADICON_SPECIFIC
            | RESET_CLIENT_VARCACHE
            | PLAYER_INFO
            | NPC_INFO
            | SERVER_TICK_END
            | UPDATE_ZONE_PARTIAL_FOLLOWS
            | UPDATE_ZONE_FULL_FOLLOWS
            | UPDATE_ZONE_PARTIAL_ENCLOSED
            | PROJANIM_SPECIFIC
            | SPOTANIM_SPECIFIC
            | LOC_ANIM_SPECIFIC
    ) || super::varp::handles(op)
        || zone_input(op).is_some()
}
fn zone_input(op: u8) -> Option<zone_state::Input> {
    use wire::server::*;
    use zone_state::Input::*;
    Some(match op {
        UPDATE_ZONE_PARTIAL_FOLLOWS => PartialFollows,
        UPDATE_ZONE_FULL_FOLLOWS => FullFollows,
        UPDATE_ZONE_PARTIAL_ENCLOSED => Enclosed,
        LOC_PREFETCH => Atom(0),
        MAP_ANIM => Atom(1),
        TEXT_COORD => Atom(2),
        OBJ_COUNT => Atom(3),
        LOC_ANIM => Atom(4),
        LOC_DEL => Atom(5),
        MAP_PROJANIM => Atom(7),
        OBJ_REVEAL => Atom(8),
        OBJ_DEL => Atom(9),
        LOC_ADD_CHANGE => Atom(10),
        LOC_CUSTOMISE => Atom(11),
        MAP_PROJANIM_HALFSQ => Atom(12),
        OBJ_ADD => Atom(13),
        SOUND_AREA => Atom(14),
        _ => return None,
    })
}
/// What one decoded packet changes in `State`. Decoders either build only the
/// sub-state they replace, or (PLAYER_INFO, NPC_INFO) edit it in place with an
/// undo log. A rejected packet therefore leaves `State` untouched without a
/// staged copy of the whole world; the client itself logs out on a decode
/// exception, so no partial state is ever observed.
enum Change {
    None,
    State(Box<State>),
    Players(Box<super::PlayersUndo>),
    Npcs(npc::Undo),
    Zones(Box<zone_state::State>),
    Transients(crate::entities910::transient::Transients),
    ResetAnims,
    ResetVarcache,
    Npc(usize, Box<crate::entities910::Npc>),
    NpcHeadIcon {
        index: usize,
        slot: usize,
        group: i32,
        icon: i16,
    },
    NpcSpot(usize, usize, crate::entities910::animation_state::Spot),
    PlayerSpot(usize, usize, crate::entities910::animation_state::Spot),
    TileSpot(i64, Option<crate::entities910::transient::Spot>),
}
impl Change {
    fn commit(self, state: &mut State) {
        match self {
            Self::None | Self::Players(_) | Self::Npcs(_) => {}
            Self::State(next) => *state = *next,
            Self::Zones(zones) => state.zones = *zones,
            Self::Transients(transients) => state.zones.transients = transients,
            Self::ResetAnims => {
                for player in state.players.players.iter_mut().flatten() {
                    player.animation.reset_main();
                }
                for npc in state.npcs.entities.values_mut() {
                    npc.path.animation.reset_main();
                }
            }
            Self::ResetVarcache => {
                if let Some(varps) = state.varps.as_mut() {
                    varps.reset();
                }
            }
            Self::Npc(index, npc) => {
                state.npcs.entities.insert(index, *npc);
            }
            Self::NpcHeadIcon {
                index,
                slot,
                group,
                icon,
            } => {
                if let Some(npc) = state.npcs.entities.get_mut(&index) {
                    let icons = npc.head_icons.get_or_insert(([-1; 8], [-1; 8]));
                    if slot < icons.0.len() {
                        icons.0[slot] = group;
                        icons.1[slot] = icon;
                    }
                }
            }
            Self::NpcSpot(index, slot, spot) => {
                if let Some(npc) = state.npcs.entities.get_mut(&index) {
                    npc.path.animation.spots[slot] = spot;
                }
            }
            Self::PlayerSpot(index, slot, spot) => {
                if let Some(player) = state
                    .players
                    .players
                    .get_mut(index)
                    .and_then(Option::as_mut)
                {
                    player.animation.spots[slot] = spot;
                }
            }
            Self::TileSpot(key, spot) => {
                let spots = &mut state.zones.transients.spots;
                spots.retain(|spot| spot.key != key);
                spots.extend(spot);
            }
        }
    }
    /// Undo an in-place decode the owner rejected after decoding.
    fn abort(self, state: &mut State) {
        match self {
            Self::Players(undo) => undo.rollback(&mut state.players),
            Self::Npcs(undo) => undo.rollback(&mut state.npcs),
            _ => {}
        }
    }
}
impl Feed {
    /// Entity frames read from the socket but not yet applied (the headless
    /// session replay's stream position).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn queued(&self) -> usize {
        self.pending.len()
    }
    pub fn front(&self) -> Option<&Frame> {
        self.pending.front()
    }
    #[cfg(any(test, feature = "test-hooks"))] // test-only introspection
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
    /// Receipts retain RNG usage and zone refresh requests until the owning
    /// adapter has committed the corresponding external context changes.
    pub fn drain_completed(&mut self) -> impl Iterator<Item = (u8, Applied)> + '_ {
        self.completed.drain(..)
    }
    /// Call only after strict framing. The session stops socket polling on a
    /// context failure, so this queue cannot grow indefinitely while blocked.
    pub fn enqueue(&mut self, opcode: u8, payload: &[u8]) -> bool {
        if !handles(opcode) {
            return false;
        }
        self.pending.push_back(Frame {
            opcode,
            payload: payload.to_vec(),
        });
        true
    }
    pub fn apply_next(&mut self, c: &Contexts) -> Result<Option<Applied>> {
        self.apply_next_checked(c, || Ok(()))
    }
    /// `check` runs after a successful decode and before the commit; its error
    /// rejects the packet exactly like a decode error (state untouched, packet
    /// retained, `blocked` set).
    pub fn apply_next_checked(
        &mut self,
        c: &Contexts,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<Option<Applied>> {
        let Some(frame) = self.pending.front() else {
            return Ok(None);
        };
        let opcode = frame.opcode;
        let result = match Self::decode(&mut self.state, frame, c) {
            Ok((change, applied)) => match check() {
                Ok(()) => Ok((change, applied)),
                Err(error) => {
                    change.abort(&mut self.state);
                    Err(error)
                }
            },
            Err(error) => Err(error),
        };
        match result {
            Ok((change, applied)) => {
                self.completed.push_back((opcode, applied.clone()));
                change.commit(&mut self.state);
                self.pending.pop_front();
                self.blocked = None;
                Ok(Some(applied))
            }
            Err(error) => {
                self.blocked = Some(error.clone());
                Err(error)
            }
        }
    }
    /// Uses the same retained front packet as entity dispatch; an error cannot
    /// allow later entity deltas to pass a varp update.
    pub fn apply_varp_next(
        &mut self,
        now: i64,
        bits: &super::varp::BitLookup,
    ) -> Result<Option<Applied>> {
        let Some(frame) = self.pending.front() else {
            return Ok(None);
        };
        let result = match self.state.varps.as_mut() {
            Some(varps) => super::varp::apply(frame.opcode, &frame.payload, varps, now, bits),
            None => Err(Error::UnsupportedContext("local varp state")),
        };
        match result {
            Ok(d) => {
                let mut applied = Applied::bytes(d.consumed);
                applied.varp = Some(VarpReceipt {
                    clock_reads: d.outcome.clock_reads,
                    ignored_overflow: d.outcome.ignored_overflow,
                    transmit_increment: d.transmit_increment,
                });
                self.completed.push_back((frame.opcode, applied.clone()));
                self.pending.pop_front();
                self.blocked = None;
                Ok(Some(applied))
            }
            Err(e) => {
                self.blocked = Some(e.clone());
                Err(e)
            }
        }
    }
    /// Decodes the front packet against the retained state. Only PLAYER_INFO and
    /// NPC_INFO edit `state` here, each rolling itself back on error; every other
    /// packet leaves it untouched and returns the replacement as a [`Change`].
    fn decode(state: &mut State, frame: &Frame, c: &Contexts) -> Result<(Change, Applied)> {
        use wire::server::*;
        let bytes = &frame.payload;
        let mut applied = Applied::bytes(bytes.len());
        let change = match frame.opcode {
            RESET_ANIMS => {
                if !bytes.is_empty() {
                    return Err(Error::Invalid("RESET_ANIMS length"));
                }
                Change::ResetAnims
            }
            NPC_ANIM_SPECIFIC => {
                if bytes.len() != 19 {
                    return Err(Error::Invalid("NPC_ANIM_SPECIFIC length"));
                }
                let context = c
                    .npc
                    .ok_or(Error::UnsupportedContext("live NPC animation context"))?;
                let animation = context
                    .animation
                    .ok_or(Error::UnsupportedContext("NPC animation config"))?;
                let mut p = Packet::new(bytes);
                let modes = (0..4).map(|_| p.g4_alt1()).collect::<Result<Vec<_>>>()?;
                let index =
                    usize::try_from(p.g2()?).map_err(|_| Error::Invalid("NPC animation index"))?;
                let delay = p.g1_alt2()?;
                if p.pos != bytes.len() {
                    return Err(Error::Invalid("NPC_ANIM_SPECIFIC length"));
                }
                match state.npcs.entities.get(&index) {
                    Some(npc) => {
                        let mut npc = npc.clone();
                        npc.path.animation.select_modes(
                            modes,
                            delay,
                            true,
                            npc.path.route_length,
                            &mut npc.path.steps_remaining,
                            animation,
                        )?;
                        Change::Npc(index, Box::new(npc))
                    }
                    None => Change::None,
                }
            }
            NPC_HEADICON_SPECIFIC => {
                if bytes.len() != 9 {
                    return Err(Error::Invalid("NPC_HEADICON_SPECIFIC length"));
                }
                let mut p = Packet::new(bytes);
                let group = p.g4s()?;
                let icon = p.g2()? as i16;
                let slot = usize::try_from(p.g1_alt1()?).unwrap_or(usize::MAX);
                let index = usize::try_from(p.g2_alt3()?).unwrap_or(usize::MAX);
                if p.pos != bytes.len() {
                    return Err(Error::Invalid("NPC_HEADICON_SPECIFIC length"));
                }
                Change::NpcHeadIcon {
                    index,
                    slot,
                    group,
                    icon,
                }
            }
            RESET_CLIENT_VARCACHE => {
                if !bytes.is_empty() {
                    return Err(Error::Invalid("RESET_CLIENT_VARCACHE length"));
                }
                Change::ResetVarcache
            }
            SERVER_TICK_END => {
                if !bytes.is_empty() {
                    return Err(Error::Invalid("SERVER_TICK_END length"));
                }
                applied.read_batch_end = true;
                Change::None
            }
            REBUILD_NORMAL => {
                let context = c
                    .rebuild
                    .ok_or(Error::UnsupportedContext("live rebuild context"))?;
                if state
                    .world
                    .as_ref()
                    .is_some_and(|world| world != context.prior)
                {
                    return Err(Error::Invalid("stale rebuild world context"));
                }
                let d = rebuild_state::decode_normal(bytes, state, context)?;
                applied.bit_pos = d.initial_bits;
                applied.rebuild = Some((d.world, d.effects));
                Change::State(Box::new(d.state))
            }
            REBUILD_REGION => {
                let context = c
                    .rebuild
                    .ok_or(Error::UnsupportedContext("live rebuild context"))?;
                let d = rebuild_state::decode_region(bytes, state, context)?;
                applied.rebuild = Some((d.world, d.effects));
                Change::State(Box::new(d.state))
            }
            LOC_ANIM_SPECIFIC => {
                let context = c
                    .zone
                    .ok_or(Error::UnsupportedContext("live zone context"))?;
                let d = zone_state::decode_loc_anim_specific(bytes, &state.zones, context)?;
                applied.refresh = d.refresh;
                Change::Zones(Box::new(d.state))
            }
            PROJANIM_SPECIFIC => {
                let context = c
                    .zone
                    .ok_or(Error::UnsupportedContext("live zone context"))?;
                let transient = context
                    .transients
                    .ok_or(Error::UnsupportedContext("zone transient context"))?;
                Change::Transients(super::transient::decode_specific(
                    bytes,
                    &state.zones.transients,
                    context.map,
                    transient,
                )?)
            }
            SPOTANIM_SPECIFIC => decode_spotanim_specific(bytes, state, c)?,
            PLAYER_INFO => {
                if !state.initialized {
                    return Err(Error::Invalid("PLAYER_INFO before initialization"));
                }
                let d = super::apply_full(
                    bytes,
                    &mut state.players,
                    c.player
                        .ok_or(Error::UnsupportedContext("live player context"))?,
                )?;
                applied.bit_pos = Some(d.bit_pos);
                Change::Players(Box::new(d.undo))
            }
            NPC_INFO => {
                if !state.initialized {
                    return Err(Error::Invalid("NPC_INFO before initialization"));
                }
                let context = c.npc.ok_or(Error::UnsupportedContext("live NPC context"))?;
                let local = state
                    .players
                    .players
                    .get(context.map.local)
                    .and_then(Option::as_ref)
                    .ok_or(Error::Invalid("missing local player"))?;
                if (local.x[0], local.z[0]) != (context.local_x, context.local_z) {
                    return Err(Error::Invalid("stale NPC local waypoint context"));
                }
                let d = npc::apply(bytes, &mut state.npcs, context)?;
                applied.bit_pos = Some(d.bit_pos);
                applied.random_used = d.random_used;
                Change::Npcs(d.undo)
            }
            op => {
                let input =
                    zone_input(op).ok_or(Error::UnsupportedContext("entity-specific packet"))?;
                if !state.initialized {
                    return Err(Error::Invalid("zone before initialization"));
                }
                let d = zone_state::decode(
                    bytes,
                    &state.zones,
                    c.zone
                        .ok_or(Error::UnsupportedContext("live zone context"))?,
                    input,
                )?;
                applied.refresh = d.refresh;
                Change::Zones(Box::new(d.state))
            }
        };
        Ok((change, applied))
    }
}

/// The `SPOTANIM_SPECIFIC` packet. The packed target selects a
/// map tile, NPC slot, or player slot; all three targets share the same
/// effect/animation metadata and retained spot owners. The new spot is built
/// completely (all fallible steps) before [`Change::commit`] stores it.
fn decode_spotanim_specific(bytes: &[u8], state: &State, c: &Contexts<'_>) -> Result<Change> {
    let zone = c
        .zone
        .ok_or(Error::UnsupportedContext("spot animation context"))?;
    let animation = zone
        .transients
        .ok_or(Error::UnsupportedContext("spot animation config"))?
        .animation;
    let mut p = Packet::new(bytes);
    let info = p.byte()?;
    let slot = usize::from((info & 7) as u8);
    let mut delay = i32::from(((info >> 3) & 15) as u8);
    if delay == 15 {
        delay = -1;
    }
    let targeted = (info & 0x80) != 0;
    let height = p.g2_alt2()?;
    let packed = p.g4_alt1()? as u32;
    let mut effect = p.g2()?;
    if effect == 65535 {
        effect = -1;
    }
    let start = p.g2()?;
    let orientation = p.g1_alt2()?;
    if p.pos != bytes.len() {
        return Err(Error::Invalid("SPOTANIM_SPECIFIC length"));
    }

    let mode_for = |effect: i32| -> Result<(bool, i32)> {
        if effect == -1 {
            return Ok((false, 0));
        }
        let def = animation
            .effects
            .get(&effect)
            .ok_or(Error::UnsupportedContext("spot effect"))?;
        let mode = if targeted {
            1
        } else if def.looping {
            0
        } else {
            2
        };
        Ok((def.looping, mode))
    };
    let set_slot = |spot: &mut crate::entities910::animation_state::Spot| -> Result<()> {
        let (looping, mode) = mode_for(effect)?;
        spot.id = effect;
        spot.height = height;
        spot.orientation = orientation;
        spot.delay = delay;
        spot.looping = looping;
        spot.cancels_on_move = targeted;
        if effect == -1 {
            spot.node.set(-1, 0, 0, animation)?;
        } else {
            let sequence = animation
                .effects
                .get(&effect)
                .map(|e| e.sequence)
                .unwrap_or(-1);
            spot.node.set(sequence, start, mode, animation)?;
        }
        Ok(())
    };

    if packed >> 30 != 0 {
        let level = ((packed >> 28) & 3) as i32;
        let x = ((packed >> 14) & 0x3fff) as i32 - zone.map.base_x;
        let z = (packed & 0x3fff) as i32 - zone.map.base_z;
        if x < 0 || z < 0 || x >= zone.map.width || z >= zone.map.height {
            return Ok(Change::None);
        }
        let key = ((x as i64) << 16) | z as i64;
        if effect == -1 {
            return Ok(Change::TileSpot(key, None));
        }
        let (looping, mode) = mode_for(effect)?;
        let def = animation
            .effects
            .get(&effect)
            .ok_or(Error::UnsupportedContext("spot effect"))?;
        let mut node = crate::entities910::animation_state::Node::default();
        node.set(def.sequence, start, mode, animation)?;
        let fx = x * 512 + 256;
        let fz = z * 512 + 256;
        let spot = crate::entities910::transient::Spot {
            key,
            effect,
            level,
            occlude: level + if level < 3 { zone.map.bridge(x, z) } else { 0 },
            position: [
                fx as f32,
                (zone
                    .transients
                    .ok_or(Error::UnsupportedContext("spot animation config"))?
                    .height)(fx, fz, level)
                .wrapping_sub(height) as f32,
                fz as f32,
            ],
            orientation,
            targeted: i32::from(targeted),
            animation: Some(node),
        };
        let _ = looping;
        return Ok(Change::TileSpot(key, Some(spot)));
    }

    let entity = (packed & 0xffff) as usize;
    if packed >> 29 != 0 {
        if let Some(npc) = state.npcs.entities.get(&entity) {
            if slot < npc.path.animation.spots.len() {
                let mut spot = npc.path.animation.spots[slot].clone();
                set_slot(&mut spot)?;
                return Ok(Change::NpcSpot(entity, slot, spot));
            }
        }
    } else if packed >> 28 != 0 {
        if let Some(player) = state.players.players.get(entity).and_then(Option::as_ref) {
            if slot < player.animation.spots.len() {
                let mut spot = player.animation.spots[slot].clone();
                set_slot(&mut spot)?;
                return Ok(Change::PlayerSpot(entity, slot, spot));
            }
        }
    }
    Ok(Change::None)
}

#[cfg(test)]
mod tests;
