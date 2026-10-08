# tools/perf: performance tools

Measurement scripts for the client and the modern renderer. None of them
edits the repository: each stages an instrumented copy of the tree under
`$CARGO_TARGET_DIR` (default `tools/target`) and builds it there. They need
the packed cache (`server/data/pack`). Each script's header comment documents
its arguments.

## Modern renderer

| Script | What |
|---|---|
| `modern_bench.sh build\|run\|check` | The headless matrix bench (scenes by size, anti-aliasing, shadows, far level and plan), with a regression guard against `modern-baseline.tsv`. Run it on every renderer change. Besides times it records the process's CPU work per frame (`mcycles`, `minstr`: cycles and instructions of every thread, which another process's load does not stretch as it does `draw`'s wall time), and with `MODERN_PERF_PASSCOUNTS=1` the draws and bind group sets of each pass (stderr). The hitches bench also writes the render thread's CPU time of `draw` (`draw_cpu_ms`). |
| `modern_bench.sh views`, `settings-views` | Render the offline views (or one settled frame per scene and settings combination) as raw RGBA, for byte comparison between two builds. |
| `modern_bench.sh client`, `modern_online.sh` | An online session on private ports with a perf client, then a screenshot and exit. |
| `modern_bench.sh ambient` | The per-square captured ambient's cost under the verified look: per frame the faces captured, CPU `draw`, wall and GPU times, until every square has its block. |
| `modern_bench.sh hitches` | Time the start, region changes and settings changes of one renderer (worst frames, pipelines compiled per frame). `MODERN_BENCH_COLD=1` uses a cold Metal shader cache. |
| `modern_bench_sum.py`, `modern_online_sum.py`, `modern_hitch_sum.py`, `modern_views_diff.py` | Tables, per-phase and per-pass medians, hitch reports and view diffs from those runs. |
| `modern_stage.py`, `modern_perf_bench.rs`, `modern_perf_hook.rs` | Staging and the instrumentation the scripts above use. |

The renderer's design is in [`docs/renderer/modern-renderer.md`](../../docs/renderer/modern-renderer.md).

## Client

| Script | What |
|---|---|
| `build.sh` | Build an instrumented (perf probe) release client from a tree without touching it. |
| `views.sh`, `bench.sh` | Per-frame statistics on five fixed-clock offline views and two online sessions. |
| `online.sh`, `profile-online.sh` | One online session with fresh dev servers on private ports; the second adds the engine profiler and two region changes. These and `views.sh` run the reference renderer (`--renderer faithful-gpu`) unless `RENDERER` says otherwise, so their numbers stay comparable with earlier runs. |
| `abtest.sh` | Interleaved A/B of two probe binaries so machine drift hits both alike. |
| `captures.sh` | Content traces of recorded UI sessions painted through the GPU painter. |
| `summarize.py`, `profsum.py`, `trcmp.py`, `substream.py`, `pxdiff.py`, `sample_tree.py`, `instrument.py` | Summaries and comparisons of the probe's rows, profiler dumps and traces, pixel differences, and macOS `sample` call trees. |

Guidelines: measure before and after with the same script and machine, use the
fixed clock, and put the numbers in the pull request.
