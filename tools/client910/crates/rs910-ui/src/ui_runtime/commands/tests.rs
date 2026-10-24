//! The command table's own invariant.
use super::COMMANDS;

/// `Engine::dispatch_command` binary-searches [`COMMANDS`]: the names must be
/// sorted by byte order and unique.
#[test]
fn table_is_sorted_and_unique() {
    for pair in COMMANDS.windows(2) {
        assert!(
            pair[0].0.as_bytes() < pair[1].0.as_bytes(),
            "{} !< {}",
            pair[0].0,
            pair[1].0
        );
    }
}
