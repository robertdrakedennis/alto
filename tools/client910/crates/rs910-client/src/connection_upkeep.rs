//! What the session does to keep its connections healthy: counting the
//! world's silence, the 30-second ping report and the round trip it measures.
//!
//! Everything is counted in logic cycles or read from the logic clock, so a
//! test or replay with a fixed clock sees exactly what a live session does.
//! The outgoing keepalive of both connections is
//! [`crate::loading_connection::IdleConnection`].
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// The cycles without a byte from the world after which the client treats
/// the connection as lost and reconnects.
pub const WORLD_SILENCE_CYCLES: i32 = 2250;

/// The world scene must have been drawn this many times in the current state
/// before its silence counts (the first frames of a session wait on the map).
pub const SILENCE_GRACE_DRAWS: i32 = 10;

/// Silence from the world, in logic cycles: reset by any byte read, advanced
/// once per cycle of the game state.
#[derive(Debug, Default)]
pub struct IncomingIdle(i32);

impl IncomingIdle {
    /// A byte arrived.
    pub fn heard(&mut self) {
        self.0 = 0;
    }

    /// One logic cycle of the game state: counts when the world has been
    /// drawn `state_ticks` times (more than [`SILENCE_GRACE_DRAWS`]); true
    /// once the silence is longer than [`WORLD_SILENCE_CYCLES`].
    pub fn tick(&mut self, state_ticks: i32) -> bool {
        if state_ticks > SILENCE_GRACE_DRAWS {
            self.0 = self.0.wrapping_add(1);
        }
        self.0 > WORLD_SILENCE_CYCLES
    }
}

/// How often the client reports its round trip, in milliseconds.
pub const PING_REPORT_INTERVAL_MS: i64 = 30_000;

/// What the round trip to the game host in a report is when nothing answers.
pub const PING_UNANSWERED_MS: i32 = 1000;

/// How the round trip to the game host is measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PingBackend {
    /// An ICMP echo, answered or not within a second.
    #[default]
    Echo,
    /// A fixed answer (tests, replays).
    Fixed(i32),
    /// Completion ingress supplied by a recorded session, with no worker.
    Recorded,
}

/// The `PING_STATISTICS` report: while the game state runs, a round trip to
/// the game host is measured, reported with the frame rate, and measured
/// again after [`PING_REPORT_INTERVAL_MS`].
#[derive(Debug, Default)]
pub struct PingReporter {
    backend: PingBackend,
    /// The logic-clock time the next measurement may start.
    next_at: i64,
    /// The measurement in flight.
    measuring: Option<PingMeasurement>,
}

#[derive(Debug)]
enum PingMeasurement {
    Worker(Receiver<i32>),
    Recorded(Option<i32>),
}

/// A worker result observed by the logic owner, rather than a wall-clock guess.
pub const PING_COMPLETION_RECORD_TAG: [u8; 4] = *b"PNGC";

impl PingReporter {
    #[must_use]
    pub fn with_backend(backend: PingBackend) -> Self {
        Self {
            backend,
            ..Self::default()
        }
    }

    /// A new session: the next cycle measures at once.
    pub fn reset(&mut self) {
        self.next_at = 0;
        self.measuring = None;
    }

    /// Supply one external completion to an already pending replay measurement.
    /// The ordinary cycle still encodes the report and advances its interval.
    pub fn complete_recorded(&mut self, round_trip: i32) -> anyhow::Result<()> {
        match self.measuring.as_mut() {
            Some(PingMeasurement::Recorded(answer)) if answer.is_none() => {
                anyhow::ensure!(
                    (0..=PING_UNANSWERED_MS).contains(&round_trip),
                    "recorded ping round trip is out of range"
                );
                *answer = Some(round_trip);
                Ok(())
            }
            _ => anyhow::bail!("recorded ping completion has no pending measurement"),
        }
    }

    /// A supplied completion must be consumed by the exact recorded cycle.
    pub fn has_recorded_completion(&self) -> bool {
        matches!(self.measuring, Some(PingMeasurement::Recorded(Some(_))))
    }

    /// One logic cycle of the game state at logic-clock time `now`: starts a
    /// measurement of `host` when one is due, and once one has answered
    /// appends the report to `out`.
    pub fn cycle(&mut self, now: i64, host: &str, fps: i32, out: &mut Vec<u8>) {
        let Some(measuring) = &mut self.measuring else {
            if now >= self.next_at {
                self.measuring = Some(measure(self.backend, host));
            }
            return;
        };
        let round_trip = match measuring {
            PingMeasurement::Worker(answer) => answer.try_recv().ok(),
            PingMeasurement::Recorded(answer) => answer.take(),
        };
        let Some(round_trip) = round_trip else {
            return;
        };
        crate::session_record::record(&PING_COMPLETION_RECORD_TAG, &round_trip.to_le_bytes());
        out.extend(crate::net::encode_ping_statistics(
            round_trip,
            fps,
            crate::net::NO_COLLECTOR_PERCENT,
        ));
        self.measuring = None;
        self.next_at = now + PING_REPORT_INTERVAL_MS;
    }
}

