//! The machine probe: operating system, memory, processor and, where the
//! platform exposes them, clock speed and processor names.
//!
//! The figures feed three consumers: the `os_physicalmemorysize` and
//! `detailget_canchoosesafemode` script queries, the capability profile the
//! client options rules read (memory budget, processor count) and the
//! hardware block of the login packets. A field the platform does not expose
//! stays 0 or empty, exactly as a probe that fails reports it, and
//! [`Hardware::default`] is that all-unknown machine (the deterministic value
//! tests use; only the shell calls [`probe`]).
//!
//! Sources: macOS `sysctlbyname`; Linux `/proc/meminfo`, `/proc/cpuinfo` and
//! the cpufreq sysfs node; Windows `GlobalMemoryStatusEx`, `RtlGetVersion` and
//! the processor registry key. Apple silicon reports no clock speed.

use std::sync::OnceLock;

/// What the probe found. Every field is "unknown" at its default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hardware {
    /// 1 Windows, 2 macOS, 3 Linux, 4 anything else.
    pub os_id: u8,
    /// The operating system is 64-bit.
    pub os_64bit: bool,
    /// The operating-system release code of the login block's table
    /// ([`windows_version_code`], [`mac_version_code`]); 0 when unlisted.
    pub os_version_code: u16,
    /// Memory the process may use, in megabytes: the figure the option rules
    /// compare against their thresholds. A quarter of the physical memory
    /// (the default budget of a managed runtime), 0 when memory is unknown.
    pub memory_budget_mb: u32,
    /// Logical processors.
    pub logical_cpus: u32,
    /// Installed physical memory in megabytes.
    pub ram_mb: u32,
    /// Nominal processor clock in megabytes per second (MHz); 0 when the
    /// platform does not expose it.
    pub cpu_mhz: u32,
    /// The graphics adapter's name; [`probe`] fills it once the shell has
    /// recorded it with [`set_gpu_description`].
    pub gpu_description: String,
    /// The processor vendor string ("GenuineIntel", "AuthenticAMD", ...).
    pub cpu_vendor: String,
    /// The processor's marketing name.
    pub cpu_description: String,
    /// Logical processors per package, when the processor reports it.
    pub cpu_logical_per_package: u8,
    /// The processor signature word, as the platform reports it; 0 when
    /// unavailable.
    pub cpu_signature: u32,
    /// The processor feature words: standard ecx, standard edx and extended
    /// edx; 0 when unavailable.
    pub cpu_features: [u32; 3],
}

impl Hardware {
    /// The memory budget as the option rules' `max_memory_mb` (they compare
    /// it against 96 and 245): the probe's figure, or 512 when memory is
    /// unknown.
    #[must_use]
    pub fn profile_memory_mb(&self) -> i32 {
        if self.memory_budget_mb == 0 {
            512
        } else {
            i32::try_from(self.memory_budget_mb).unwrap_or(i32::MAX)
        }
    }

    /// The processor count as the option rules' `cpu_count` (2 when unknown).
    #[must_use]
    pub fn profile_cpu_count(&self) -> i32 {
        if self.logical_cpus == 0 {
            2
        } else {
            i32::try_from(self.logical_cpus).unwrap_or(i32::MAX)
        }
    }
}

/// The graphics adapter's name, recorded once a device exists.
static GPU_DESCRIPTION: OnceLock<String> = OnceLock::new();

/// Records the graphics adapter's name (kept to the 40 characters the login
/// block allows). The first call wins.
pub fn set_gpu_description(name: &str) {
    let _ = GPU_DESCRIPTION.set(name.chars().take(40).collect());
}

/// The machine's figures: probed once per process, with the adapter name
/// once [`set_gpu_description`] has run.
#[must_use]
pub fn probe() -> Hardware {
    static PROBED: OnceLock<Hardware> = OnceLock::new();
    let mut hardware = PROBED.get_or_init(probe_now).clone();
    if let Some(gpu) = GPU_DESCRIPTION.get() {
        hardware.gpu_description.clone_from(gpu);
    }
    hardware
}

fn probe_now() -> Hardware {
    let mut hardware = platform::probe();
    hardware.logical_cpus = std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .unwrap_or(0);
    hardware.os_id = if cfg!(windows) {
        1
    } else if cfg!(target_os = "macos") {
        2
    } else if cfg!(target_os = "linux") {
        3
    } else {
        4
    };
    hardware.os_64bit = cfg!(target_pointer_width = "64");
    hardware.memory_budget_mb = (hardware.ram_mb / 4).min(u32::from(u16::MAX));
    hardware
}

