//! Enables the maintainer-only session replays when their gitignored folder exists.
fn main() {
    const SESSIONS: &str = "fixtures/session-replay/rust/sessions.rs";
    println!("cargo::rustc-check-cfg=cfg(alto_sessions)");
    println!("cargo::rerun-if-changed={SESSIONS}");
    if std::path::Path::new(SESSIONS).exists() {
        println!("cargo::rustc-cfg=alto_sessions");
    }
}
