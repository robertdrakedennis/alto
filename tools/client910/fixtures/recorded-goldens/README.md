# Recorded goldens

Compact projections of recordings of the original client. The default
`cargo test` compares production Rust output against them. Each line is
`name len fnv64 [samples...]`. `len` is the word count of the recorded array
and `fnv64` is FNV-1a 64 over its words, sign-extended (`h ^= word as i64; h *=
0x100000001b3`). Arrays of up to 256 words keep every value; longer arrays
keep 16 evenly spaced samples. The reader lives in `src/recorded_golden.rs`.

The recordings were made once, outside this repository; they are data and are
not regenerated here.

| File | What was recorded | Rust test |
|---|---|---|
| `floor-oracle.txt` | colour tables, perlin, blend colours, trig tables and atan2 ties, water noise, camera and skybox view matrices | `core_goldens::{colour,trig}` (`hsl_*_match_the_recording`, `tables_radians_and_atan2_match_the_recording`, `atan2_rounds_negative_ties_up_like_the_recording`), `scene_goldens::maploader::perlin_and_blend_colours_match_the_recording`, `model_goldens::water::noise_heights_and_normal_volume_match_the_recording`, `scene_goldens::camera::{rotate_around_axis,classic_view_matrix}_matches_the_recording`, `scene_goldens::skybox::model_view_matches_the_recording` |
| `lumbridge-scene.txt` | floors, scene graph, static lights and environment of the 104x104 window around Lumbridge (3222,3222) | `scene_golden::lumbridge_scene_build_matches_the_recording` |
| `minimap-lumbridge.txt` | minimap wall and loc marks over that scene, player levels 0 and 1 (map scene icons excluded) | `scene_goldens::minimap::lumbridge_base_marks_match_the_recording` |
| `occlusion-raster.txt` | occlusion raster rows, 304 cases x 5 passes, and tile-visibility gates | `scene_goldens::occlusion_fixtures::raster_rows_match_the_recording` |
| `particles-torch.txt` | a torch emitter and an eviction/pool-reuse trace with fixed random sources | `model_goldens::particle::{torch_emitter_trace,slot_ring_eviction_and_pool_reuse}_match_the_recording` |
| `scene-draw-lumbridge.txt` | scene draw decisions for the recorded cameras | `scene_goldens::draw::lumbridge_draw_decisions_match_the_recording` |

Larger recordings of whole output streams live in `../recorded/`.
