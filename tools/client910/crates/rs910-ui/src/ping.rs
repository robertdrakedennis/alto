//! The server ping the debug overlay shows: an ICMP echo to the game or
//! lobby host once a second, its round trip in milliseconds.
//!
//! A [`Pinger`] owns a thread that resolves the host and pings it while the
//! host is set. A failed ping (a host that does not answer, a system that
//! does not allow ping sockets) keeps the last value; before the first answer
//! the value is -1, which the overlay shows as "N/A".
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one ping waits for the answer.
const TIMEOUT: Duration = Duration::from_millis(10_000);

/// The pause between two pings.
const INTERVAL: Duration = Duration::from_millis(1_000);

/// An ICMP echo request: type 8, the identifier, the sequence number and the
/// payload, with its checksum.
#[must_use]
pub fn echo_request(ident: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![8, 0, 0, 0];
    packet.extend(ident.to_be_bytes());
    packet.extend(seq.to_be_bytes());
    packet.extend(payload);
    let sum = checksum(&packet);
    packet[2..4].copy_from_slice(&sum.to_be_bytes());
    packet
}

/// The internet checksum of `data`.
#[must_use]
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Whether `received` is the echo reply to the request with sequence number
/// `seq`. Some systems hand the reply over with its IP header.
#[must_use]
pub fn is_reply_to(received: &[u8], seq: u16) -> bool {
    let icmp = match received.first() {
        Some(first) if first >> 4 == 4 => {
            let header = usize::from(first & 0xF) * 4;
            received.get(header..).unwrap_or(&[])
        }
        _ => received,
    };
    icmp.len() >= 8 && icmp[0] == 0 && icmp[1] == 0 && icmp[6..8] == seq.to_be_bytes()
}

/// One ping to `address`: the round trip in whole milliseconds.
#[cfg(unix)]
fn ping_once(address: Ipv4Addr, seq: u16) -> std::io::Result<i64> {
    use std::io::{Error, ErrorKind};
    // SAFETY: plain socket calls on a descriptor this function owns and
    // closes on every path; the buffers outlive the calls that use them.
    unsafe {
        let socket = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP);
        if socket < 0 {
            return Err(Error::last_os_error());
        }
        let result = (|| {
            let wait = libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            };
            if libc::setsockopt(
                socket,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&raw const wait).cast(),
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            ) < 0
            {
                return Err(Error::last_os_error());
            }
            let mut target: libc::sockaddr_in = std::mem::zeroed();
            target.sin_family = libc::AF_INET as libc::sa_family_t;
            target.sin_addr.s_addr = u32::from(address).to_be();
            let request = echo_request(std::process::id() as u16, seq, b"alto-ping");
            let started = Instant::now();
            if libc::sendto(
                socket,
                request.as_ptr().cast(),
                request.len(),
                0,
                (&raw const target).cast(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            ) < 0
            {
                return Err(Error::last_os_error());
            }
            let mut buffer = [0u8; 128];
            while started.elapsed() < TIMEOUT {
                let read = libc::recv(socket, buffer.as_mut_ptr().cast(), buffer.len(), 0);
                if read > 0 && is_reply_to(&buffer[..read as usize], seq) {
                    return Ok(started.elapsed().as_millis() as i64);
                }
            }
            Err(Error::from(ErrorKind::TimedOut))
        })();
        libc::close(socket);
        result
    }
}

#[cfg(not(unix))]
fn ping_once(_address: Ipv4Addr, _seq: u16) -> std::io::Result<i64> {
    Err(std::io::ErrorKind::Unsupported.into())
}

struct Shared {
    host: Mutex<Option<String>>,
    /// Bumped by every host change, so the thread resolves the new host.
    generation: AtomicU32,
    rtt: AtomicI64,
    running: AtomicBool,
}

/// A host's ping, kept up to date by a thread of its own.
pub struct Pinger {
    shared: Arc<Shared>,
}

impl Default for Pinger {
    fn default() -> Self {
        Self::new()
    }
}

impl Pinger {
    /// A pinger with no host yet.
    #[must_use]
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            host: Mutex::new(None),
            generation: AtomicU32::new(0),
            rtt: AtomicI64::new(-1),
            running: AtomicBool::new(true),
        });
        let thread = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("ping".into())
            .spawn(move || run(&thread))
            .expect("the ping thread starts");
        Self { shared }
    }

    /// Ping `host` from now on; the value is unknown until it answers.
    pub fn set_host(&self, host: Option<&str>) {
        let mut current = self.shared.host.lock().expect("ping host");
        if current.as_deref() == host {
            return;
        }
        *current = host.map(str::to_owned);
        self.shared.rtt.store(-1, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// The last round trip in milliseconds, -1 before the first answer.
    #[must_use]
    pub fn rtt(&self) -> i64 {
        self.shared.rtt.load(Ordering::Acquire)
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Release);
    }
}

fn resolve(host: &str) -> Option<Ipv4Addr> {
    (host, 0)
        .to_socket_addrs()
        .ok()?
        .find_map(|a| match a.ip() {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        })
}

fn run(shared: &Shared) {
    let mut seen = u32::MAX;
    let mut address = None;
    let mut seq = 0u16;
    while shared.running.load(Ordering::Acquire) {
        let generation = shared.generation.load(Ordering::Acquire);
        if generation != seen {
            seen = generation;
            let host = shared.host.lock().expect("ping host").clone();
            address = host.as_deref().and_then(resolve);
        }
        if let Some(address) = address {
            seq = seq.wrapping_add(1);
            if let Ok(rtt) = ping_once(address, seq) {
                // A host changed meanwhile has no use for this answer.
                if shared.generation.load(Ordering::Acquire) == seen {
                    shared.rtt.store(rtt, Ordering::Release);
                }
            }
        }
        // Sleep in short steps so a dropped pinger stops soon.
        let until = Instant::now() + INTERVAL;
        while shared.running.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request is an echo with a valid checksum, and the reply is
    /// recognised by its sequence number with or without an IP header.
    #[test]
    fn echo_request_and_reply_recognition() {
        let request = echo_request(0x1234, 7, b"abc");
        assert_eq!(&request[..2], &[8, 0]);
        assert_eq!(&request[4..8], &[0x12, 0x34, 0, 7]);
        // A packet that carries its own checksum sums to zero.
        assert_eq!(checksum(&request), 0);
        let mut reply = request.clone();
        reply[0] = 0;
        assert!(is_reply_to(&reply, 7));
        assert!(!is_reply_to(&reply, 8), "another request's reply");
        assert!(!is_reply_to(&request, 7), "an echo request is not a reply");
        let mut with_header = vec![
            0x45, 0, 0, 40, 0, 0, 0, 0, 64, 1, 0, 0, 127, 0, 0, 1, 127, 0, 0, 1,
        ];
        with_header.extend(&reply);
        assert!(is_reply_to(&with_header, 7));
        assert!(!is_reply_to(&[], 7));
    }

    /// With no host the value stays unknown; a host that does not resolve
    /// never gives one.
    #[test]
    fn a_pinger_without_an_answer_stays_unknown() {
        let pinger = Pinger::new();
        assert_eq!(pinger.rtt(), -1);
        pinger.set_host(Some("host.invalid"));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(pinger.rtt(), -1);
        pinger.set_host(None);
        assert_eq!(pinger.rtt(), -1);
    }
}
