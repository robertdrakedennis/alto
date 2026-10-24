use super::*;
use crate::protocol910::live::{Contexts, Feed};
use tokio::io::AsyncWriteExt;

fn frame(op: u8, bytes: &[u8]) -> Vec<u8> {
    // The server message read with ISAAC disabled.
    let mut out = if op < 128 { vec![op] } else { vec![128, op] };
    match crate::proto::server::size(op).unwrap() {
        -2 => out.extend((bytes.len() as u16).to_be_bytes()),
        -1 => out.push(bytes.len() as u8),
        n => assert_eq!(n as usize, bytes.len()),
    }
    out.extend(bytes);
    out
}

#[tokio::test]
async fn phase_g_startup_preserves_prefix_npcs_and_coalesced_successor() {
    let (mut reader, mut writer) = tokio::io::duplex(16384);
    // Synthetic framing test only: entity payloads deliberately opaque here.
    // Decoder fidelity uses the recorded reference fixtures in run.sh.
    let mut rebuild = vec![0xab, 0xcd];
    rebuild.extend([1, 18, 5, 119, 0, 1, 18, 127]);
    let mut all = frame(88, &rebuild);
    all.extend(frame(186, &[0, 255, 254]));
    all.extend(frame(122, &[0xaa]));
    let successor = frame(122, &[0xbb]);
    all.extend(&successor);
    writer.write_all(&all).await.unwrap();
    let result = drain_frames(&mut reader, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(result.entities.pending_len(), 3);
    assert_eq!(result.entities.front().unwrap().payload, rebuild);
    assert_eq!(result.pending, successor);
}

#[test]
fn phase_g_strict_poll_retains_blocked_frame_and_unread_successor() {
    let mut pending = frame(88, &[1, 2, 3]);
    let next = frame(122, &[4, 5]);
    pending.extend(&next);
    let mut feed = Feed::default();
    let mut writes = vec![];
    let mut ui = vec![];
    let c = Contexts {
        rebuild: None,
        player: None,
        npc: None,
        zone: None,
    };
    assert!(drain_pending_entities(&mut pending, &mut writes, &mut ui, &mut feed, &c).is_err());
    assert_eq!(feed.front().unwrap().payload, [1, 2, 3]);
    assert_eq!(pending, next);
    assert!(writes.is_empty()); // no premature map-build acknowledgment
    assert!(drain_pending_entities(&mut pending, &mut writes, &mut ui, &mut feed, &c).is_err());
    assert_eq!(pending, next);
}

#[test]
fn phase_g_tick_boundary_stops_before_next_frame() {
    let mut pending = frame(129, &[]);
    let next = frame(83, &[]);
    pending.extend(&next);
    let mut feed = Feed::default();
    let mut writes = vec![];
    let mut ui = vec![];
    let c = Contexts {
        rebuild: None,
        player: None,
        npc: None,
        zone: None,
    };
    drain_pending_entities(&mut pending, &mut writes, &mut ui, &mut feed, &c).unwrap();
    assert_eq!(pending, next);
    assert!(writes.is_empty());
}

#[tokio::test]
async fn phase_g_strict_startup_defers_acknowledgment() {
    use tokio::io::AsyncReadExt;
    let (mut reader, mut writer) = tokio::io::duplex(16384);
    let rebuild = [1, 18, 5, 119, 0, 1, 18, 127];
    let next = frame(122, &[0xaa]);
    let mut all = frame(88, &rebuild);
    all.extend(&next);
    writer.write_all(&all).await.unwrap();
    let result = drain_frames_mode(&mut reader, Duration::from_secs(1), true)
        .await
        .unwrap();
    assert_eq!(result.entities.pending_len(), 1);
    assert_eq!(result.pending, next);
    drop(reader);
    let mut replies = vec![];
    writer.read_to_end(&mut replies).await.unwrap();
    // Display status belongs to the native canvas owner; map completion waits
    // for the CPU/GPU rebuild. Headless packet draining sends neither.
    assert!(replies.is_empty());
}
