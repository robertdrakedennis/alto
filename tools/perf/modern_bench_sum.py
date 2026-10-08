#!/usr/bin/env python3
"""Tables and the regression guard for the modern bench (modern_bench.sh run).

    modern_bench_sum.py tables DIR TAG [TAG...]     markdown tables (base, shadows,
                                                 far, ablations, phases, passes)
    modern_bench_sum.py check SUMMARY.tsv BASELINE.tsv [TOL]
                                                 exit 1 when a configuration of
                                                 SUMMARY is slower than BASELINE
    modern_bench_sum.py online DIR PREFIX...        the client runs (modern_online.sh)

Several TAGs (repeat runs) are merged by configuration key: the median of
each timing column over the runs (counts are deterministic per scene).

The guard compares, per configuration key (scene, size, aa, shadows, far,
label): `cpu_total_p50` and `gpu_span_p50` against the baseline times
(1 + TOL) plus 0.5 ms (default TOL 0.15), and the counts `draws`, `passes`,
`set_bind_group`, `set_vb`, `allocs`, `creates`, `create_bind_group` against the
baseline times 1.02 plus 2 (they do not depend on the machine's load).
Timing is only meaningful at a low load average; the summary carries it.
"""
import csv
import statistics
import sys
from collections import defaultdict

KEY = ('scene', 'size', 'aa', 'shadows', 'far', 'label')
TIMES = ('draw_p50', 'draw_p99', 'finish_p50', 'submit_p50', 'idle_p50', 'wall_p50', 'wall_p99',
         'wall_max', 'gpu_span_p50', 'gpu_span_p99', 'gpu_sum_p50', 'cpu_total_p50', 'draw_min',
         'wall_min', 'new_ms', 'new_compile_ms', 'first_draw_ms', 'first_wall_ms', 'settle_max_ms',
         'settle_draw_max_ms', 'settle_total_ms')
COUNTS = ('draws', 'passes', 'set_bind_group', 'set_vb', 'allocs', 'creates', 'create_bind_group')


def rows(path):
    with open(path, newline='') as f:
        return list(csv.DictReader(f, delimiter='\t'))


def key(r):
    return tuple(r[k] for k in KEY)


def merge(paths):
    """Rows by key; timing columns the median over the runs."""
    by = defaultdict(list)
    order = []
    for p in paths:
        for r in rows(p):
            k = key(r)
            if k not in by:
                order.append(k)
            by[k].append(r)
    out = {}
    for k in order:
        rs = by[k]
        m = dict(rs[0])
        for c in TIMES:
            vals = [float(r[c]) for r in rs if r.get(c) not in (None, '')]
            if vals:
                m[c] = f'{statistics.median(vals):.3f}'
        m['runs'] = str(len(rs))
        m['loads'] = ' / '.join(r['load'].split()[0] for r in rs)
        m['ids'] = [r['id'] for r in rs]
        out[k] = m
    return out


def f(v, d=1):
    return f'{float(v):.{d}f}'


