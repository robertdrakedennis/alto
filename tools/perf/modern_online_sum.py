#!/usr/bin/env python3
"""Per-phase CPU and per-pass GPU medians of an modern hook file (MODERN_PERF_OUT,
modern_online.sh's PREFIX-modern.tsv), over the frames after `--skip` (default: the
first half: the load and the region change).

    modern_online_sum.py FILE.tsv [--skip N]
"""
import csv
import statistics
import sys
from collections import defaultdict

path = sys.argv[1]
rows = list(csv.DictReader(open(path, newline=''), delimiter='\t'))
skip = int(sys.argv[sys.argv.index('--skip') + 1]) if '--skip' in sys.argv else len(rows) // 2
rows = rows[skip:]
ph, gp, inc = defaultdict(list), defaultdict(list), defaultdict(list)
for r in rows:
    seen = defaultdict(float)
    for item in filter(None, r['phases'].split(';')):
        n, v = item.rsplit('=', 1)
        seen[n] += float(v)
    for n, v in seen.items():
        ph[n].append(v)
    g = defaultdict(float)
    for item in filter(None, r['gpu'].split(';')):
        n, v = item.rsplit('=', 1)
        g[n] += float(v)
    for n, v in g.items():
        gp[n].append(v)
med = lambda v: statistics.median(v) if v else 0.0
q = lambda v, x: sorted(v)[min(len(v) - 1, int(len(v) * x))] if v else 0.0
cpu = [float(r['cpu_ms']) for r in rows]
span = [float(r['gpu_span_ms']) for r in rows if float(r['gpu_span_ms']) > 0]
print(f'{len(rows)} frames: draw CPU p50 {med(cpu):.2f} p99 {q(cpu, .99):.2f}; GPU span p50 {med(span):.2f} p99 {q(span, .99):.2f}')
for k in ('draws', 'passes', 'set_vertex_buffer', 'set_vertex_buffer_same', 'set_bind_group', 'set_bind_group_same',
          'create_buffer', 'create_buffer_bytes', 'create_texture', 'create_bind_group', 'write_buffer_bytes', 'create_pipeline',
          'allocs', 'mesh_cache', 'probe_captures'):
    if k not in rows[0]:
        continue
    v = [float(r[k]) for r in rows]
    print(f'  {k:24} mean {sum(v) / len(v):12.1f}  max {max(v):12.0f}')
print('CPU phases (p50 ms, p99):')
for n, v in sorted(ph.items(), key=lambda kv: -med(kv[1])):
    if med(v) >= 0.05 or q(v, .99) >= 1:
        print(f'  {n:44} {med(v):7.2f} {q(v, .99):7.2f}')
print('GPU passes (begin-end p50 ms; overlapping):')
for n, v in sorted(gp.items(), key=lambda kv: -med(kv[1])):
    print(f'  {n:44} {med(v):7.2f}')
