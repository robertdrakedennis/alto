#!/usr/bin/env python3
"""Summarize perf_probe stats rows (CLIENT910_PERF_STATS).

usage: summarize.py [--skip N] [--last N] A.tsv [B.tsv ...]
       summarize.py --compare [--skip N] BASE.tsv NEW.tsv
       summarize.py --table [--skip N] DIR BASE NEW NAME...
       summarize.py --ab [--skip N] DIR A B NAME...   (abtest.sh rounds)

`--table` prints one markdown row per NAME (DIR/BASE-NAME.tsv against
DIR/NEW-NAME.tsv) and metric: base -> new per frame.

Per file: the steady-state window (rows after --skip, default 60, or the
last --last rows), mean per frame of every column; CPU and GPU times also
as the median. `--compare` prints BASE, NEW and the change side by side.
"""
import statistics
import sys

args = sys.argv[1:]
skip, last, compare = 60, None, False
if '--skip' in args:
    i = args.index('--skip'); skip = int(args[i + 1]); del args[i:i + 2]
if '--last' in args:
    i = args.index('--last'); last = int(args[i + 1]); del args[i:i + 2]
if '--compare' in args:
    args.remove('--compare'); compare = True
table = '--table' in args
if table:
    args.remove('--table')
ab = '--ab' in args
if ab:
    args.remove('--ab')

TIMES = ('logic_cpu_ms', 'redraw_cpu_ms', 'redraw_wall_ms', 'gpu_ms')
ORDER = ['redraw_cpu_ms', 'logic_cpu_ms', 'gpu_ms', 'main_allocs', 'main_alloc_bytes', 'all_allocs',
         'buffers', 'buffer_bytes', 'bind_groups', 'samplers', 'textures', 'views', 'pipelines',
         'bundles', 'encoders', 'write_buffer', 'write_buffer_bytes', 'write_texture', 'submits',
         'passes', 'draws', 'set_pipeline', 'set_bind_group', 'create_buffer_us',
         'create_bind_group_us', 'write_buffer_us', 'write_texture_us', 'submit_us']


def load(path):
    with open(path) as f:
        rows = [l.rstrip('\n').split('\t') for l in f if l.strip()]
    head, body = rows[0], rows[1:]
    body = body[-last:] if last else body[skip:]
    cols = {h: [float(r[i]) for r in body] for i, h in enumerate(head)}
    out = {}
    for h, v in cols.items():
        if not v or h == 'frame':
            continue
        out[h] = (statistics.mean(v), statistics.median(v))
    out['_frames'] = (len(body), len(body))
    return out


def fmt(h, mv):
    mean, med = mv
    if h in TIMES:
        return f'{mean:.3f} (p50 {med:.3f})'
    return f'{mean:.1f}' if mean < 1e5 else f'{mean:.3g}'


TABLE = [('redraw_cpu_ms', 'redraw CPU ms'), ('logic_cpu_ms', 'logic CPU ms'), ('gpu_ms', 'GPU ms'),
         ('main_allocs', 'allocs'), ('main_alloc_bytes', 'alloc KB'), ('buffers', 'buffers'),
         ('bind_groups', 'bind groups'), ('samplers', 'samplers'), ('write_buffer', 'write_buffer'),
         ('submits', 'submits'), ('bundles', 'bundles')]
if ab:
    import glob, re
    d, na, nb, names = args[0], args[1], args[2], args[3:]
    def rounds(prefix, view):
        files = []
        for f in glob.glob(f'{d}/{prefix}*.tsv'):
            m = re.fullmatch(re.escape(f'{d}/{prefix}') + r'(\d+)-' + re.escape(view) + r'\.tsv', f)
            if m:
                files.append(f)
        return [load(f) for f in sorted(files)]
    print('| view | ' + ' | '.join(label for _, label in TABLE) + ' | rounds |')
    print('|---' * (len(TABLE) + 2) + '|')
    for n in names:
        ra, rb = rounds(na, n), rounds(nb, n)
        if not ra or not rb:
            continue
        cells = []
        for h, _ in TABLE:
            pick = lambda rs: statistics.median(r[h][1 if h in TIMES else 0] for r in rs)
            va, vb = pick(ra), pick(rb)
            if h == 'main_alloc_bytes':
                va, vb = va / 1024, vb / 1024
            f = (lambda v: f'{v:.2f}') if h in TIMES else (lambda v: f'{v:.0f}' if v >= 10 or v == int(v) else f'{v:.1f}')
            cells.append(f'{f(va)} → {f(vb)}')
        spread = lambda rs: '/'.join(f'{r["redraw_cpu_ms"][1]:.2f}' for r in rs)
        print(f'| {n} | ' + ' | '.join(cells) + f' | {spread(ra)} vs {spread(rb)} |')
elif table:
    d, base, new, names = args[0], args[1], args[2], args[3:]
    print('| view | ' + ' | '.join(label for _, label in TABLE) + ' |')
    print('|---' * (len(TABLE) + 1) + '|')
    for n in names:
        a, b = load(f'{d}/{base}-{n}.tsv'), load(f'{d}/{new}-{n}.tsv')
        cells = []
        for h, _ in TABLE:
            va, vb = a[h][1 if h in TIMES else 0], b[h][1 if h in TIMES else 0]
            if h == 'main_alloc_bytes':
                va, vb = va / 1024, vb / 1024
            f = (lambda v: f'{v:.2f}') if h in TIMES else (lambda v: f'{v:.0f}' if v >= 10 or v == int(v) else f'{v:.1f}')
            cells.append(f'{f(va)} → {f(vb)}')
        print(f'| {n} | ' + ' | '.join(cells) + ' |')
elif compare:
    a, b = load(args[0]), load(args[1])
    print(f'{"metric/frame":22} {"base":>24} {"new":>24} {"change":>9}   ({a["_frames"][0]} vs {b["_frames"][0]} frames)')
    for h in ORDER:
        if h not in a:
            continue
        ma, mb = a[h][0], b[h][0]
        ch = f'{(mb - ma) / ma * 100:+.0f}%' if ma else ('=' if mb == 0 else 'new')
        print(f'{h:22} {fmt(h, a[h]):>24} {fmt(h, b[h]):>24} {ch:>9}')
else:
    for p in args:
        s = load(p)
        print(f'== {p} ({s["_frames"][0]} frames)')
        for h in ORDER:
            if h in s:
                print(f'  {h:22} {fmt(h, s[h])}')
