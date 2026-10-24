//! The system clipboard, as read by the `getclipboard` script command and
//! the developer console's paste/copy. The system clipboard is reached
//! through each platform's stock command-line bridge (no extra crates):
//! macOS `pbpaste`/`pbcopy`; Linux/BSD `wl-paste`/`wl-copy` under Wayland,
//! else `xclip` then `xsel`; Windows PowerShell `Get-Clipboard` /
//! `Set-Clipboard`. A missing bridge reads as an absent/empty clipboard.
use std::io::Write;
use std::process::{Command, Stdio};

/// `(program, args)` candidates for reading text, in preference order.
fn readers() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        vec![("/usr/bin/pbpaste", &[])]
    } else if cfg!(windows) {
        vec![(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Clipboard -Raw",
            ],
        )]
    } else {
        let mut v: Vec<(&'static str, &'static [&'static str])> = Vec::new();
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            v.push(("wl-paste", &["--no-newline"]));
        }
        v.push(("xclip", &["-selection", "clipboard", "-o"]));
        v.push(("xsel", &["--clipboard", "--output"]));
        v
    }
}

/// `(program, args)` candidates for writing text (text on stdin).
fn writers() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        vec![("/usr/bin/pbcopy", &[])]
    } else if cfg!(windows) {
        vec![(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Set-Clipboard -Value ([Console]::In.ReadToEnd())",
            ],
        )]
    } else {
        let mut v: Vec<(&'static str, &'static [&'static str])> = Vec::new();
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            v.push(("wl-copy", &[]));
        }
        v.push(("xclip", &["-selection", "clipboard", "-i"]));
        v.push(("xsel", &["--clipboard", "--input"]));
        v
    }
}

/// The clipboard's string flavour, or `None` when no clipboard is reachable
/// or it holds no text.
pub fn get_text() -> Option<String> {
    readers().into_iter().find_map(|(program, args)| {
        let out = Command::new(program)
            .args(args)
            // `pbpaste` writes the text in the locale's encoding; a non-UTF-8
            // locale turned non-ASCII text into bytes the strict decode below
            // rejected, so the string read as "".
            .env("LANG", "en_US.UTF-8")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        // The original client's clipboard read never failed on odd bytes
        // either, so decode lossily.
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        if cfg!(windows) {
            // PowerShell terminates its output with CRLF.
            if text.ends_with("\r\n") {
                text.truncate(text.len() - 2);
            }
        }
        Some(text)
    })
}

/// Puts `text` on the clipboard; returns whether a bridge accepted it.
pub fn set_text(text: &str) -> bool {
    writers().into_iter().any(|(program, args)| {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut input| input.write_all(text.as_bytes()).is_ok());
        child.wait().is_ok_and(|s| s.success()) && wrote
    })
}
