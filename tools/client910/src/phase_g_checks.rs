//! Decoder/state robustness checks for the entity packet owners
//! (`protocol910`, `entities910`), formerly the stand-alone
//! `tests/phase-g` crate that compiled `#[path]` copies of these modules and
//! stopped building. They now run against the production modules in the
//! fast suite. Packet corpora (packet construction only, no expected
//! state) in `fixtures/phase-g/`.
#[path = "../tests/phase-g/checks.rs"]
mod checks;
#[path = "../tests/phase-g/context-checks.rs"]
mod context_checks;
#[path = "../tests/phase-g/defaults-checks.rs"]
mod defaults_checks;
#[path = "../tests/phase-g/provider-checks.rs"]
mod provider_checks;
#[path = "../tests/phase-g/rebuild-checks.rs"]
mod rebuild_checks;
#[path = "../tests/phase-g/titles-checks.rs"]
mod titles_checks;
#[path = "../tests/phase-g/types-checks.rs"]
mod types_checks;
#[path = "../tests/phase-g/varps-checks.rs"]
mod varps_checks;
