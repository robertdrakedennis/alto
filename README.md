# Alto

Alto is an open-source game client engine written in Rust for the MMORPG RuneScape at revision 910 (10 December
2019). The client reimplements the game client's observable behaviour (network
protocol, interface scripting, scene, audio, input) as our own engine. The
longer-term goal is an engine and studio: a client that can also load later
revisions, a modern renderer, and editors for interfaces, maps and scripts.

Alto is an independent project. It is not affiliated with or endorsed by Jagex
Ltd. RuneScape is a trademark of Jagex Ltd; the name is used here only to say
which game the software is compatible with. See [`NOTICE`](NOTICE).

## What is in this repository

| Path | What |
|---|---|
| `tools/client910/` | The Rust client: wgpu renderers, retained interface runtime, CS2 script host, audio, networking, session lifecycle. |
| `tools/native910/` | Rust library and CLI for the cache and CS2 scripts: decoders, encoders, VM, cross-reference, script project builds. |
| `tools/` | The Rust workspace, quality gates (`tools/refactor/`), performance tools (`tools/perf/`). |
| `docs/` | Contributor documentation (index: [`docs/README.md`](docs/README.md)). |

## What is not in this repository

The development server and its npm workspace are maintained privately and are
not included here. Online sessions need a separately supplied compatible
server; the client can also render offline from your own cache.

Nothing here ships game content. You supply your own data:

- **The game cache** (`server/data/cache/`, `server/data/pack/`, about 14 GB
  packed). It is Jagex's data. Provision it yourself from a revision 910 cache
  you have obtained (see [`docs/cache.md`](docs/cache.md)); both directories are
  gitignored.
- **Player data** written by the dev server (`server/data/players/`).
- Any original client, decompiled source, jars or shaders. The client is our
  own code and contains none of them.

Tests compare the client against recordings of protocol bytes, variable
state, CS2 stacks and UI streams that are committed as plain data
(`tools/client910/fixtures/`, `tools/native910/tests/fixtures/`). Tests that need the cache are reported as
ignored when built with `--features no-pack`.

## Quickstart

Requirements: Rust (pinned by `rust-toolchain.toml`), Python 3, and Git with Git LFS.
On Linux, install ALSA development headers and `pkg-config`.

Install Git LFS before cloning so the large replay recordings are downloaded.
For an existing clone, run `git lfs install` and `git lfs pull` before testing.

```bash
# Build the client. Game data is supplied separately; see docs/cache.md.
cargo build --release --manifest-path tools/client910/Cargo.toml

# Render offline from your packed cache.
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --offline

# For online play, supply your own compatible server and login public key.
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --host 127.0.0.1 --rsa-key-file /path/to/login-rsa.pub
```

### Renderers

The client draws with the modern renderer by default (`--renderer modern`):
shadows, ambient occlusion, a sky, scattering and a captured ambient light, over
the same game state. `--renderer faithful-gpu` (alias `classic`) draws with the
reference renderer, whose pixels the checks pin, and choosing the software
display mode in the settings (toolkit 0) draws with it too. On a high-DPI
display the modern renderer draws the scene at about 1600 x 1000 pixels and
scales it up (the interface stays sharp); `renderscale auto|50..200` in the
developer console sets and saves a choice. Details:
[`docs/renderer/modern-renderer.md`](docs/renderer/modern-renderer.md).

Full walkthrough: [`docs/setup.md`](docs/setup.md).

## Tests

```bash
cargo test --manifest-path tools/client910/Cargo.toml --lib     # add --features no-pack without a cache
cargo test --release --manifest-path tools/native910/Cargo.toml # add --features no-pack without a cache
tools/refactor/gate.sh --base main                              # the Rust quality gate (tools/refactor/README.md)
```

## Documentation

- [`docs/README.md`](docs/README.md): index
- [`docs/architecture.md`](docs/architecture.md): processes, crates and dependency rules
- [`docs/client-status.md`](docs/client-status.md): what works and what is partial
- [`docs/engineering.md`](docs/engineering.md): rules for changing the code
- [`docs/renderer/modern-renderer.md`](docs/renderer/modern-renderer.md): the modern renderer
- [`docs/engine-and-studio.md`](docs/engine-and-studio.md): where the project is going
- [`tools/README.md`](tools/README.md): the Rust workspace

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md), [`AGENTS.md`](AGENTS.md) and
[`docs/engineering.md`](docs/engineering.md).
The client must stay our own code: real names, typed state, engine-shaped
modules, and no code, identifiers, shader text or data copied from the
original game.

## License

MIT, see [`LICENSE`](LICENSE). Copyright (c) 2026 robert dennis (client, engine, tools). The MIT notice
retains prior project attribution. Third-party components are
listed in [`NOTICE`](NOTICE); the licence of every dependency is in
[`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

## Trademark

RuneScape is a trademark of Jagex Ltd. Alto is not affiliated with,
sponsored by or endorsed by Jagex Ltd. Alto contains no Jagex code or game
assets. By default the client connects to localhost; supply a compatible server
separately. Do not use it to access, or to interfere with, any service you are
not authorised to use.