/// Start measuring the round trip to `host`.
fn measure(backend: PingBackend, host: &str) -> PingMeasurement {
    if backend == PingBackend::Recorded {
        return PingMeasurement::Recorded(None);
    }
    let (answer, receive) = channel();
    match backend {
        PingBackend::Fixed(round_trip) => {
            let _ = answer.send(round_trip);
        }
        PingBackend::Echo => {
            let host = host.to_owned();
            let spawned = std::thread::Builder::new()
                .name("client910-ping".into())
                .spawn(move || {
                    let _ = answer.send(echo_round_trip(&host));
                });
            if let Err(error) = spawned {
                log::warn!("[client910] ping measurement not started: {error}");
            }
        }
        PingBackend::Recorded => {
            unreachable!("recorded measurement returned before worker creation")
        }
    }
    PingMeasurement::Worker(receive)
}

/// The round trip of one ICMP echo to the first IPv4 address of `host`, in
/// milliseconds; [`PING_UNANSWERED_MS`] for a host that does not resolve or
/// answer within that time, or where no echo can be sent.
fn echo_round_trip(host: &str) -> i32 {
    let address = (host, 0).to_socket_addrs().ok().and_then(|mut found| {
        found.find_map(|address| match address {
            std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
            std::net::SocketAddr::V6(_) => None,
        })
    });
    address
        .and_then(|address| icmp_echo(address, Duration::from_millis(1000)))
        .map_or(PING_UNANSWERED_MS, |round_trip| {
            i32::try_from(round_trip.as_millis())
                .unwrap_or(PING_UNANSWERED_MS)
                .min(PING_UNANSWERED_MS)
        })
}

/// The ICMP echo checksum over `bytes`.
fn icmp_checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = bytes
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// The echo request the measurement sends: type 8, code 0, checksum, an
/// identifier and sequence number, and an eight-byte payload.
fn echo_request(identifier: u16, sequence: u16) -> [u8; 16] {
    let mut packet = [0u8; 16];
    packet[0] = 8;
    packet[4..6].copy_from_slice(&identifier.to_be_bytes());
    packet[6..8].copy_from_slice(&sequence.to_be_bytes());
    packet[8..].copy_from_slice(b"client91");
    let checksum = icmp_checksum(&packet);
    packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    packet
}

/// Whether `reply` is an echo reply, with or without the IPv4 header some
/// systems deliver in front of it.
fn is_echo_reply(reply: &[u8]) -> bool {
    let body = match reply.first() {
        Some(first) if first >> 4 == 4 => reply.get(usize::from(first & 0x0F) * 4..),
        _ => Some(reply),
    };
    body.and_then(|body| body.first()) == Some(&0)
}

#[cfg(unix)]
fn icmp_echo(address: Ipv4Addr, timeout: Duration) -> Option<Duration> {
    use std::mem::{size_of, zeroed};
    use std::os::fd::{FromRawFd, OwnedFd};
    // An unprivileged datagram ICMP socket (macOS, and Linux when the group
    // may ping).
    // SAFETY: plain system calls on a descriptor this function owns; every
    // pointer passed is to a live local of the stated size.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP);
        if fd < 0 {
            return None;
        }
        let socket = OwnedFd::from_raw_fd(fd);
        let raw = std::os::fd::AsRawFd::as_raw_fd(&socket);
        let wait = libc::timeval {
            tv_sec: timeout.as_secs() as _,
            tv_usec: timeout.subsec_micros() as _,
        };
        let set = libc::setsockopt(
            raw,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            std::ptr::addr_of!(wait).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        );
        if set != 0 {
            return None;
        }
        let mut target: libc::sockaddr_in = zeroed();
        target.sin_family = libc::AF_INET as _;
        target.sin_addr.s_addr = u32::from_ne_bytes(address.octets());
        #[cfg(target_os = "macos")]
        {
            target.sin_len = size_of::<libc::sockaddr_in>() as u8;
        }
        let request = echo_request(0, 1);
        let started = std::time::Instant::now();
        let sent = libc::sendto(
            raw,
            request.as_ptr().cast(),
            request.len(),
            0,
            std::ptr::addr_of!(target).cast(),
            size_of::<libc::sockaddr_in>() as libc::socklen_t,
        );
        if sent < 0 {
            return None;
        }
        let mut reply = [0u8; 128];
        while started.elapsed() < timeout {
            let read = libc::recv(raw, reply.as_mut_ptr().cast(), reply.len(), 0);
            if read < 0 {
                return None;
            }
            if is_echo_reply(&reply[..read as usize]) {
                return Some(started.elapsed());
            }
        }
        None
    }
}