def tables(d, tags):
    summ = merge([f'{d}/{t}-summary.tsv' for t in tags])
    phases = defaultdict(lambda: defaultdict(list))
    passes = defaultdict(lambda: defaultdict(list))
    for t in tags:
        ids = {r['id']: key(r) for r in rows(f'{d}/{t}-summary.tsv')}
        try:
            for r in rows(f'{d}/{t}-phases.tsv'):
                phases[ids[r['id']]][r['phase']].append(float(r['p50']))
            for r in rows(f'{d}/{t}-passes.tsv'):
                passes[ids[r['id']]][r['pass']].append(float(r['inc_p50']))
        except FileNotFoundError:
            pass
    out = []
    out.append('### Base (MED shadows, far 2)\n')
    out.append('| scene | size | AA | CPU draw p50/p99 | +finish+submit | GPU span p50/p99 | serial wall p50/p99/max | draws | passes | set VB (same) | bind groups (same) | allocs/frame (MB) | Metal MB | load |')
    out.append('|---|---|---|---|---|---|---|---|---|---|---|---|---|---|')
    for k, r in summ.items():
        if r['plan'] != 'base':
            continue
        out.append(
            f"| {r['scene']} | {r['size']} | {r['aa']}x | {f(r['draw_p50'])} / {f(r['draw_p99'])} | {f(r['cpu_total_p50'])} | "
            f"{f(r['gpu_span_p50'])} / {f(r['gpu_span_p99'])} | {f(r['wall_p50'])} / {f(r['wall_p99'])} / {f(r['wall_max'])} | "
            f"{f(r['draws'], 0)} | {f(r['passes'], 0)} | {f(r['set_vb'], 0)} ({f(r['set_vb_same'], 0)}) | "
            f"{f(r['set_bind_group'], 0)} ({f(r['set_bind_group_same'], 0)}) | {f(r['allocs'], 0)} ({float(r['alloc_bytes']) / 1e6:.1f}) | "
            f"{f(r['metal_mb'], 0)} | {r['loads']} |")
    for plan, col, title in (('shadows', 'shadows', 'Shadow quality (far 2)'), ('far', 'far', 'Far level (MED shadows)')):
        out.append(f'\n### {title}: CPU draw p50 / GPU span p50 ms, draws\n')
        variants = []
        for k, r in summ.items():
            if r['plan'] in (plan, 'base') and r[col] not in variants:
                variants.append(r[col])
        if plan == 'far':
            variants.sort(key=lambda v: -1 if v == 'off' else int(v))
        else:
            orderq = ['off', 'low', 'med', 'high', 'ultra', 'ultra+']
            variants.sort(key=orderq.index)
        out.append('| scene | size | AA | ' + ' | '.join(variants) + ' |')
        out.append('|---|---|---|' + '---|' * len(variants))
        groups = defaultdict(dict)
        for k, r in summ.items():
            if r['plan'] not in (plan, 'base'):
                continue
            if plan == 'shadows' and r['far'] != '2':
                continue
            if plan == 'far' and r['shadows'] != 'med':
                continue
            groups[(r['scene'], r['size'], r['aa'])][r[col]] = r
        for (sc, sz, aa), g in groups.items():
            cells = []
            for v in variants:
                r = g.get(v)
                cells.append('-' if r is None else f"{f(r['draw_p50'])} / {f(r['gpu_span_p50'])}, {f(r['draws'], 0)}")
            out.append(f'| {sc} | {sz} | {aa}x | ' + ' | '.join(cells) + ' |')
    abl = [r for r in summ.values() if r['plan'] == 'ablate']
    if abl:
        out.append('\n### Ablations (one feature off): CPU draw p50 / GPU span p50 ms, draws\n')
        out.append('| scene | size | config | CPU draw | GPU span | draws | passes |')
        out.append('|---|---|---|---|---|---|---|')
        for r in summ.values():
            if r['plan'] in ('ablate',) or (r['plan'] == 'base' and any(a['scene'] == r['scene'] and a['size'] == r['size'] and a['aa'] == r['aa'] for a in abl)):
                out.append(f"| {r['scene']} | {r['size']} | {r['label']} ({r['aa']}x) | {f(r['draw_p50'])} | {f(r['gpu_span_p50'])} | {f(r['draws'], 0)} | {f(r['passes'], 0)} |")
    out.append('\n### Startup and hitches (per configuration: renderer creation, first frame, probe settle)\n')
    out.append('| scene | size | AA | config | new ms (compile) | first frame ms (compile, pipelines) | settle frames, max, total ms | RSS MB |')
    out.append('|---|---|---|---|---|---|---|---|')
    for r in summ.values():
        if r['plan'] != 'base':
            continue
        out.append(f"| {r['scene']} | {r['size']} | {r['aa']}x | {r['label']} | {f(r['new_ms'])} ({f(r['new_compile_ms'])}) | "
                   f"{f(r['first_wall_ms'])} ({f(r['first_compile_ms'])}, {r['first_pipelines']}) | {r['settle_frames']}, {f(r['settle_max_ms'])}, {f(r['settle_total_ms'])} | {f(r['rss_mb'], 0)} |")
    base = [k for k, r in summ.items() if r['plan'] == 'base']
    if base and phases:
        out.append('\n### CPU phases of `draw` (p50 ms; base configurations)\n')
        names = []
        for k in base:
            for n in phases[k]:
                if n not in names:
                    names.append(n)
        cols = [summ[k] for k in base]
        out.append('| phase | ' + ' | '.join(f"{c['scene'][:5]} {c['size'].split('x')[0]} {c['aa']}x" for c in cols) + ' |')
        out.append('|---|' + '---|' * len(cols))
        for n in names:
            vals = [statistics.median(phases[k][n]) if phases[k][n] else 0.0 for k in base]
            if max(vals) < 0.05:
                continue
            out.append(f'| {n} | ' + ' | '.join(f'{v:.2f}' for v in vals) + ' |')
        out.append('\n### GPU passes (incremental end p50 ms; base configurations)\n')
        pnames = []
        for k in base:
            for n in passes[k]:
                if n not in pnames:
                    pnames.append(n)
        out.append('| pass | ' + ' | '.join(f"{c['scene'][:5]} {c['size'].split('x')[0]} {c['aa']}x" for c in cols) + ' |')
        out.append('|---|' + '---|' * len(cols))
        for n in pnames:
            vals = [statistics.median(passes[k][n]) if passes[k][n] else 0.0 for k in base]
            out.append(f'| {n} | ' + ' | '.join(f'{v:.2f}' for v in vals) + ' |')
    print('\n'.join(out))


