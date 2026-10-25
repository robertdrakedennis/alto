# Alto

At some point you just have to ship it.

Alto is a RuneScape 910 client built from the ground up in Rust. I made it to
make 910 easier to get into, and, with enough effort, to start a path toward an
open RS3 client one day.

It works with 910 servers that use the Java client's packets (NXT support is
coming). It's fully open source and ready for you to tinker with.

## What you get

- Full 910 CS2 support: interfaces and scripts run as they should.
- The whole network stack: login, game packets and cache streaming.
- An NXT-style look with my own vanilla+ take on it: shadows, sky, lighting.
- Speedups: batched loc chunks, cached shadows and a much longer draw distance.

It's probably not perfect, and there's likely a long list of bugs. If you find
one, open an issue or a pull request.

## Get it running

You need Rust (`rustup` installs the right version) and a revision 910 packed
cache ([`docs/cache.md`](docs/cache.md)). On Linux, also install
`libasound2-dev` and `pkg-config`.

```bash
git clone https://github.com/robertdrakedennis/alto.git
cd alto

# Look around offline using your cache
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --offline

# Connect to your own 910 server
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --host 127.0.0.1 --rsa-key-file /path/to/login-rsa.pub
```

`--help` lists every option. More detail is in [`docs/setup.md`](docs/setup.md).

## Find your way around

- `tools/client910/` is the client itself.
- `tools/native910/` holds the cache and CS2 tools.
- [`docs/`](docs/README.md) has everything else. Start with
  [`architecture.md`](docs/architecture.md), then
  [`client-status.md`](docs/client-status.md) to see what works and what doesn't.

## Contributing

Run `tools/check.sh` before you commit, and read
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## License and trademark

MIT, see [`LICENSE`](LICENSE). Third-party notices are in [`NOTICE`](NOTICE) and
[`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

RuneScape is a trademark of Jagex Ltd. Alto is not affiliated with Jagex.
