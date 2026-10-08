#!/bin/sh
# GLSL-versus-WGSL pixel comparisons (macOS, desktop GPU, clang++, the 910
# cache). The original cache shaders run through OpenGL (the C++ references in
# this directory); the production WGSL runs through wgpu. Nothing here needs a
# JDK or the original client sources.
#
#   sh tools/oracle/run-gpu-pixels.sh
#
# Honours CARGO_TARGET_DIR. Run the wired-RAM guard first: each step launches a
# GPU test process.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/tools/target}"
OUT="$ROOT/tools/client910/target/gpu-pixels"
CM="$ROOT/tools/client910/Cargo.toml"
GPU_CM="$ROOT/tools/client910/crates/rs910-render-gpu/Cargo.toml"
CXX="clang++ -std=c++17 -Wno-deprecated-declarations"
rm -rf "$OUT"
mkdir -p "$OUT/sprite" "$OUT/text" "$OUT/material"

cargo build --offline -q --release --manifest-path "$CM" --bin client910
"$TARGET/release/client910" --pack-root "$ROOT/server/data/pack" --dump-archive shaders "$OUT/shaders"

# Sprite and text quads: the export tests write the case inputs and the port's
# quads (equal to the recorded client's, which the same tests assert); the C++
# reference draws those quads with the cache GLSL.
for kind in sprite text; do
  case "$kind" in
    sprite) VAR=CLIENT910_SPRITE_DRAW_REPLAY; EXPORT=sprite_draw_oracle::export; PIXELS=sprite_draw_oracle::pixels ;;
    text) VAR=CLIENT910_TEXT_RENDER_REPLAY; EXPORT=text_render_oracle::export; PIXELS=text_render_oracle::pixels ;;
  esac
  DIR="$OUT/$kind"
  env "$VAR=$DIR" cargo test --offline --manifest-path "$CM" --lib "$EXPORT" -- --exact --nocapture
  cp "$DIR/rust-quads.bin" "$DIR/java-quads.bin"
  cp "$OUT/shaders/1/42.bin" "$DIR/vertex.glsl"
  cp "$OUT/shaders/1/43.bin" "$DIR/fragment.glsl"
  $CXX -framework OpenGL "$ROOT/tools/oracle/$kind-pixels.cpp" -o "$DIR/reference"
  "$DIR/reference" "$DIR"
  env "$VAR=$DIR" cargo test --offline --manifest-path "$CM" --lib "$PIXELS" -- --ignored --exact --nocapture
done

# Floor and model materials.
$CXX "$ROOT/tools/oracle/material-pixels.cpp" -framework OpenGL -o "$OUT/material/reference"
"$OUT/material/reference" "$OUT/shaders/1" "$OUT/material/reference.rgba"
CLIENT910_MATERIAL_REFERENCE="$OUT/material/reference.rgba" cargo test --offline --manifest-path "$GPU_CM" --lib floor_render::tests -- --include-ignored --nocapture
echo "PASS GLSL pixel comparisons (sprite, text, material)"