def check(summary, baseline, tol):
    base = {key(r): r for r in rows(baseline)}
    bad = 0
    for r in rows(summary):
        b = base.get(key(r))
        if b is None:
            continue
        for c in ('cpu_total_p50', 'gpu_span_p50'):
            lim = float(b[c]) * (1 + tol) + 0.5
            if float(r[c]) > lim:
                print(f"SLOWER {' '.join(key(r))}: {c} {float(r[c]):.2f} > {lim:.2f} (baseline {float(b[c]):.2f}; load {r['load']})")
                bad += 1
        for c in COUNTS:
            lim = float(b[c]) * 1.02 + 2
            if float(r[c]) > lim:
                print(f"MORE {' '.join(key(r))}: {c} {float(r[c]):.0f} > {lim:.0f} (baseline {float(b[c]):.0f})")
                bad += 1
    print(f'{bad} regressions' if bad else 'ok: no regression')
    return 1 if bad else 0


def online(d, prefixes):
    """The client runs: modern hook rows after the teleport has settled."""
    for p in prefixes:
        path = f'{d}/{p}-modern.tsv'
        try:
            rs = rows(path)
        except FileNotFoundError:
            print(f'{p}: no rows')
            continue
        rs = rs[len(rs) // 2:]  # the second half: Draynor, settled
        cpu = [float(r['cpu_ms']) for r in rs]
        span = [float(r['gpu_span_ms']) for r in rs if float(r['gpu_span_ms']) > 0]
        q = lambda v, x: sorted(v)[min(len(v) - 1, int(len(v) * x))] if v else 0.0
        mean = lambda c: sum(float(r[c]) for r in rs) / max(1, len(rs))
        print(f"{p}: {len(rs)} frames; draw CPU p50 {q(cpu, .5):.2f} p99 {q(cpu, .99):.2f} max {max(cpu, default=0):.1f}; "
              f"GPU span p50 {q(span, .5):.2f} p99 {q(span, .99):.2f}; draws {mean('draws'):.0f} passes {mean('passes'):.0f} "
              f"creates {mean('create_buffer') + mean('create_texture'):.1f} bind groups {mean('create_bind_group'):.1f} "
              f"writes {mean('write_buffer'):.0f} ({mean('write_buffer_bytes') / 1e6:.2f} MB)")


if __name__ == '__main__':
    cmd = sys.argv[1]
    if cmd == 'tables':
        tables(sys.argv[2], sys.argv[3:])
    elif cmd == 'check':
        sys.exit(check(sys.argv[2], sys.argv[3], float(sys.argv[4]) if len(sys.argv) > 4 else 0.15))
    elif cmd == 'online':
        online(sys.argv[2], sys.argv[3:])
    else:
        sys.exit(__doc__)
