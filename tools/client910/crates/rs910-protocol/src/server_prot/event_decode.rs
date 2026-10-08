//! Server opcode dispatch into typed UI and scene events.

/// `PayloadReader` and `cp1252_byte` (split out in Phase 2.2).
pub use crate::payload_reader::*;

use super::{
    crc32, decode_g1_alt1, decode_g1_alt2, decode_g1_alt3, decode_g2_alt2_u16, decode_g4_alt1,
    is_telemetry_opcode, parse_audio_event, parse_client_debug_event, parse_environment_override,
    parse_if_setposition, parse_if_setscrollpos, parse_if_settext, parse_runclientscript,
    ActiveBinding, ActiveVariant, InterfaceCoord, UiEvent,
};

pub fn parse_ui_event(opcode: u8, payload: &[u8]) -> anyhow::Result<Option<UiEvent>> {
    if is_telemetry_opcode(opcode) {
        return Ok(Some(UiEvent::Telemetry {
            opcode,
            bytes: payload.to_vec(),
        }));
    }
    if let Some(event) = parse_audio_event(opcode, payload)? {
        return Ok(Some(event));
    }
    if let Some(event) = parse_client_debug_event(opcode, payload)? {
        return Ok(Some(event));
    }
    if opcode == crate::proto::server::PLAYER_SNAPSHOT {
        anyhow::ensure!(payload.len() >= 2, "PLAYER_SNAPSHOT header");
        let snapshot = i32::from(payload[0]);
        return Ok(Some(UiEvent::PlayerSnapshot {
            id: -snapshot - 2,
            gender: payload[1] as i8,
            bytes: payload[2..].to_vec(),
        }));
    }
    if opcode == crate::proto::server::CLEAR_PLAYER_SNAPSHOT {
        anyhow::ensure!(payload.len() == 1, "CLEAR_PLAYER_SNAPSHOT length");
        let snapshot = i32::from(payload[0]);
        return Ok(Some(UiEvent::ClearPlayerSnapshot { id: -snapshot - 2 }));
    }
    if opcode == crate::proto::server::LOBBY_APPEARANCE {
        anyhow::ensure!(!payload.is_empty(), "LOBBY_APPEARANCE header");
        return Ok(Some(UiEvent::LobbyAppearance {
            gender: payload[0] as i8,
            bytes: payload[1..].to_vec(),
        }));
    }
    if opcode == crate::proto::server::UPDATE_STOCKMARKET_SLOT {
        anyhow::ensure!(payload.len() == 21, "UPDATE_STOCKMARKET_SLOT length");
        let mut r = PayloadReader::new(payload);
        let market = usize::from(r.g1()?);
        let slot = usize::from(r.g1()?);
        anyhow::ensure!(market < 3, "stockmarket market index {market}");
        anyhow::ensure!(slot < 8, "stockmarket slot index {slot}");
        let state = r.g1()?;
        let object = r.g2()? as i32;
        let price = r.g4s()?;
        let count = r.g4s()?;
        let completed_count = r.g4s()?;
        let completed_gold = r.g4s()?;
        r.finish("UPDATE_STOCKMARKET_SLOT")?;
        return Ok(Some(UiEvent::StockmarketSlot {
            market,
            slot,
            state,
            object,
            price,
            count,
            completed_count,
            completed_gold,
        }));
    }
    if opcode == crate::proto::server::CUTSCENE {
        let mut r = PayloadReader::new(payload);
        let id = r.g2()?;
        let parameter = r.g2()?;
        let length = usize::from(r.g1()?);
        anyhow::ensure!(r.remaining() == length, "CUTSCENE appearance length");
        let start = r.pos;
        let appearance = payload[start..].to_vec();
        r.pos += length;
        r.finish("CUTSCENE")?;
        return Ok(Some(UiEvent::Cutscene {
            id,
            parameter,
            appearance,
        }));
    }
    if opcode == crate::proto::server::UPDATE_SITESETTINGS {
        let mut reader = PayloadReader::new(payload);
        let value = reader.gjstr()?;
        reader.finish("UPDATE_SITESETTINGS")?;
        return Ok(Some(UiEvent::SiteSettings { value }));
    }
    if opcode == crate::proto::server::UPDATE_UID192 {
        anyhow::ensure!(payload.len() == 28, "UPDATE_UID192 length");
        let mut value = [0u8; 24];
        value.copy_from_slice(&payload[..24]);
        let expected = u32::from_be_bytes(payload[24..].try_into().unwrap());
        let value = (crc32(&value) == expected).then_some(value);
        return Ok(Some(UiEvent::Uid192 { value }));
    }
    if opcode == crate::proto::server::LAST_LOGIN_INFO {
        anyhow::ensure!(payload.len() == 4, "LAST_LOGIN_INFO length");
        let mut reader = PayloadReader::new(payload);
        let address = reader.g4s()?;
        reader.finish("LAST_LOGIN_INFO")?;
        return Ok(Some(UiEvent::LastLoginInfo { address }));
    }
    if opcode == crate::proto::server::URL_OPEN {
        let mut reader = PayloadReader::new(payload);
        let javascript = reader.g1()? == 1;
        let primary = reader.gjstr()?;
        let fallback = if javascript && reader.remaining() > 0 {
            Some(reader.gjstr()?)
        } else {
            None
        };
        reader.finish("URL_OPEN")?;
        return Ok(Some(UiEvent::UrlOpen {
            primary,
            fallback,
            javascript,
        }));
    }
    if opcode == crate::proto::server::SOCIAL_NETWORK_LOGOUT {
        let url = payload
            .iter()
            .copied()
            .map(crate::ui_dialogue::cp1252_decode_byte)
            .collect();
        return Ok(Some(UiEvent::SocialNetworkLogout { url }));
    }
    if opcode == crate::proto::server::ENVIRONMENT_OVERRIDE {
        return Ok(Some(UiEvent::OverrideEnvironment(
            parse_environment_override(payload)?,
        )));
    }
    if opcode == crate::proto::server::CAM_RESET {
        anyhow::ensure!(payload.is_empty(), "CAM_RESET length");
        return Ok(Some(UiEvent::CameraReset));
    }
    if opcode == crate::proto::server::CAM_SMOOTHRESET {
        anyhow::ensure!(payload.is_empty(), "CAM_SMOOTHRESET length");
        return Ok(Some(UiEvent::CameraSmoothReset));
    }
    if opcode == crate::proto::server::CAM_REMOVEROOF {
        let mut reader = PayloadReader::new(payload);
        let packed = reader.g4_alt2()?;
        reader.finish("CAM_REMOVEROOF")?;
        return Ok(Some(UiEvent::CameraRemoveRoof { packed }));
    }
    if opcode == crate::proto::server::POINTLIGHT_COLOUR {
        anyhow::ensure!(payload.len() == 8, "POINTLIGHT_COLOUR length");
        let mut reader = PayloadReader::new(payload);
        let colour = reader.g4_alt3()?;
        let duration = reader.g2_alt1()?;
        let id = reader.g2_alt3()?;
        reader.finish("POINTLIGHT_COLOUR")?;
        return Ok(Some(UiEvent::PointLightColour {
            duration,
            colour,
            id,
        }));
    }
    if opcode == crate::proto::server::POINTLIGHT_INTENSITY {
        anyhow::ensure!(payload.len() == 5, "POINTLIGHT_INTENSITY length");
        let mut reader = PayloadReader::new(payload);
        let duration = reader.g2_alt2_u16()?;
        let id = reader.g2_alt2_u16()?;
        let raw = reader.g1()?;
        reader.finish("POINTLIGHT_INTENSITY")?;
        return Ok(Some(UiEvent::PointLightIntensity {
            duration,
            intensity: if raw == 255 { -1 } else { i32::from(raw) },
            id,
        }));
    }
    if opcode == crate::proto::server::CAM_FORCEANGLE {
        anyhow::ensure!(payload.len() == 4, "CAM_FORCEANGLE length");
        let mut r = PayloadReader::new(payload);
        let yaw = r.g2_alt1()?;
        let pitch = r.g2()?;
        r.finish("CAM_FORCEANGLE")?;
        return Ok(Some(UiEvent::CameraForceAngle { pitch, yaw }));
    }
    if opcode == crate::proto::server::CAM_SHAKE {
        anyhow::ensure!(payload.len() == 6, "CAM_SHAKE length");
        let mut r = PayloadReader::new(payload);
        let wobble_speed = r.g2_alt2_u16()?;
        let channel = r.g1_alt1()?;
        let jitter = r.g1_alt2()?;
        let wobble_scale = r.g1_alt3()?;
        let cycle = r.g1()?;
        r.finish("CAM_SHAKE")?;
        return Ok(Some(UiEvent::CameraShake {
            channel,
            jitter,
            wobble_scale,
            cycle,
            wobble_speed,
        }));
    }
    if opcode == crate::proto::server::CAM_MOVETO {
        anyhow::ensure!(payload.len() == 6, "CAM_MOVETO length");
        let mut r = PayloadReader::new(payload);
        let x = r.g1_alt2()?;
        let acceleration = r.g1_alt1()?;
        let z = r.g1()?;
        let speed = r.g1_alt3()?;
        let source_height = r.g2()?;
        r.finish("CAM_MOVETO")?;
        return Ok(Some(UiEvent::CameraMoveTo {
            x,
            z,
            source_height,
            acceleration,
            speed,
        }));
    }
    if opcode == crate::proto::server::CAM_LOOKAT {
        anyhow::ensure!(payload.len() == 6, "CAM_LOOKAT length");
        let mut r = PayloadReader::new(payload);
        let x = r.g1_alt2()?;
        let z = r.g1()?;
        let speed = r.g1_alt3()?;
        let acceleration = r.g1_alt2()?;
        let height = r.g2_alt2_u16()?;
        r.finish("CAM_LOOKAT")?;
        return Ok(Some(UiEvent::CameraLookAt {
            x,
            z,
            height,
            acceleration,
            speed,
        }));
    }
    if matches!(
        opcode,
        crate::proto::server::CAM2_ENABLE | crate::proto::server::CAMERA_UPDATE
    ) {
        anyhow::ensure!(
            if opcode == crate::proto::server::CAM2_ENABLE {
                payload.len() == 1
            } else {
                !payload.is_empty()
            },
            "invalid camera packet length"
        );
        return Ok(Some(UiEvent::Camera {
            opcode,
            bytes: payload.to_vec(),
        }));
    }
    use crate::proto::server as p;
    if matches!(
        opcode,
        p::UPDATE_INV_FULL | p::UPDATE_INV_PARTIAL | p::UPDATE_INV_STOP_TRANSMIT
    ) {
        return Ok(Some(UiEvent::Inventory {
            opcode,
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::UPDATE_STAT {
        anyhow::ensure!(payload.len() == 6, "UPDATE_STAT length");
        let mut r = PayloadReader::new(payload);
        let current_level = r.g1_alt2()?;
        let xp = r.g4_alt1()?;
        let skill = r.g1_alt3()?;
        r.finish("UPDATE_STAT")?;
        return Ok(Some(UiEvent::Stat {
            current_level,
            xp,
            skill,
        }));
    }
    if opcode == p::UPDATE_RUNENERGY {
        anyhow::ensure!(payload.len() == 1, "UPDATE_RUNENERGY length");
        return Ok(Some(UiEvent::RunEnergy { value: payload[0] }));
    }
    if opcode == p::UPDATE_RUNWEIGHT {
        anyhow::ensure!(payload.len() == 2, "UPDATE_RUNWEIGHT length");
        return Ok(Some(UiEvent::RunWeight {
            value: i16::from_be_bytes([payload[0], payload[1]]),
        }));
    }
    if opcode == p::UPDATE_REBOOT_TIMER {
        anyhow::ensure!(payload.len() == 2, "UPDATE_REBOOT_TIMER length");
        return Ok(Some(UiEvent::RebootTimer {
            ticks: u16::from_be_bytes([payload[0], payload[1]]),
        }));
    }
    if opcode == p::SET_TARGET {
        anyhow::ensure!(payload.len() == 2, "SET_TARGET length");
        let mut r = PayloadReader::new(payload);
        let value = r.g2s_alt2()?;
        r.finish("SET_TARGET")?;
        return Ok(Some(UiEvent::SetTarget { value }));
    }
    if opcode == p::SETDRAWORDER {
        anyhow::ensure!(payload.len() == 1, "SETDRAWORDER length");
        return Ok(Some(UiEvent::SetDrawOrder {
            value: decode_g1_alt3(payload[0]),
        }));
    }
    if opcode == p::CHAT_FILTER_SETTINGS {
        anyhow::ensure!(payload.len() == 2, "CHAT_FILTER_SETTINGS length");
        return Ok(Some(UiEvent::ChatFilters {
            trade: i32::from(decode_g1_alt2(payload[0])),
            public: i32::from(decode_g1_alt2(payload[1])),
        }));
    }
    if opcode == p::CHAT_FILTER_SETTINGS_PRIVATECHAT {
        anyhow::ensure!(
            payload.len() == 1,
            "CHAT_FILTER_SETTINGS_PRIVATECHAT length"
        );
        let raw = payload[0];
        return Ok(Some(UiEvent::ChatPrivateFilter {
            value: (raw <= 2).then_some(i32::from(raw)),
        }));
    }
    if opcode == p::CREATE_CHECK_EMAIL_REPLY {
        anyhow::ensure!(payload.len() == 1, "CREATE_CHECK_EMAIL_REPLY length");
        let value = match payload[0] {
            2 | 3 | 20 | 21 => i32::from(payload[0]),
            _ => 3,
        };
        return Ok(Some(UiEvent::CreateEmailReply { value }));
    }
    if opcode == p::CREATE_ACCOUNT_REPLY {
        anyhow::ensure!(payload.len() == 1, "CREATE_ACCOUNT_REPLY length");
        let value = match payload[0] {
            2..=10 | 20..=21 | 30..=34 | 38 => i32::from(payload[0]),
            _ => 3,
        };
        return Ok(Some(UiEvent::AccountCreationResult { value }));
    }
    if opcode == p::CREATE_CHECK_NAME_REPLY {
        anyhow::ensure!(payload.len() == 1, "CREATE_CHECK_NAME_REPLY length");
        let value = match payload[0] {
            2..=8 => i32::from(payload[0]),
            _ => 3,
        };
        return Ok(Some(UiEvent::CreateNameReply { value }));
    }
    if opcode == p::CREATE_SUGGEST_NAME_ERROR {
        anyhow::ensure!(payload.len() == 1, "CREATE_SUGGEST_NAME_ERROR length");
        let value = match payload[0] {
            2..=4 => i32::from(payload[0]),
            _ => 3,
        };
        return Ok(Some(UiEvent::CreateSuggestNameError { value }));
    }
    if opcode == p::CREATE_SUGGEST_NAME_REPLY {
        let mut reader = PayloadReader::new(payload);
        let value = reader.gjstr()?;
        reader.finish("CREATE_SUGGEST_NAME_REPLY")?;
        return Ok(Some(UiEvent::CreateSuggestName { value }));
    }
    if opcode == p::UPDATE_DOB {
        anyhow::ensure!(payload.len() == 4, "UPDATE_DOB length");
        let mut r = PayloadReader::new(payload);
        let dob = r.g3s()?;
        let verified = r.g1()? == 1;
        r.finish("UPDATE_DOB")?;
        return Ok(Some(UiEvent::UpdateDob { dob, verified }));
    }
    if opcode == p::LOYALTY_UPDATE {
        anyhow::ensure!(payload.len() == 4, "LOYALTY_UPDATE length");
        let mut r = PayloadReader::new(payload);
        let value = r.g4_alt3()?;
        r.finish("LOYALTY_UPDATE")?;
        return Ok(Some(UiEvent::LoyaltyUpdate { value }));
    }
    if opcode == p::JCOINS_UPDATE {
        anyhow::ensure!(payload.len() == 4, "JCOINS_UPDATE length");
        let mut r = PayloadReader::new(payload);
        let value = r.g4_alt2()?;
        r.finish("JCOINS_UPDATE")?;
        return Ok(Some(UiEvent::JCoinsUpdate { value }));
    }
    if opcode == p::TRIGGER_ONDIALOGABORT {
        anyhow::ensure!(payload.is_empty(), "TRIGGER_ONDIALOGABORT length");
        return Ok(Some(UiEvent::TriggerDialogAbort));
    }
    if opcode == p::MESSAGE_GAME {
        let mut r = PayloadReader::new(payload);
        let chat_type = r.gsmart1or2()?;
        let flags = r.g4s()?;
        let name_flags = r.g1()?;
        let mut name = String::new();
        let mut name_unfiltered = String::new();
        if name_flags & 1 != 0 {
            name = r.gjstr()?;
            name_unfiltered = if name_flags & 2 == 0 {
                name.clone()
            } else {
                r.gjstr()?
            };
        }
        let message = r.gjstr()?;
        r.finish("MESSAGE_GAME")?;
        return Ok(Some(UiEvent::GameMessage {
            chat_type,
            flags,
            name,
            name_unfiltered,
            message,
        }));
    }
    if opcode == p::UPDATE_FRIENDLIST {
        return Ok(Some(UiEvent::FriendList {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::FRIENDLIST_LOADED {
        anyhow::ensure!(payload.is_empty(), "FRIENDLIST_LOADED length");
        return Ok(Some(UiEvent::FriendListLoaded));
    }
    if opcode == p::UPDATE_IGNORELIST {
        return Ok(Some(UiEvent::IgnoreList {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::UPDATE_FRIENDCHAT_CHANNEL_FULL {
        return Ok(Some(UiEvent::FriendChatFull {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::UPDATE_FRIENDCHAT_CHANNEL_SINGLEUSER {
        anyhow::ensure!(
            !payload.is_empty(),
            "UPDATE_FRIENDCHAT_CHANNEL_SINGLEUSER empty payload"
        );
        return Ok(Some(UiEvent::FriendChatSingle {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::MESSAGE_FRIENDCHANNEL {
        anyhow::ensure!(!payload.is_empty(), "MESSAGE_FRIENDCHANNEL empty payload");
        return Ok(Some(UiEvent::FriendChannelMessage {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::CLANCHANNEL_FULL {
        anyhow::ensure!(!payload.is_empty(), "CLANCHANNEL_FULL empty payload");
        return Ok(Some(UiEvent::ClanChannelFull {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::CLANCHANNEL_DELTA {
        anyhow::ensure!(!payload.is_empty(), "CLANCHANNEL_DELTA empty payload");
        return Ok(Some(UiEvent::ClanRosterDelta {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::CLANSETTINGS_FULL {
        anyhow::ensure!(!payload.is_empty(), "CLANSETTINGS_FULL empty payload");
        return Ok(Some(UiEvent::ClanSettingsFull {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::CLANSETTINGS_DELTA {
        anyhow::ensure!(!payload.is_empty(), "CLANSETTINGS_DELTA empty payload");
        return Ok(Some(UiEvent::ClanSettingsUpdate {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::MESSAGE_CLANCHANNEL {
        anyhow::ensure!(!payload.is_empty(), "MESSAGE_CLANCHANNEL empty payload");
        return Ok(Some(UiEvent::ClanChannelMessage {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::MESSAGE_CLANCHANNEL_SYSTEM {
        anyhow::ensure!(
            !payload.is_empty(),
            "MESSAGE_CLANCHANNEL_SYSTEM empty payload"
        );
        return Ok(Some(UiEvent::ClanChannelSystemMessage {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::PLAYER_GROUP_FULL {
        return Ok(Some(UiEvent::PlayerGroupFull {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::PLAYER_GROUP_DELTA {
        anyhow::ensure!(!payload.is_empty(), "PLAYER_GROUP_DELTA empty payload");
        return Ok(Some(UiEvent::GroupRosterDelta {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::PLAYER_GROUP_VARPS {
        anyhow::ensure!(payload.len() >= 3, "PLAYER_GROUP_VARPS header");
        return Ok(Some(UiEvent::PlayerGroupVars {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::MESSAGE_PLAYER_GROUP {
        anyhow::ensure!(!payload.is_empty(), "MESSAGE_PLAYER_GROUP empty payload");
        return Ok(Some(UiEvent::PlayerGroupMessage {
            bytes: payload.to_vec(),
        }));
    }
    if matches!(
        opcode,
        p::MESSAGE_QUICKCHAT_PRIVATE
            | p::MESSAGE_QUICKCHAT_CLANCHANNEL
            | p::MESSAGE_QUICKCHAT_PRIVATE_ECHO
            | p::MESSAGE_QUICKCHAT_FRIENDCHAT
            | p::MESSAGE_QUICKCHAT_PLAYER_GROUP
    ) {
        anyhow::ensure!(!payload.is_empty(), "quick-chat message empty payload");
        return Ok(Some(UiEvent::QuickChat {
            opcode,
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::MESSAGE_PUBLIC {
        // The public message resolves the player against the live actor
        // table; retain the wire payload until Runtime::packet owns that
        // table, so an unknown/reused player index is handled there.
        anyhow::ensure!(payload.len() >= 5, "MESSAGE_PUBLIC truncated header");
        return Ok(Some(UiEvent::PublicMessage {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::MESSAGE_PRIVATE_ECHO {
        let mut reader = PayloadReader::new(payload);
        let _ = reader.gjstr()?;
        anyhow::ensure!(
            reader.remaining() > 0,
            "MESSAGE_PRIVATE_ECHO missing packed body"
        );
        return Ok(Some(UiEvent::PrivateMessageEcho {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::MESSAGE_PRIVATE {
        let mut reader = PayloadReader::new(payload);
        let has_unfiltered = reader.g1()? == 1;
        let _ = reader.gjstr()?;
        if has_unfiltered {
            let _ = reader.gjstr()?;
        }
        let _ = reader.g2()?;
        let _ = reader.g3s()?;
        let _ = reader.g1()?;
        anyhow::ensure!(
            reader.remaining() > 0,
            "MESSAGE_PRIVATE missing packed body"
        );
        return Ok(Some(UiEvent::PrivateMessage {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::CHANGE_LOBBY {
        let mut r = PayloadReader::new(payload);
        let host = r.gjstr()?;
        let node = r.g2()?;
        let port = r.g2()?;
        let port2 = r.g2()?;
        r.finish("CHANGE_LOBBY")?;
        return Ok(Some(UiEvent::ChangeLobby {
            host,
            node,
            port,
            port2,
        }));
    }
    if opcode == p::LOGOUT_TRANSFER {
        let mut r = PayloadReader::new(payload);
        let world_id = r.g2()?;
        let host = r.gjstr()?;
        let port = r.g2()?;
        let port2 = r.g2()?;
        let cancellable = r.g1()? == 1;
        r.finish("LOGOUT_TRANSFER")?;
        return Ok(Some(UiEvent::LogoutTransfer {
            world_id,
            host,
            port,
            port2,
            cancellable,
        }));
    }
    if opcode == p::LOGOUT || opcode == p::LOGOUT_FULL {
        anyhow::ensure!(payload.len() == 1, "logout packet length");
        return Ok(Some(UiEvent::Logout {
            reason: payload[0],
            full: opcode == p::LOGOUT_FULL,
        }));
    }
    if opcode == p::IF_SETANIM {
        anyhow::ensure!(payload.len() == 8, "IF_SETANIM length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4s()? as u32;
        let animation = r.g4_alt3()?;
        r.finish("IF_SETANIM")?;
        return Ok(Some(UiEvent::SetInterfaceAnim { packed, animation }));
    }
    if opcode == p::IF_SETMODEL {
        anyhow::ensure!(payload.len() == 8, "IF_SETMODEL length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt2()? as u32;
        let model = r.g4s()?;
        r.finish("IF_SETMODEL")?;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 1,
            model,
            model_name_hash: -1,
            local_player: false,
        }));
    }
    if opcode == p::IF_SETOBJECT {
        anyhow::ensure!(payload.len() == 10, "IF_SETOBJECT length");
        let mut r = PayloadReader::new(payload);
        let raw_object = r.g2()?;
        let object = if raw_object == u16::MAX {
            -1
        } else {
            i32::from(raw_object)
        };
        let packed = r.g4_alt3()? as u32;
        let count = r.g4_alt1()?;
        r.finish("IF_SETOBJECT")?;
        return Ok(Some(UiEvent::SetInterfaceObject {
            packed,
            object,
            count,
        }));
    }
    if opcode == p::IF_SET_HTTP_IMAGE {
        anyhow::ensure!(payload.len() == 8, "IF_SET_HTTP_IMAGE length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4s()? as u32;
        let image = r.g4_alt1()?;
        r.finish("IF_SET_HTTP_IMAGE")?;
        return Ok(Some(UiEvent::SetInterfaceHttpImage { packed, image }));
    }
    if opcode == p::SET_MAP_FLAG {
        anyhow::ensure!(payload.len() == 2, "SET_MAP_FLAG length");
        let raw_x = decode_g1_alt1(payload[0]);
        let (x, z) = if raw_x == 255 {
            (-1, -1)
        } else {
            (i32::from(raw_x), i32::from(payload[1]))
        };
        return Ok(Some(UiEvent::SetMapFlag { x, z }));
    }
    if opcode == p::MINIMAP_TOGGLE {
        anyhow::ensure!(payload.len() == 1, "MINIMAP_TOGGLE length");
        return Ok(Some(UiEvent::MinimapToggle {
            toggle: i32::from(payload[0]),
        }));
    }
    if opcode == p::HINT_ARROW {
        anyhow::ensure!(payload.len() == 14, "HINT_ARROW length");
        return Ok(Some(UiEvent::HintArrow {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::HINT_TRAIL {
        let mut r = PayloadReader::new(payload);
        let slot = usize::from(r.g1()?);
        anyhow::ensure!(slot < 8, "HINT_TRAIL slot {slot}");
        let model = r.gsmart2or4s()?;
        let mut points = Vec::new();
        if model != -1 {
            let count = r.gsmart1or2s()?;
            anyhow::ensure!(
                (0..=4096).contains(&count),
                "HINT_TRAIL point count {count}"
            );
            let count = usize::try_from(count).expect("validated HINT_TRAIL count");
            let mut x = i32::from(r.g2()?);
            let mut z = i32::from(r.g2()?);
            points.reserve(count);
            for _ in 0..count {
                x += i32::from(r.g1s()?);
                z += i32::from(r.g1s()?);
                points.push([x, z]);
            }
        }
        r.finish("HINT_TRAIL")?;
        return Ok(Some(UiEvent::HintTrail {
            slot,
            model,
            points,
        }));
    }
    if opcode == p::IF_SETPLAYERHEAD {
        anyhow::ensure!(payload.len() == 4, "IF_SETPLAYERHEAD length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt1()? as u32;
        r.finish("IF_SETPLAYERHEAD")?;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 3,
            model: 0,
            model_name_hash: 0,
            local_player: true,
        }));
    }
    if opcode == p::IF_SETPLAYERHEAD_OTHER {
        anyhow::ensure!(payload.len() == 10, "IF_SETPLAYERHEAD_OTHER length");
        let mut r = PayloadReader::new(payload);
        let model_name_hash = r.g4_alt1()?;
        let model = i32::from(r.g2_alt2_u16()?);
        let packed = r.g4_alt3()? as u32;
        r.finish("IF_SETPLAYERHEAD_OTHER")?;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 3,
            model,
            model_name_hash,
            local_player: false,
        }));
    }
    if opcode == p::IF_SETPLAYERHEAD_IGNOREWORN {
        anyhow::ensure!(payload.len() == 10, "IF_SETPLAYERHEAD_IGNOREWORN length");
        let mut r = PayloadReader::new(payload);
        let low = r.g2()?;
        let packed = r.g4_alt2()? as u32;
        let model_name_hash = i32::from(r.g2_alt1()?);
        let high = r.g2_alt1()?;
        r.finish("IF_SETPLAYERHEAD_IGNOREWORN")?;
        let model = ((u32::from(high) << 16) | u32::from(low)) as i32;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 7,
            model,
            model_name_hash,
            local_player: false,
        }));
    }
    if opcode == p::IF_SETPLAYERMODEL_SELF {
        anyhow::ensure!(payload.len() == 4, "IF_SETPLAYERMODEL_SELF length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4s()? as u32;
        r.finish("IF_SETPLAYERMODEL_SELF")?;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 5,
            model: 0,
            model_name_hash: 0,
            local_player: true,
        }));
    }
    if opcode == p::IF_SETPLAYERMODEL_OTHER {
        anyhow::ensure!(payload.len() == 10, "IF_SETPLAYERMODEL_OTHER length");
        let mut r = PayloadReader::new(payload);
        let model = i32::from(r.g2_alt1()?);
        let packed = r.g4s()? as u32;
        let model_name_hash = r.g4s()?;
        r.finish("IF_SETPLAYERMODEL_OTHER")?;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 5,
            model,
            model_name_hash,
            local_player: false,
        }));
    }
    if opcode == p::IF_SETPLAYERMODEL_SNAPSHOT {
        anyhow::ensure!(payload.len() == 5, "IF_SETPLAYERMODEL_SNAPSHOT length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt3()? as u32;
        let snapshot = i32::from(r.g1_alt1()?);
        r.finish("IF_SETPLAYERMODEL_SNAPSHOT")?;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 5,
            model: -snapshot - 2,
            model_name_hash: 0,
            local_player: false,
        }));
    }
    if opcode == p::IF_SETNPCHEAD {
        anyhow::ensure!(payload.len() == 8, "IF_SETNPCHEAD length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt3()? as u32;
        let model = r.g4_alt2()?;
        r.finish("IF_SETNPCHEAD")?;
        return Ok(Some(UiEvent::SetInterfaceModel {
            packed,
            model_kind: 2,
            model,
            model_name_hash: -1,
            local_player: false,
        }));
    }
    if opcode == p::IF_SETCOLOUR {
        anyhow::ensure!(payload.len() == 6, "IF_SETCOLOUR length");
        let mut r = PayloadReader::new(payload);
        let colour = r.g2_alt2_u16()?;
        let packed = r.g4s()? as u32;
        r.finish("IF_SETCOLOUR")?;
        return Ok(Some(UiEvent::SetInterfaceColour { packed, colour }));
    }
    if opcode == p::IF_SETANGLE {
        anyhow::ensure!(payload.len() == 10, "IF_SETANGLE length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt2()? as u32;
        let x = r.g2_alt1()?;
        let y = r.g2_alt1()?;
        let zoom = r.g2_alt2_u16()?;
        r.finish("IF_SETANGLE")?;
        return Ok(Some(UiEvent::SetInterfaceAngle { packed, x, y, zoom }));
    }
    if opcode == p::IF_SETGRAPHIC {
        anyhow::ensure!(payload.len() == 8, "IF_SETGRAPHIC length");
        let mut r = PayloadReader::new(payload);
        let graphic = r.g4_alt2()?;
        let packed = r.g4_alt1()? as u32;
        r.finish("IF_SETGRAPHIC")?;
        return Ok(Some(UiEvent::SetInterfaceGraphic { packed, graphic }));
    }
    if opcode == p::IF_SETTEXTANTIMACRO {
        anyhow::ensure!(payload.len() == 5, "IF_SETTEXTANTIMACRO length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt3()? as u32;
        let enabled = r.g1()? == 1;
        r.finish("IF_SETTEXTANTIMACRO")?;
        return Ok(Some(UiEvent::SetInterfaceTextAntiMacro { packed, enabled }));
    }
    if opcode == p::IF_SETTEXTFONT {
        anyhow::ensure!(payload.len() == 8, "IF_SETTEXTFONT length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt1()? as u32;
        let font = r.g4_alt3()?;
        r.finish("IF_SETTEXTFONT")?;
        return Ok(Some(UiEvent::SetInterfaceTextFont { packed, font }));
    }
    if opcode == p::IF_SETCLICKMASK {
        anyhow::ensure!(payload.len() == 5, "IF_SETCLICKMASK length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt3()? as u32;
        let enabled = r.g1_alt2()? == 1;
        r.finish("IF_SETCLICKMASK")?;
        return Ok(Some(UiEvent::SetInterfaceClickMask { packed, enabled }));
    }
    if opcode == p::IF_SETRECOL {
        anyhow::ensure!(payload.len() == 9, "IF_SETRECOL length");
        let mut r = PayloadReader::new(payload);
        let packed = r.g4_alt1()? as u32;
        let source = r.g2_alt2_u16()?;
        let destination = r.g2_alt1()?;
        let index = r.g1_alt1()?;
        r.finish("IF_SETRECOL")?;
        return Ok(Some(UiEvent::SetInterfaceRecolour {
            packed,
            index,
            source,
            destination,
        }));
    }
    if opcode == p::IF_SETRETEX {
        anyhow::ensure!(payload.len() == 9, "IF_SETRETEX length");
        let mut r = PayloadReader::new(payload);
        let source = r.g2_alt3()?;
        let destination = r.g2_alt1()?;
        let packed = r.g4s()? as u32;
        let index = r.g1_alt1()?;
        r.finish("IF_SETRETEX")?;
        return Ok(Some(UiEvent::SetInterfaceRetexture {
            packed,
            index,
            source,
            destination,
        }));
    }
    if matches!(opcode, p::CLIENT_SETVARC_SMALL | p::CLIENT_SETVARC_LARGE) {
        let (value, id) = if opcode == p::CLIENT_SETVARC_SMALL {
            anyhow::ensure!(payload.len() == 3, "CLIENT_SETVARC_SMALL length");
            (
                i32::from(decode_g1_alt3(payload[0]) as i8),
                i32::from(u16::from_be_bytes([payload[1], payload[2]])),
            )
        } else {
            anyhow::ensure!(payload.len() == 6, "CLIENT_SETVARC_LARGE length");
            (
                decode_g4_alt1(payload[0..4].try_into().unwrap()),
                i32::from(decode_g2_alt2_u16(payload[4], payload[5])),
            )
        };
        return Ok(Some(UiEvent::SetVarc { id, value }));
    }
    if opcode == p::CLIENT_SETVARCBIT_SMALL {
        anyhow::ensure!(payload.len() == 3, "CLIENT_SETVARCBIT_SMALL length");
        let id = i32::from(u16::from_be_bytes([payload[0], payload[1]]));
        let value = i32::from((128u8.wrapping_sub(payload[2])) as i8);
        return Ok(Some(UiEvent::SetVarcBit { id, value }));
    }
    if opcode == p::CLIENT_SETVARCBIT_LARGE {
        anyhow::ensure!(payload.len() == 6, "CLIENT_SETVARCBIT_LARGE length");
        let id = i32::from(decode_g2_alt2_u16(payload[0], payload[1]));
        let value = decode_g4_alt1(payload[2..6].try_into().unwrap());
        return Ok(Some(UiEvent::SetVarcBit { id, value }));
    }
    if matches!(
        opcode,
        p::CLIENT_SETVARCSTR_SMALL | p::CLIENT_SETVARCSTR_LARGE
    ) {
        anyhow::ensure!(payload.len() >= 3, "CLIENT_SETVARCSTR length");
        let id = i32::from(u16::from_be_bytes([payload[0], payload[1]]));
        let mut r = PayloadReader::new(&payload[2..]);
        let value = r.gjstr()?.encode_utf16().collect();
        r.finish("CLIENT_SETVARCSTR")?;
        return Ok(Some(UiEvent::SetVarcString { id, value }));
    }
    if opcode == p::VARCLAN_ENABLE {
        anyhow::ensure!(payload.is_empty(), "VARCLAN_ENABLE length");
        return Ok(Some(UiEvent::VarClanEnable));
    }
    if opcode == p::VARCLAN_DISABLE {
        anyhow::ensure!(payload.is_empty(), "VARCLAN_DISABLE length");
        return Ok(Some(UiEvent::VarClanDisable));
    }
    if opcode == p::VARCLAN {
        anyhow::ensure!(payload.len() >= 2, "VARCLAN truncated");
        return Ok(Some(UiEvent::VarClan {
            bytes: payload.to_vec(),
        }));
    }
    if opcode == p::SET_MOVEACTION {
        let mut r = PayloadReader::new(payload);
        let text = if payload.len() > 2 {
            r.gjstr()?
        } else {
            rs910_core::texts::Msg::WalkHere.get().to_string()
        };
        let action = if payload.is_empty() {
            -1
        } else {
            anyhow::ensure!(payload.len() - r.pos == 2, "SET_MOVEACTION length");
            let v = r.g2()?;
            if v == 65535 {
                -1
            } else {
                v as i32
            }
        };
        r.finish("SET_MOVEACTION")?;
        return Ok(Some(UiEvent::SetMoveAction { text, action }));
    }
    if opcode == p::SHOW_FACE_HERE {
        anyhow::ensure!(payload.len() == 1, "SHOW_FACE_HERE length");
        return Ok(Some(UiEvent::ShowFaceHere {
            enabled: decode_g1_alt3(payload[0]) == 1,
        }));
    }
    if opcode == p::REDUCE_PLAYER_ATTACK_PRIORITY {
        let mut r = PayloadReader::new(payload);
        let value = r.g1()?;
        r.finish("REDUCE_PLAYER_ATTACK_PRIORITY")?;
        return Ok(Some(UiEvent::PlayerAttackPriority { value }));
    }
    if opcode == p::REDUCE_NPC_ATTACK_PRIORITY {
        let mut r = PayloadReader::new(payload);
        let value = r.g1_alt3()?;
        r.finish("REDUCE_NPC_ATTACK_PRIORITY")?;
        return Ok(Some(UiEvent::NpcAttackPriority { value }));
    }
    if opcode == p::SET_PLAYER_OP {
        let mut r = PayloadReader::new(payload);
        let slot = r.g1_alt2()?;
        let cursor = {
            let v = r.g2_alt3()?;
            if v == 65535 {
                -1
            } else {
                v as i32
            }
        };
        let raw = r.gjstr()?;
        let name = if raw.eq_ignore_ascii_case("null") {
            None
        } else {
            Some(raw)
        };
        let deprioritised = r.g1_alt3()? == 0;
        r.finish("SET_PLAYER_OP")?;
        // The player-option packet reads the complete payload first, then ignores
        // an out-of-range slot; malformed framing is still rejected above.
        if !(1..=8).contains(&slot) {
            return Ok(None);
        }
        return Ok(Some(UiEvent::SetPlayerOp {
            slot,
            cursor,
            name,
            deprioritised,
        }));
    }
    let mut r = PayloadReader::new(payload);
    let open = match opcode {
        p::IF_SETTARGETPARAM => {
            anyhow::ensure!(payload.len() == 10, "IF_SETTARGETPARAM length");
            let packed = r.g4s()? as u32;
            let to = i32::from(r.g2()?);
            let param = i32::from(r.g2_alt2_u16()?);
            let from = i32::from(r.g2_alt2_u16()?);
            Some(UiEvent::SetTargetParam {
                packed,
                from: if from == 65535 { -1 } else { from },
                to: if to == 65535 { -1 } else { to },
                param,
            })
        }
        p::STORE_SERVERPERM_VARCS_ACK => {
            anyhow::ensure!(payload.is_empty(), "STORE_SERVERPERM_VARCS_ACK payload");
            Some(UiEvent::VarcAck)
        }
        p::IF_SETEVENTS => {
            let from = i32::from(r.g2_alt2_u16()?);
            let mask = r.g4s()?;
            let to = r.g2_alt3()? as i32;
            let packed = r.g4_alt1()? as u32;
            Some(UiEvent::SetEvents {
                packed,
                from: if from == 65535 { -1 } else { from },
                to: if to == 65535 { -1 } else { to },
                mask,
            })
        }
        p::IF_OPENTOP => {
            let k3 = r.g4_alt2()?;
            let k2 = r.g4_alt1()?;
            let k0 = r.g4_alt2()?;
            let k1 = r.g4s()?;
            r.g1()?;
            Some(UiEvent::OpenTop {
                interface_id: r.g2_alt3()? as u32,
                keys: [k0, k1, k2, k3],
            })
        }
        p::IF_OPENSUB => {
            let k2 = r.g4_alt2()?;
            let parent_packed = r.g4_alt1()? as u32;
            let kind = r.g1_alt2()?;
            let k3 = r.g4s()?;
            let sub_id = r.g2()? as u32;
            let k1 = r.g4_alt2()?;
            let k0 = r.g4_alt2()?;
            Some(UiEvent::OpenSub {
                parent_packed,
                sub_id,
                kind,
                keys: [k0, k1, k2, k3],
            })
        }
        p::IF_OPENSUB_ACTIVE_LOC => {
            let parent_packed = r.g4_alt1()? as u32;
            let coord = InterfaceCoord::unpack(r.g4_alt3()?);
            let sub_id = r.g2()? as u32;
            let kind = r.g1_alt3()?;
            let keys = [r.g4_alt3()?, r.g4s()?, r.g4_alt2()?, r.g4_alt3()?];
            let shape = r.g1()?;
            let id = r.g4s()?;
            anyhow::ensure!(
                shape & 128 == 0,
                "active-loc transform exceeds fixed 32-byte packet"
            );
            Some(UiEvent::OpenSubActive {
                parent_packed,
                sub_id,
                kind,
                keys,
                variant: ActiveVariant::Loc,
                binding: ActiveBinding::Loc {
                    coord,
                    shape: (shape >> 2 & 31) as i32,
                    angle: (shape & 3) as i32,
                    id,
                },
            })
        }
        p::IF_OPENSUB_ACTIVE_PLAYER => {
            let k0 = r.g4_alt2()?;
            let k2 = r.g4_alt3()?;
            let parent_packed = r.g4_alt2()? as u32;
            let k3 = r.g4s()?;
            let kind = r.g1()?;
            let index = r.g2()? as i32;
            let k1 = r.g4_alt3()?;
            let sub_id = r.g2_alt3()? as u32;
            Some(UiEvent::OpenSubActive {
                parent_packed,
                sub_id,
                kind,
                keys: [k0, k1, k2, k3],
                variant: ActiveVariant::Player,
                binding: ActiveBinding::Player { index },
            })
        }
        p::IF_OPENSUB_ACTIVE_NPC => {
            let sub_id = r.g2_alt3()? as u32;
            let k0 = r.g4_alt1()?;
            let k1 = r.g4_alt3()?;
            let k3 = r.g4_alt3()?;
            let index = r.g2_alt1()? as i32;
            let kind = r.g1()?;
            let k2 = r.g4s()?;
            let parent_packed = r.g4_alt1()? as u32;
            Some(UiEvent::OpenSubActive {
                parent_packed,
                sub_id,
                kind,
                keys: [k0, k1, k2, k3],
                variant: ActiveVariant::Npc,
                binding: ActiveBinding::Npc { index },
            })
        }
        p::IF_OPENSUB_ACTIVE_OBJ => {
            let sub_id = r.g2()? as u32;
            let k2 = r.g4_alt3()?;
            let coord = InterfaceCoord::unpack(r.g4_alt2()?);
            let parent_packed = r.g4_alt1()? as u32;
            let k3 = r.g4s()?;
            let id = r.g2_alt1()? as i32;
            let k0 = r.g4_alt3()?;
            let kind = r.g1_alt1()?;
            let k1 = r.g4_alt2()?;
            Some(UiEvent::OpenSubActive {
                parent_packed,
                sub_id,
                kind,
                keys: [k0, k1, k2, k3],
                variant: ActiveVariant::Obj,
                binding: ActiveBinding::Obj { coord, id },
            })
        }
        p::IF_CLOSESUB => Some(UiEvent::CloseSub {
            parent_packed: r.g4_alt2()? as u32,
        }),
        p::IF_MOVESUB => Some(UiEvent::MoveSub {
            source_packed: r.g4_alt2()? as u32,
            target_packed: r.g4_alt2()? as u32,
        }),
        p::IF_SETHIDE => {
            let packed = r.g4s()? as u32;
            let flag = r.g1_alt2()?;
            Some(UiEvent::SetHide {
                packed,
                hidden: flag == 1,
                flag,
            })
        }
        _ => None,
    };
    if open.is_some() {
        r.finish("interface packet")?;
        return Ok(open);
    }
    match opcode {
        crate::proto::server::WORLDLIST_FETCH_REPLY => {
            anyhow::ensure!(!payload.is_empty(), "empty world list reply");
            Ok(Some(UiEvent::WorldList {
                bytes: payload.to_vec(),
            }))
        }
        crate::proto::server::IF_SETTEXT => {
            let (packed, text) = parse_if_settext(payload)?;
            Ok(Some(UiEvent::SetText { packed, text }))
        }
        crate::proto::server::IF_SETPOSITION => {
            let (packed, x, y) = parse_if_setposition(payload)?;
            Ok(Some(UiEvent::SetPosition { packed, x, y }))
        }
        crate::proto::server::IF_SETSCROLLPOS => {
            let (packed, scroll_y) = parse_if_setscrollpos(payload)?;
            Ok(Some(UiEvent::SetScrollPos { packed, scroll_y }))
        }
        crate::proto::server::RUNCLIENTSCRIPT => {
            Ok(Some(UiEvent::RunScript(parse_runclientscript(payload)?)))
        }
        _ => Ok(None),
    }
}
