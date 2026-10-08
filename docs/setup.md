# Setup

Build the Rust client and native tools for revision 910 (10 December 2019).
The development server is maintained separately and is not distributed here.

## Prerequisites

- Git with Git LFS. Run `git lfs install` before cloning.
- Rust, pinned by `rust-toolchain.toml` (rustup installs it on first use).
- Python 3 for the repository tooling.
- On Linux, ALSA development headers (`libasound2-dev`) and `pkg-config`.
- A revision 910 packed cache for rendering. Tests can run without it.

## Clone and build

```bash
git clone https://github.com/robertdrakedennis/alto.git
cd alto
git lfs pull
cargo build --release --manifest-path tools/client910/Cargo.toml
```

## Supply your cache and run

Alto ships no game data. Supply your own packed cache outside version control
(see [`cache.md`](cache.md)) and pass its directory with `--pack-root`.
The historical maintainer path `server/data/pack` remains supported and ignored.

```bash
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --offline
```

For an online session, supply a compatible server separately:

```bash
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --host 127.0.0.1 --rsa-key-file /path/to/login-rsa.pub
```

The client defaults to localhost. `--lobby-port` and `--world-port` select the
endpoints. The login public key is supplied with `--rsa-key-file` or
`--rsa-modulus` and `--rsa-exponent`; corresponding `ALTO_RSA_*` environment
variables are also supported. Use development credentials with your own server.
`--login-crypto off` is available for recordings and test peers using plain login
blocks. `--help` lists all options.

The modern renderer is the default. `--renderer faithful-gpu` selects the
reference renderer; `--renderer null` draws nothing. `--screenshot FILE` exports
a frame, and `--fixed-clock START_MS[:STEP_MS]` provides deterministic timing.

## Tests and quality checks

```bash
cargo test --manifest-path tools/Cargo.toml --features no-pack
python3 tools/revision/gen_protocol.py --check
python3 tools/refactor/test-inventory.py --mode no-pack --no-server --native-profile dev --quiet
tools/refactor/gate.sh --base main
```

Without the cache, `--features no-pack` reports cache-dependent tests as ignored.
See [`../tools/refactor/README.md`](../tools/refactor/README.md) for the full gate.
