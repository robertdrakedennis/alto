#!/bin/sh
# Required-corpus profile. Run from any directory; missing evidence is a failure.
set -eu
USAGE_ERROR_STATUS=2
case "$*" in
    ""|"--release") ;;
    *) echo "usage: verify.sh [--release]" >&2; exit "$USAGE_ERROR_STATUS" ;;
esac
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
cd "$ROOT"
OUT="tools/native910/target/verification"
mkdir -p "$OUT"
for archive in scripts interfaces config enum.config struct.config dbtableindex sprites models anims fontmetrics; do
    test -s "server/data/pack/client.$archive.js5" || {
        echo "required corpus missing: client.$archive.js5" >&2
        exit 1
    }
done
shasum -a 256 server/data/pack/client.scripts.js5 server/data/pack/client.interfaces.js5 \
    server/data/pack/client.config.js5 server/data/pack/client.enum.config.js5 \
    server/data/pack/client.dbtableindex.js5 > "$OUT/inputs.sha256"
cargo clippy --manifest-path tools/native910/Cargo.toml --all-targets
cargo test --manifest-path tools/native910/Cargo.toml "$@" -- --nocapture
cargo test --manifest-path tools/native910/Cargo.toml "$@" --test repack_scripts -- --ignored --nocapture
cargo test --manifest-path tools/native910/Cargo.toml "$@" --test project_build -- --ignored --nocapture
(cd server && node src/lostcity/tools/js5pack/verify-scripts.ts ../tools/native910/target/repack-verified/client.scripts.js5)
(cd server && node src/lostcity/tools/js5pack/verify-scripts.ts ../tools/native910/target/timer-acceptance/client.scripts.js5)
(cd server && node src/lostcity/tools/js5pack/verify-scripts.ts ../tools/native910/target/interfaces-verified/client.interfaces.js5)
echo "Verified corpus, recorded-trace core/timer/database fixtures and rebuilt packs. See $OUT."
