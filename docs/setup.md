# Setup

Build the Rust client and native tools for revision 910.

## Prerequisites

- Rust, pinned by `rust-toolchain.toml` (rustup installs it on first use).
- Python 3 for the repository tooling.
- On Linux, ALSA development headers (`libasound2-dev`) and `pkg-config`.
- A revision 910 packed cache for rendering. Tests can run without it.

## Clone and build

```bash
git clone https://github.com/robertdrakedennis/alto.git
cd alto
cargo build --release --manifest-path tools/client910/Cargo.toml
```

## Supply your cache and run

Pass your packed cache directory with `--pack-root` (see [`cache.md`](cache.md)).

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
variables are also supported. A content server that signs its JS5 master index
needs its public key as `--js5-rsa-modulus` and `--js5-rsa-exponent`; without
them the master index is read unsigned. Use development credentials with your
own server.
`--login-crypto off` is available for recordings and test peers using plain login
blocks. `--help` lists all options.

The modern renderer draws the scene; `--renderer null` draws nothing. `--screenshot FILE` exports
a frame, and `--fixed-clock START_MS[:STEP_MS]` provides deterministic timing.

## Tests and quality checks

```bash
cargo test --manifest-path tools/Cargo.toml --features no-pack
tools/check.sh
```

Without the cache, `--features no-pack` reports cache-dependent tests as ignored.
