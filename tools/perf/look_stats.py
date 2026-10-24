#!/usr/bin/env python3
"""Image statistics of renderer frames against the look reference shots.

    look_stats.py REFDIR FRAMEDIR TAG...

REFDIR holds the reference shots (lumbridge-aerial|castle|castle-roof|street.webp),
FRAMEDIR the frames as `TAG-aerial|castle|roof|street.ppm` or `.png` (the views of
the `ref-*` scenes in modern_perf_bench.rs, converted from raw RGBA). One table per
view: the luminance mean and spread, the percentiles, the saturation, the colour
balance, the luminance and saturation of the top, middle and bottom thirds, and the
darkest over the brightest 5% (the shadow contrast).
"""
import sys, os
import numpy as np
from PIL import Image
R = sys.argv[1].rstrip('/') + '/'
S = sys.argv[2].rstrip('/')
refs = {'aerial': 'lumbridge-aerial.webp', 'castle': 'lumbridge-castle.webp', 'roof': 'lumbridge-castle-roof.webp', 'street': 'lumbridge-street.webp'}
def load(f):
    return np.asarray(Image.open(f).convert('RGB').resize((640, 360), Image.LANCZOS), dtype=np.float64) / 255.0
def lum(a): return 0.2126*a[...,0]+0.7152*a[...,1]+0.0722*a[...,2]
def sat(a):
    mx, mn = a.max(-1), a.min(-1); return np.where(mx > 0, (mx-mn)/np.maximum(mx, 1e-9), 0)
def feats(a):
    l = lum(a); H = l.shape[0]
    p = np.percentile(l, [5, 25, 50, 75, 95])
    th = [a[:H//3], a[H//3:2*H//3], a[2*H//3:]]
    d = {'mean': l.mean(), 'std': l.std(), 'p05': p[0], 'p25': p[1], 'p50': p[2], 'p75': p[3], 'p95': p[4],
         'sat': sat(a).mean(),
         'r/g': a[...,0].mean()/a[...,1].mean(), 'b/g': a[...,2].mean()/a[...,1].mean(),
         'topL': lum(th[0]).mean(), 'midL': lum(th[1]).mean(), 'botL': lum(th[2]).mean(),
         'topS': sat(th[0]).mean(), 'midS': sat(th[1]).mean(), 'botS': sat(th[2]).mean(),
         'p05/p95': p[0]/max(p[4], 1e-6)}
    return d
tags = sys.argv[3:]
for pose, rf in refs.items():
    rows = [('ref', feats(load(R + rf)))]
    for t in tags:
        for ext in ('ppm', 'png'):
            f = f'{S}/{t}-{pose}.{ext}'
            if os.path.exists(f):
                rows.append((t, feats(load(f))))
                break
    keys = list(rows[0][1].keys())
    print(f'== {pose}')
    print('  ' + ' '.join(f'{k:>6s}' for k in ['tag'] + keys))
    for n, d in rows:
        print('  ' + ' '.join([f'{n:>6s}'] + [f'{d[k]:6.3f}' for k in keys]))