/// The login block's Windows release code: 4.0 → 1, 4.1 → 2, 4.9 → 3,
/// 5.0 → 4, 5.1 → 5, 5.2 → 8, 6.0 → 6, 6.1 → 7, 6.2 → 9, 6.3 → 10,
/// 10.0 → 11; anything else 0.
#[must_use]
pub fn windows_version_code(major: u32, minor: u32) -> u16 {
    match (major, minor) {
        (4, 0) => 1,
        (4, 1) => 2,
        (4, 9) => 3,
        (5, 0) => 4,
        (5, 1) => 5,
        (5, 2) => 8,
        (6, 0) => 6,
        (6, 1) => 7,
        (6, 2) => 9,
        (6, 3) => 10,
        (10, 0) => 11,
        _ => 0,
    }
}

/// The login block's macOS release code: 10.4 → 20 through 10.11 → 27;
/// anything else (every later release) 0.
#[must_use]
pub fn mac_version_code(major: u32, minor: u32) -> u16 {
    if major == 10 && (4..=11).contains(&minor) {
        16 + minor as u16
    } else {
        0
    }
}

/// `MemTotal` of a `/proc/meminfo` text, in megabytes.
#[must_use]
pub fn parse_meminfo_mb(text: &str) -> u32 {
    text.lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse::<u64>().ok())
        .map_or(0, |kb| u32::try_from(kb / 1024).unwrap_or(u32::MAX))
}

/// The processor fields of a `/proc/cpuinfo` text: vendor, name, MHz of the
/// first processor entry that names them.
#[must_use]
pub fn parse_cpuinfo(text: &str) -> (String, String, u32) {
    let (mut vendor, mut name, mut mhz) = (String::new(), String::new(), 0);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "vendor_id" if vendor.is_empty() => vendor = value.to_owned(),
            "model name" | "Model" | "Hardware" if name.is_empty() => name = value.to_owned(),
            "cpu MHz" if mhz == 0 => mhz = value.parse::<f64>().map_or(0, |v| v as u32),
            _ => {}
        }
    }
    (vendor, name, mhz)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{mac_version_code, Hardware};
    use std::ffi::{c_char, c_int, c_void, CString};

    extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> c_int;
    }

    fn sysctl_bytes(name: &str) -> Option<Vec<u8>> {
        let name = CString::new(name).ok()?;
        let mut len = 0usize;
        // SAFETY: a null buffer asks for the size of the value only.
        let sized = unsafe {
            sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if sized != 0 || len == 0 {
            return None;
        }
        let mut out = vec![0u8; len];
        // SAFETY: `out` is `len` bytes, the size the first call reported.
        let read = unsafe {
            sysctlbyname(
                name.as_ptr(),
                out.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (read == 0).then(|| {
            out.truncate(len);
            out
        })
    }

    fn sysctl_u64(name: &str) -> Option<u64> {
        let bytes = sysctl_bytes(name)?;
        match bytes.len() {
            8 => Some(u64::from_ne_bytes(bytes.try_into().ok()?)),
            4 => Some(u64::from(u32::from_ne_bytes(bytes.try_into().ok()?))),
            _ => None,
        }
    }

    fn sysctl_string(name: &str) -> String {
        sysctl_bytes(name)
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .trim_end_matches('\0')
                    .to_owned()
            })
            .unwrap_or_default()
    }

    pub(super) fn probe() -> Hardware {
        let mut hardware = Hardware::default();
        if let Some(bytes) = sysctl_u64("hw.memsize") {
            hardware.ram_mb = u32::try_from(bytes >> 20).unwrap_or(u32::MAX);
        }
        // Intel Macs report the clock; Apple silicon does not.
        if let Some(hz) =
            sysctl_u64("hw.cpufrequency_max").or_else(|| sysctl_u64("hw.cpufrequency"))
        {
            hardware.cpu_mhz = u32::try_from(hz / 1_000_000).unwrap_or(u32::MAX);
        }
        hardware.cpu_description = sysctl_string("machdep.cpu.brand_string");
        hardware.cpu_vendor = sysctl_string("machdep.cpu.vendor");
        if let Some(signature) = sysctl_u64("machdep.cpu.signature") {
            hardware.cpu_signature = signature as u32;
        }
        // The kernel packs ecx above edx.
        if let Some(features) = sysctl_u64("machdep.cpu.feature_bits") {
            hardware.cpu_features[0] = (features >> 32) as u32;
            hardware.cpu_features[1] = features as u32;
        }
        if let Some(features) = sysctl_u64("machdep.cpu.extfeature_bits") {
            hardware.cpu_features[2] = features as u32;
        }
        let version = sysctl_string("kern.osproductversion");
        let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        hardware.os_version_code = mac_version_code(major, minor);
        hardware
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{parse_cpuinfo, parse_meminfo_mb, Hardware};

    pub(super) fn probe() -> Hardware {
        let mut hardware = Hardware::default();
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            hardware.ram_mb = parse_meminfo_mb(&text);
        }
        if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
            let (vendor, name, mhz) = parse_cpuinfo(&text);
            hardware.cpu_vendor = vendor;
            hardware.cpu_description = name;
            hardware.cpu_mhz = mhz;
        }
        if hardware.cpu_mhz == 0 {
            // The nominal ceiling, in kilohertz.
            hardware.cpu_mhz =
                std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq")
                    .ok()
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .map_or(0, |khz| khz / 1000);
        }
        hardware
    }
}

