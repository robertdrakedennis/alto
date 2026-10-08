# Cache provisioning

Alto does not distribute game data. Supply your own revision 910 packed cache
and keep it outside version control. The private development server and its
packing tools are not included in this public repository.

## Packed input

The client reads the packed JS5 archives from the directory passed with
`--pack-root`, for example `/path/to/pack`. A pack typically uses
`client.<name>.js5` archive files and is about 14 GB. An already packed cache
can be copied or mounted at that path. The historical maintainer location
`server/data/pack` is still supported; all of `server/` is now gitignored.

Use a cache matching revision 910. The client expects that revision's archive
formats, configurations, interfaces and CS2 scripts. Raw cache archives require
separately supplied packing tools before use with this layout.

```bash
cargo run --release --manifest-path tools/client910/Cargo.toml -- \
  --pack-root /path/to/pack --offline
```

## Native tools and tests

`native910` provides cache and CS2 decoding, verification and repacking tools;
see [`../tools/native910/README.md`](../tools/native910/README.md).
Cache-dependent tests use the historical local pack path and are ignored with
`--features no-pack`. The public suite also includes committed protocol,
variable-state, CS2 and UI recordings that need no server runtime.

Never commit the cache, packed archives, extracted game assets or player data.
