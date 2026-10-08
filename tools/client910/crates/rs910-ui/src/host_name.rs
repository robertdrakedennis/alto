//! `lastlogin`: the name of the host the account last connected from.
//!
//! The server sends the address as an IPv4 word. The name comes from a
//! reverse DNS lookup that runs in the background: until it answers the
//! scripts read an empty string, and afterwards the host name, or the dotted
//! address when the address has no name. A lookup that has not answered
//! within [`LOOKUP_TIMEOUT_MS`] of the logic clock is given up on the same
//! way (the dotted address), so a resolver that never answers cannot leave
//! the text empty for the whole session.
use std::net::Ipv4Addr;
use std::sync::mpsc::{channel, Receiver};

/// How long a lookup may run before the dotted address is used instead.
pub const LOOKUP_TIMEOUT_MS: i64 = 5000;

/// The reverse lookup: the name of the host at an address, if it has one.
pub type Resolver = fn(Ipv4Addr) -> Option<String>;

/// One address and its name, once it is known.
#[derive(Debug)]
pub struct HostName {
    address: Ipv4Addr,
    started: i64,
    pending: Option<Receiver<Option<String>>>,
    name: Option<String>,
}

impl HostName {
    /// Start resolving `packed` (the server's big-endian IPv4 word) with the
    /// system resolver, from logic-clock time `now`.
    #[must_use]
    pub fn lookup(packed: i32, now: i64) -> Self {
        Self::lookup_with(packed, now, reverse_lookup)
    }

    /// [`HostName::lookup`] with another resolver.
    #[must_use]
    pub fn lookup_with(packed: i32, now: i64, resolve: Resolver) -> Self {
        let address = Ipv4Addr::from(u32::from_be_bytes(packed.to_be_bytes()));
        let (answer, receive) = channel();
        let spawned = std::thread::Builder::new()
            .name("client910-host-name".into())
            .spawn(move || {
                let _ = answer.send(resolve(address));
            });
        if let Err(error) = spawned {
            log::warn!("[client910] host name lookup not started: {error}");
        }
        Self {
            address,
            started: now,
            pending: Some(receive),
            name: None,
        }
    }

    /// The text `lastlogin` reads at logic-clock time `now`: empty while the
    /// lookup runs.
    pub fn text(&mut self, now: i64) -> String {
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(found) => {
                    self.name = Some(found.unwrap_or_else(|| self.address.to_string()));
                    self.pending = None;
                }
                Err(_) if now - self.started > LOOKUP_TIMEOUT_MS => {
                    self.name = Some(self.address.to_string());
                    self.pending = None;
                }
                Err(_) => {}
            }
        }
        self.name.clone().unwrap_or_default()
    }
}

/// The system's reverse lookup of `address`: its host name, or `None` when
/// the address has no name (or this system cannot ask).
#[cfg(unix)]
pub fn reverse_lookup(address: Ipv4Addr) -> Option<String> {
    use std::mem::{size_of, zeroed};
    // SAFETY: `getnameinfo` reads the socket address passed and writes at
    // most `host.len()` bytes, NUL-terminated, into the buffer passed.
    unsafe {
        let mut target: libc::sockaddr_in = zeroed();
        target.sin_family = libc::AF_INET as _;
        target.sin_addr.s_addr = u32::from_ne_bytes(address.octets());
        #[cfg(target_os = "macos")]
        {
            target.sin_len = size_of::<libc::sockaddr_in>() as u8;
        }
        let mut host = [0 as libc::c_char; 1025];
        let status = libc::getnameinfo(
            std::ptr::addr_of!(target).cast(),
            size_of::<libc::sockaddr_in>() as libc::socklen_t,
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        );
        if status != 0 {
            return None;
        }
        std::ffi::CStr::from_ptr(host.as_ptr())
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

#[cfg(not(unix))]
pub fn reverse_lookup(_address: Ipv4Addr) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOPBACK_WORD: i32 = 0x7F00_0001;

    /// The text is empty until the lookup answers, then the name (or the
    /// dotted address for an address without one); a lookup that never
    /// answers falls back to the dotted address after the timeout.
    #[test]
    fn the_text_waits_for_the_lookup_then_falls_back_to_the_address() {
        fn named(_: Ipv4Addr) -> Option<String> {
            Some("home.example".into())
        }
        fn nameless(_: Ipv4Addr) -> Option<String> {
            None
        }
        fn never(_: Ipv4Addr) -> Option<String> {
            std::thread::sleep(std::time::Duration::from_secs(3));
            None
        }
        let wait_for = |lookup: &mut HostName, now: i64| {
            let started = std::time::Instant::now();
            loop {
                let text = lookup.text(now);
                if !text.is_empty() || started.elapsed().as_secs() > 5 {
                    return text;
                }
                std::thread::yield_now();
            }
        };
        assert_eq!(
            wait_for(&mut HostName::lookup_with(LOOPBACK_WORD, 0, named), 0),
            "home.example"
        );
        assert_eq!(
            wait_for(&mut HostName::lookup_with(LOOPBACK_WORD, 0, nameless), 0),
            "127.0.0.1"
        );
        let mut stuck = HostName::lookup_with(LOOPBACK_WORD, 1000, never);
        assert_eq!(stuck.text(1000), "", "empty while it runs");
        assert_eq!(stuck.text(1000 + LOOKUP_TIMEOUT_MS), "");
        assert_eq!(stuck.text(1001 + LOOKUP_TIMEOUT_MS), "127.0.0.1");
    }
}
