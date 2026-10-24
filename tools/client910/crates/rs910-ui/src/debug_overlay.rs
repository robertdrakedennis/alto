//! The FPS/debug overlay drawn in the debug component (clientcode 1405) while
//! the "displayfps" option is set, plus the connection byte counters it reads.
//!
//! Sources that have no equivalent in this process are mapped to the nearest
//! process-level measure and documented per field.

/// Total bytes sent/read on a connection and the per-50-cycle snapshots
/// (`out_per_second` / `in_per_second`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetStats {
    sent: i32,
    read: i32,
    pub out_per_second: i32,
    pub in_per_second: i32,
}

impl NetStats {
    /// Flushing a write adds the written bytes.
    pub fn wrote(&mut self, bytes: usize) {
        self.sent = self.sent.wrapping_add(bytes as i32);
    }
    /// The read counter advances by every byte the packet reader consumes;
    /// this port counts the socket bytes it reads into the same packet reader.
    pub fn read(&mut self, bytes: usize) {
        self.read = self.read.wrapping_add(bytes as i32);
    }
    /// Bytes written since the last one-second refresh (the headless session
    /// replay reads exactly this cycle's writes back from its loopback peer).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn sent(&self) -> i32 {
        self.sent
    }
    /// Called every main-loop cycle; snapshots the counters every 50 cycles.
    pub fn refresh(&mut self, loop_cycle: i32) {
        if loop_cycle % 50 == 0 {
            self.out_per_second = std::mem::take(&mut self.sent);
            self.in_per_second = std::mem::take(&mut self.read);
        }
    }
}

/// Everything the overlay reads besides the fonts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugStats {
    /// Current and averaged frame rate.
    pub fps: i32,
    pub fps_average: i32,
    /// Memory in KiB: this process has no managed heap;
    /// [`process_memory_kb`] reports its resident set as used and its peak
    /// resident set as total.
    pub mem_used_k: i64,
    pub mem_total_k: i64,
    pub game: NetStats,
    pub lobby: NetStats,
    /// Ping to the game/lobby server in milliseconds, -1 ("N/A") until
    /// measured (the game ping comes from the session's ping report).
    pub game_ping: i64,
    pub lobby_ping: i64,
    /// The toolkit's allocated buffer/texture bytes (buffers plus textures
    /// plus render targets). On Metal this is the device's `currentAllocatedSize`
    /// ([`crate::render::Renderer::allocated_bytes`]); other backends expose
    /// no allocation total through wgpu, which leaves it 0.
    pub offheap_bytes: i32,
    /// Cache totals over the archive providers that requested a prefetch:
    /// index size, verified, loaded.
    pub cache: [i32; 3],
}

impl Default for DebugStats {
    fn default() -> Self {
        Self {
            fps: 0,
            fps_average: 0,
            mem_used_k: 0,
            mem_total_k: 0,
            game: NetStats::default(),
            lobby: NetStats::default(),
            game_ping: -1,
            lobby_ping: -1,
            offheap_bytes: 0,
            cache: [0; 3],
        }
    }
}

/// One right-aligned string draw: text, colour, font (0 = the 11pt full
/// font, 1 = the 12pt full font), baseline offset from the component top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub colour: i32,
    pub font: usize,
    pub baseline: i32,
}

/// The `<col=rrggbb>` colour tag.
fn col_tag(rgb: i32) -> String {
    format!("<col={:x}>", rgb)
}

/// The overlay's ping text: "N/A" when unknown, highlighted above 500 ms.
fn ping_text(ping: i64) -> String {
    if ping == -1 {
        return "N/A".into();
    }
    let text = format!("{ping}ms");
    if ping > 500 {
        format!("{}{text}{}", col_tag(16711680), col_tag(16776960))
    } else {
        text
    }
}

/// English number formatting: `decimals` fixed-point digits taken from the
/// value, optional thousands grouping.
pub fn localised_en(mut value: i64, decimals: i32, grouping: bool) -> String {
    let negative = value < 0;
    if negative {
        value = -value;
    }
    let mut out: Vec<char> = Vec::new();
    if decimals > 0 {
        for _ in 0..decimals {
            let digit = value;
            value /= 10;
            out.push(char::from((digit - value * 10) as u8 + b'0'));
        }
        out.push('.');
    }
    let mut count = 0;
    loop {
        let digit = value;
        value /= 10;
        out.push(char::from((digit - value * 10) as u8 + b'0'));
        if value == 0 {
            if negative {
                out.push('-');
            }
            return out.into_iter().rev().collect();
        }
        if grouping {
            count += 1;
            if count % 3 == 0 {
                out.push(',');
            }
        }
    }
}

/// The overlay lines in draw order.
pub fn lines(stats: &DebugStats) -> Vec<Line> {
    let mut out = Vec::new();
    let mut y = 15;
    let yellow = -256;
    let red = -65536;
    out.push(Line {
        text: format!("Fps:{} ({} ms)", stats.fps, stats.fps_average),
        colour: if stats.fps < 20 { red } else { yellow },
        font: 1,
        baseline: y,
    });
    y += 15;
    out.push(Line {
        text: format!("Mem:{}/{}k", stats.mem_used_k, stats.mem_total_k),
        colour: if stats.mem_used_k > 262_144 {
            red
        } else {
            yellow
        },
        font: 1,
        baseline: y,
    });
    y += 15;
    for (name, net, ping) in [
        ("Game", stats.game, stats.game_ping),
        ("Lobby", stats.lobby, stats.lobby_ping),
    ] {
        out.push(Line {
            text: format!(
                "{name}: In:{}B/s Out:{}B/s Ping:{}",
                net.in_per_second,
                net.out_per_second,
                ping_text(ping)
            ),
            colour: yellow,
            font: 1,
            baseline: y,
        });
        y += 15;
    }
    let offheap = stats.offheap_bytes / 1024;
    out.push(Line {
        text: format!("Offheap:{offheap}k"),
        colour: if offheap > 65536 { red } else { yellow },
        font: 1,
        baseline: y,
    });
    y += 15;
    let [total, verified, loaded] = stats.cache.map(i64::from);
    let loaded_pct = if total == 0 { 0 } else { loaded * 100 / total };
    let verified_pct = if total == 0 {
        0
    } else {
        verified * 10000 / total
    };
    out.push(Line {
        text: format!(
            "Cache:{}% ({loaded_pct}%)",
            localised_en(verified_pct, 2, true)
        ),
        colour: yellow,
        font: 0,
        baseline: y,
    });
    out
}

/// Resident and peak-resident set of this process in KiB (the `Mem:` line's
/// used/total). `None` where the platform query is unavailable.
pub fn process_memory_kb() -> Option<(i64, i64)> {
    // SAFETY: getrusage writes the provided struct; zeroed is a valid init.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    // ru_maxrss is bytes on macOS and KiB on Linux.
    let peak = if cfg!(target_os = "macos") {
        usage.ru_maxrss / 1024
    } else {
        usage.ru_maxrss
    };
    let resident = resident_kb().unwrap_or(peak);
    Some((resident, peak.max(resident)))
}

#[cfg(target_os = "macos")]
fn resident_kb() -> Option<i64> {
    // SAFETY: proc_pidinfo fills at most `size` bytes of the task info struct.
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    let got = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    (got == size).then_some(info.pti_resident_size as i64 / 1024)
}

#[cfg(target_os = "linux")]
fn resident_kb() -> Option<i64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: i64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as i64;
    Some(pages * page / 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn resident_kb() -> Option<i64> {
    None
}