#[cfg(not(unix))]
fn icmp_echo(_address: Ipv4Addr, _timeout: Duration) -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The silence only counts once the world was drawn enough times, resets
    /// on any byte, and expires on the 2251st quiet cycle.
    #[test]
    fn silence_expires_after_2250_counted_cycles() {
        let mut idle = IncomingIdle::default();
        for _ in 0..100 {
            assert!(!idle.tick(10), "the first draws do not count");
        }
        for _ in 0..WORLD_SILENCE_CYCLES {
            assert!(!idle.tick(11));
        }
        assert!(idle.tick(11), "the 2251st counted cycle");
        idle.heard();
        assert!(!idle.tick(11), "a byte starts the count again");
    }

    /// One report per 30 s of the logic clock: measured at once, reported
    /// with the frame rate when the answer arrives, then measured again after
    /// the interval.
    #[test]
    fn ping_reports_every_thirty_seconds() {
        let mut ping = PingReporter::with_backend(PingBackend::Fixed(37));
        let mut out = Vec::new();
        ping.cycle(0, "world", 50, &mut out);
        assert!(
            out.is_empty(),
            "the answer arrives a cycle after the request"
        );
        ping.cycle(20, "world", 50, &mut out);
        assert_eq!(out, crate::net::encode_ping_statistics(37, 50, -1));
        out.clear();
        for now in (40..30_020).step_by(20) {
            ping.cycle(now, "world", 50, &mut out);
        }
        assert!(out.is_empty(), "nothing before the interval is over");
        ping.cycle(30_020, "world", 49, &mut out);
        ping.cycle(30_040, "world", 49, &mut out);
        assert_eq!(out, crate::net::encode_ping_statistics(37, 49, -1));
        ping.reset();
        out.clear();
        ping.cycle(31_000, "world", 50, &mut out);
        ping.cycle(31_020, "world", 50, &mut out);
        assert_eq!(out.len(), 5, "a new session measures at once");
    }

    #[test]
    fn recorded_ping_completion_is_pending_once_and_due_by_the_normal_interval() {
        const LOGIC_INTERVAL_MS: i64 = 20;
        const COMPLETION_AT_MS: i64 = LOGIC_INTERVAL_MS * 2;
        const NEXT_REQUEST_AT_MS: i64 = COMPLETION_AT_MS + PING_REPORT_INTERVAL_MS;
        const ROUND_TRIP_MS: i32 = 37;
        const FRAME_RATE: i32 = 50;
        let mut ping = PingReporter::with_backend(PingBackend::Recorded);
        let mut out = Vec::new();
        assert!(
            ping.complete_recorded(ROUND_TRIP_MS).is_err(),
            "no request yet"
        );
        ping.cycle(0, "unused", FRAME_RATE, &mut out);
        ping.cycle(LOGIC_INTERVAL_MS, "unused", FRAME_RATE, &mut out);
        assert!(out.is_empty(), "no worker or guessed completion");
        ping.complete_recorded(ROUND_TRIP_MS).unwrap();
        assert!(
            ping.complete_recorded(ROUND_TRIP_MS + 1).is_err(),
            "one result per request"
        );
        ping.cycle(COMPLETION_AT_MS, "unused", FRAME_RATE, &mut out);
        assert!(!ping.has_recorded_completion());
        assert_eq!(
            out,
            crate::net::encode_ping_statistics(
                ROUND_TRIP_MS,
                FRAME_RATE,
                crate::net::NO_COLLECTOR_PERCENT
            )
        );
        out.clear();
        assert!(
            ping.complete_recorded(ROUND_TRIP_MS).is_err(),
            "interval has not started"
        );
        ping.cycle(NEXT_REQUEST_AT_MS - 1, "unused", FRAME_RATE - 1, &mut out);
        assert!(ping.complete_recorded(ROUND_TRIP_MS).is_err());
        ping.cycle(NEXT_REQUEST_AT_MS, "unused", FRAME_RATE - 1, &mut out);
        assert!(ping.complete_recorded(PING_UNANSWERED_MS + 1).is_err());
        ping.complete_recorded(0).unwrap();
        ping.cycle(
            NEXT_REQUEST_AT_MS + LOGIC_INTERVAL_MS,
            "unused",
            FRAME_RATE - 1,
            &mut out,
        );
        assert_eq!(
            out,
            crate::net::encode_ping_statistics(0, FRAME_RATE - 1, crate::net::NO_COLLECTOR_PERCENT)
        );
        ping.reset();
        assert!(
            ping.complete_recorded(ROUND_TRIP_MS).is_err(),
            "reset closes the old request"
        );
    }

    #[test]
    fn echo_request_has_a_valid_checksum() {
        let request = echo_request(7, 1);
        // The checksum of a packet that includes its own checksum is zero.
        assert_eq!(icmp_checksum(&request), 0);
        assert_eq!((request[0], request[1]), (8, 0));
        assert!(is_echo_reply(&[0, 0, 0, 0]));
        assert!(!is_echo_reply(&[8, 0, 0, 0]));
        let mut with_header = vec![0x45];
        with_header.extend([0u8; 19]);
        with_header.push(0);
        assert!(is_echo_reply(&with_header));
    }
}
