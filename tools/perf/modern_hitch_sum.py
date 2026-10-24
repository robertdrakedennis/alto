#!/usr/bin/env python3
"""The hitches of one online session (lane P5): `modern_online.sh`'s
engine-profiler dump (PREFIX-prof.csv) and modern hook rows
(PREFIX-modern.tsv).

    modern_hitch_sum.py OUTDIR/PREFIX [--window N] [--min-ms X]

Reports, from the profiler's frames (the whole client frame: logic, the
Java redraw, the modern draw, present):
- the first scene frame (the first frame that hands a scene over, `R14
  scene handoff`, i.e. the modern renderer's first draw with its creation)
  and its time since the process's first frame;
- the region change (the frame with `region change`, the teleport) and the
  N frames from it (default 300): worst frame, p99, frames over 50 ms and
  the time over a 60 Hz budget;
- every frame of at least `--min-ms` (default 50) with its largest scopes;
- from the hook rows: the modern `draw` CPU of the worst frames and the
  pipelines created on the render thread (`create_pipeline - bg_pipelines`).
"""
import csv
import sys
from collections import defaultdict

prefix = sys.argv[1]
window = int(sys.argv[sys.argv.index('--window') + 1]) if '--window' in sys.argv else 300
min_ms = float(sys.argv[sys.argv.index('--min-ms') + 1]) if '--min-ms' in sys.argv else 50.0

frames = {}
scopes = defaultdict(list)
for r in csv.DictReader(open(prefix + '-prof.csv', newline='')):
    i = int(r['frame'])
    if r['kind'] == 'frame':
        frames[i] = (int(r['start_ns']), int(r['dur_ns']))
    elif r['kind'] == 'scope':
        scopes[i].append((int(r['depth']), r['name'], int(r['dur_ns']) / 1e6))
order = sorted(frames)
t0 = frames[order[0]][0]
ms = lambda i: frames[i][1] / 1e6


def has(i, name):
    return any(n == name for _, n, _ in scopes[i])


def top(i, k=6):
    s = sorted((v for v in scopes[i] if v[0] >= 1 and v[1] not in ('logic cycle', 'full redraw')), key=lambda v: -v[2])
    return ', '.join(f'{n} {d:.0f}' for _, n, d in s[:k])


def q(v, x):
    v = sorted(v)
    return v[min(len(v) - 1, int(len(v) * x))] if v else 0.0


first_scene = next((i for i in order if has(i, 'R14 scene handoff')), None)
if first_scene is not None:
    print(f'first scene frame {first_scene}: {ms(first_scene):.1f} ms, at {(frames[first_scene][0] - t0) / 1e9:.2f} s after the first frame; {top(first_scene)}')
    later = [ms(i) for i in order if first_scene < i < first_scene + 30]
    print(f'  next 30 frames: max {max(later):.1f} p50 {q(later, .5):.1f}')
regions = [i for i in order if has(i, 'region change')]
for rc in regions:
    win = [i for i in order if rc <= i < rc + window]
    v = [ms(i) for i in win]
    over = sum(max(0.0, x - 1000 / 60) for x in v)
    print(f'region change at frame {rc} ({(frames[rc][0] - t0) / 1e9:.2f} s): {ms(rc):.1f} ms ({top(rc)})')
    print(f'  {len(v)} frames from it: worst {max(v):.1f} ms, p99 {q(v, .99):.1f}, p50 {q(v, .5):.1f}, {sum(x > 50 for x in v)} over 50 ms, {over:.0f} ms over 60 Hz')
    worst = sorted(win, key=ms, reverse=True)[:5]
    print('  worst: ' + '; '.join(f'{i} (+{i - rc}) {ms(i):.0f} ms' for i in worst))
steady = [ms(i) for i in order[len(order) // 2:]]
print(f'second half: p50 {q(steady, .5):.1f}, p99 {q(steady, .99):.1f}, max {max(steady):.1f} ms')
print(f'frames of at least {min_ms:.0f} ms:')
for i in order:
    if ms(i) >= min_ms:
        print(f'  {i:5} {ms(i):8.1f}  {top(i)}')
try:
    rows = list(csv.DictReader(open(prefix + '-modern.tsv', newline=''), delimiter='\t'))
except FileNotFoundError:
    rows = []
if rows:
    cpu = [float(r['cpu_ms']) for r in rows]
    render = [int(r['create_pipeline']) + int(r['create_shader']) - int(r.get('bg_pipelines', 0) or 0) for r in rows]
    comp = [(int(r['shader_pipeline_ns']) - int(r.get('bg_compile_ns', 0) or 0)) / 1e6 for r in rows]
    print(f'modern draws: {len(rows)}; first draw CPU {cpu[0]:.1f} ms; render-thread compiles in draws: {sum(render)} ({sum(comp):.1f} ms)')
    for k in sorted(range(len(rows)), key=lambda k: -cpu[k])[:8]:
        ph = sorted(((p.rsplit("=", 1)[0], float(p.rsplit("=", 1)[1])) for p in rows[k]['phases'].split(';') if p), key=lambda p: -p[1])[:5]
        print(f'  draw {rows[k]["frame"]:>5} {cpu[k]:8.1f} ms  ' + ', '.join(f'{n} {v:.0f}' for n, v in ph))
