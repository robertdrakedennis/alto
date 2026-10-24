#!/usr/bin/env python3
"""Compare two perf_probe content traces (CLIENT910_PERF_TRACE).

usage: trcmp.py BASE NEW [--noise BASE2] [--context N]

The compared stream is the `S ` lines: render passes by attachment, every
draw by pipeline descriptor, viewport/scissor, bound contents and fetched
vertex stream, copies and submits (perf_probe.rs docs). It names no object
identity, so reusing buffers, bind groups, bundles or samplers does not
change it. Exit 0 when the streams are identical. Resource-creation lines
are only counted (they are what a performance change is expected to
reduce).

--noise BASE2: a second run of the base binary. Events at which BASE and
BASE2 differ (run-to-run noise of the base itself, e.g. the icon uploads of
the GPU capture replays) are not compared; the streams must still have the
same length and agree everywhere else.
"""
import sys
from collections import Counter

ctx = 3
args = sys.argv[1:]
noise = None
if '--noise' in args:
    i = args.index('--noise')
    noise = args[i + 1]
    del args[i:i + 2]
if '--context' in args:
    i = args.index('--context')
    ctx = int(args[i + 1])
    del args[i:i + 2]


def load(p):
    s, other = [], Counter()
    with open(p) as f:
        for line in f:
            line = line.rstrip('\n')
            if line.startswith('S '):
                s.append(line)
            elif line:
                other[line.split(' ', 1)[0]] += 1
    return s, other


(sa, oa), (sb, ob) = load(args[0]), load(args[1])
name = lambda p: p.split('/')[-1]
kinds = Counter(line.split(' ', 2)[1] for line in sa)
masked = set()
if noise:
    sn, _ = load(noise)
    if len(sn) != len(sa):
        sys.exit(f'noise run has {len(sn)} events, base {len(sa)}: not comparable')
    masked = {i for i, (x, y) in enumerate(zip(sa, sn)) if x != y}
    sa = ['*' if i in masked else x for i, x in enumerate(sa)]
    sb = ['*' if i in masked and i < len(sb) else x for i, x in enumerate(sb)]
ok = sa == sb
print(f'{name(args[0])} vs {name(args[1])}: {"IDENTICAL" if ok else "DIFFERENT"} content stream of '
      f'{len(sa)} vs {len(sb)} events ({dict(kinds)})'
      + (f'; {len(masked)} base-noise events masked' if noise else ''))
keys = sorted(set(oa) | set(ob))
print('  creations/writes: ' + ', '.join(f'{k} {oa[k]} -> {ob[k]}' for k in keys if oa[k] or ob[k]))
if not ok:
    n = next((i for i in range(min(len(sa), len(sb))) if sa[i] != sb[i]), min(len(sa), len(sb)))
    diff = sum(1 for x, y in zip(sa, sb) if x != y) + abs(len(sa) - len(sb))
    print(f'  first difference at event {n}; {diff} differing events')
    for k in range(max(0, n - ctx), min(n + ctx + 1, max(len(sa), len(sb)))):
        print('  A', sa[k][:300] if k < len(sa) else '-')
        print('  B', sb[k][:300] if k < len(sb) else '-')
sys.exit(0 if ok else 1)
