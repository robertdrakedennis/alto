# cs2-commands

Recorded behaviour of the original client for `crates/rs910-ui/src/ui_command_spec.rs`
(per-command behaviour tables for the `ui_host_builtins` / `ui_host_game` CS2
command partitions and the engine-owned login/lobby/social/stockmarket commands).

- `recorded.tsv` — one line per case, recorded once by running the original
  client's command handler in a fresh process per case: final stacks, thrown
  exception class, observed client statics (outgoing packet bytes, component
  fields, camera/spline state, ...). The `rec()` rows of the Rust tables name
  the cases; the recording is data only and is not regenerated in this
  repository.
- `fault-classes.tsv` — `label<TAB>golden name` for each `rs910_core::fault::Fault`
  class, so the spec test can compare the failure class of a Rust error message
  with the exception class in `recorded.tsv`. Edit by hand.

Rows that depend on the host operating system (`ttv_library_request`,
`saveRuneScapeSetup`, `pushRuneScapeSetupValue` on the installer) were
recorded with the original client's `os.name` set to the named system and, for
the installer, a stub installer script beside a scratch cache directory. Rows
that run that script exist on Unix hosts only.

The `detailcanset_toolkit_default`, `detailcanmod_toolkit_default`,
`detailget_toolkit`, `detailget_toolkit_default` and `detail_toolkit_default`
rows were recorded with the original client's `os.name` set to Linux and its
native library loader holding no library (a host that never fetched the
DirectX library), with default options, or with a saved file that holds
toolkit 3.
