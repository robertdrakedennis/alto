# native910

The client's revision-910 cache codecs (JS5 containers, packs, configs,
interfaces, sprites, database tables) and the CS2 virtual machine with its
opcode book (`data/opcodes-910.txt`) and command semantics.

Tests read the packed cache from `server/data/pack` and the recordings in
`tests/fixtures/`. CI has no cache: `--features no-pack` reports the tests
that need it as ignored.

```sh
cargo test --release -p native910
```
