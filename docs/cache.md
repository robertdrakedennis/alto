# Cache

The client reads a revision 910 packed cache: a directory of
`client.<name>.js5` archive files, about 14 GB. Pass it with `--pack-root`.

```bash
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --offline
```

Raw cache archives need packing into this layout first.

## Tests

Tests that need the cache read it from `server/data/pack`. Build with
`--features no-pack` to skip them.

`native910` has the cache and CS2 tools; see
[`../tools/native910/README.md`](../tools/native910/README.md).
