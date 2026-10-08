//! Loadable-resource descriptors, native library names and progress state.

use super::LOADABLE_RESOURCES;

// ---------------------------------------------------------------------------
// Loadable resources and their loaders
// ---------------------------------------------------------------------------

/// A resource loader: archive, library (library name, whether the group is
/// fetched raw, whether it failed), file or group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loader {
    Archive(u32),
    Dll {
        name: &'static str,
        raw: bool,
        failed: bool,
    },
    File(u32, &'static str),
    Group(u32, i32),
}

pub(super) const fn dll(name: &'static str, raw: bool) -> Loader {
    Loader::Dll {
        name,
        raw,
        failed: false,
    }
}

/// The loadable resources with the loaders the load-progress
/// tracker installs.
pub(super) const LOADERS: [Loader; LOADABLE_RESOURCES] = [
    Loader::Archive(28),
    dll("jaclib", false),
    dll("jaggl", false),
    dll("jagdx", false),
    dll("sw3d", false),
    dll("RuneScape-Setup.exe", true),
    dll("hw3d", false),
    Loader::Archive(31),
    Loader::Archive(26),
    Loader::Archive(2),
    Loader::Archive(16),
    Loader::Archive(17),
    Loader::Archive(18),
    Loader::Archive(19),
    Loader::Archive(20),
    Loader::Archive(21),
    Loader::Archive(22),
    Loader::Archive(49),
    Loader::Archive(24),
    Loader::Archive(25),
    Loader::Archive(27),
    Loader::Archive(29),
    Loader::File(10, "huffman"),
    Loader::Archive(3),
    Loader::Archive(12),
    Loader::Archive(13),
    Loader::Group(23, 0),
];

/// The load-progress statics
/// with each resource length (set from the JS5 handshake).
#[derive(Clone, Debug)]
pub struct LoadableResources {
    pub lengths: [i32; LOADABLE_RESOURCES],
    pub(super) state: i32,
    pub(super) base: i32,
    pub(super) resources: Option<Vec<Loader>>,
}

impl Default for LoadableResources {
    fn default() -> Self {
        Self {
            lengths: [1; LOADABLE_RESOURCES],
            state: 0,
            base: 0,
            resources: None,
        }
    }
}

impl LoadableResources {
    /// Forget the resource load progress.
    pub fn reset(&mut self) {
        self.state = 0;
        self.base = 0;
    }
}

/// The base path of the native libraries.
pub(super) fn library_base_path() -> String {
    let os = match std::env::consts::OS {
        "windows" => "windows/",
        "linux" => "linux/",
        "macos" => "macos/",
        _ => "",
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64/",
        "x86" => "x86/",
        a if a.starts_with("powerpc") => "ppc/",
        _ => "universal/",
    };
    format!("{os}{arch}")
}

/// The platform file name of a native library.
pub(super) fn map_library_name(name: &str) -> Option<String> {
    match std::env::consts::OS {
        "windows" => Some(format!("{name}.dll")),
        "linux" => Some(format!("lib{name}.so")),
        "macos" => Some(format!("lib{name}.dylib")),
        _ => None,
    }
}
