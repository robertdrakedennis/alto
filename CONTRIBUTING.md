# Contributing

Thanks for wanting to help. Bug reports, fixes, features and docs are all
welcome, big or small.

## Reporting a bug

Open an issue with:

- what you did and what happened,
- what you expected instead,
- your OS and GPU, and a screenshot or log if you have one.

## Making a change

1. Fork the repo and create a branch.
2. Get the client running ([`docs/setup.md`](docs/setup.md)).
3. Make your change, and run the tests for the crates you touched:

   ```bash
   cargo test --manifest-path tools/Cargo.toml -p <crate>
   ```

   Add `--features no-pack` if you don't have a cache.
4. Run `tools/check.sh` (format and clippy). CI runs the same check.
5. Open a pull request saying what you changed and why. Screenshots help for
   anything visual, and before/after numbers help for performance changes.

For a large change, open an issue first so we can agree on the approach.

## Code style

- Use clear names and typed state. Write the behaviour in your own code rather
  than porting it line by line from another client.
- Packet bytes, login, cache formats and CS2 behaviour have to match 910
  exactly. Everything else just has to look and play right.
- Tests should check against something real, such as recorded packet bytes or
  the cache format, rather than restating the code.

[`docs/architecture.md`](docs/architecture.md) explains how the crates fit
together, and [`docs/engineering.md`](docs/engineering.md) has the full
conventions.

## License

By contributing, you agree that your work is licensed under the
[MIT license](LICENSE).