#[cfg(windows)]
mod platform {
    use super::{windows_version_code, Hardware};
    use std::ffi::c_void;

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            key: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            len: *mut u32,
        ) -> i32;
    }

    const LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const CPU_KEY: &str = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0";
    const REG_DWORD_ONLY: u32 = 0x10;
    const REG_SZ_ONLY: u32 = 0x02;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn registry_dword(value: &str) -> Option<u32> {
        let (key, name) = (wide(CPU_KEY), wide(value));
        let mut data = 0u32;
        let mut len = 4u32;
        // SAFETY: `data` is the four bytes a DWORD value needs.
        let status = unsafe {
            RegGetValueW(
                LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                REG_DWORD_ONLY,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut len,
            )
        };
        (status == 0).then_some(data)
    }

    fn registry_string(value: &str) -> String {
        let (key, name) = (wide(CPU_KEY), wide(value));
        let mut buffer = [0u16; 256];
        let mut len = (buffer.len() * 2) as u32;
        // SAFETY: `buffer` is `len` bytes.
        let status = unsafe {
            RegGetValueW(
                LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ_ONLY,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if status != 0 {
            return String::new();
        }
        let chars = (len as usize / 2).saturating_sub(1).min(buffer.len());
        String::from_utf16_lossy(&buffer[..chars]).trim().to_owned()
    }

    pub(super) fn probe() -> Hardware {
        let mut hardware = Hardware::default();
        let mut status = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        // SAFETY: `status` carries its own size, as the call requires.
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
            hardware.ram_mb = u32::try_from(status.total_phys >> 20).unwrap_or(u32::MAX);
        }
        let mut version = OsVersionInfo {
            size: std::mem::size_of::<OsVersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            service_pack: [0; 128],
        };
        // SAFETY: `version` carries its own size, as the call requires.
        if unsafe { RtlGetVersion(&mut version) } == 0 {
            hardware.os_version_code = windows_version_code(version.major, version.minor);
        }
        hardware.cpu_mhz = registry_dword("~MHz").unwrap_or(0);
        hardware.cpu_description = registry_string("ProcessorNameString");
        hardware.cpu_vendor = registry_string("VendorIdentifier");
        hardware
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::Hardware;

    pub(super) fn probe() -> Hardware {
        Hardware::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_and_cpuinfo_text_are_read() {
        let meminfo = "MemTotal:       16384256 kB\nMemFree:         1000 kB\n";
        assert_eq!(parse_meminfo_mb(meminfo), 16_000);
        assert_eq!(parse_meminfo_mb("nothing here"), 0);
        let cpuinfo = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Test CPU @ 3.00GHz\ncpu MHz\t\t: 2993.008\n\nprocessor\t: 1\nvendor_id\t: Other\ncpu MHz\t\t: 1.0\n";
        assert_eq!(
            parse_cpuinfo(cpuinfo),
            (
                "GenuineIntel".to_owned(),
                "Test CPU @ 3.00GHz".to_owned(),
                2993
            )
        );
    }

    #[test]
    fn release_codes_follow_the_login_tables() {
        assert_eq!(windows_version_code(10, 0), 11);
        assert_eq!(windows_version_code(6, 1), 7);
        assert_eq!(windows_version_code(7, 0), 0);
        assert_eq!(mac_version_code(10, 4), 20);
        assert_eq!(mac_version_code(10, 11), 27);
        assert_eq!(mac_version_code(11, 0), 0);
    }

    #[test]
    fn unknown_memory_keeps_the_default_profile() {
        let none = Hardware::default();
        assert_eq!(
            (none.profile_memory_mb(), none.profile_cpu_count()),
            (512, 2)
        );
        let box_16g = Hardware {
            ram_mb: 16_384,
            memory_budget_mb: 4096,
            logical_cpus: 8,
            ..Hardware::default()
        };
        assert_eq!(
            (box_16g.profile_memory_mb(), box_16g.profile_cpu_count()),
            (4096, 8)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    #[test]
    fn the_supported_platforms_report_memory_and_processors() {
        let hardware = probe_now();
        assert!(hardware.ram_mb > 0, "no physical memory reported");
        assert!(hardware.logical_cpus > 0);
        assert!(hardware.os_id >= 1 && hardware.os_64bit == cfg!(target_pointer_width = "64"));
        assert_eq!(hardware.memory_budget_mb, (hardware.ram_mb / 4).min(65_535));
    }
}
