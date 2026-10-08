#!/usr/bin/env python3
"""Pixel difference of two screenshots (the GPU pixels' run-to-run noise
check: compare base against base first, then base against new).

usage: pxdiff.py A.png B.png [DIFF_MASK.png]

Prints the number of differing pixels, the largest channel difference and
the bounding box of the differences. Needs Pillow.
"""
import sys

from PIL import Image, ImageChops

a = Image.open(sys.argv[1]).convert('RGBA')
b = Image.open(sys.argv[2]).convert('RGBA')
if a.size != b.size:
    print('size', a.size, b.size)
    sys.exit(1)
d = ImageChops.difference(a, b)
px = list(d.getdata())
n = sum(1 for p in px if p != (0, 0, 0, 0))
mx = max(max(p) for p in px)
name = lambda p: p.split('/')[-1]
print(f'{name(sys.argv[1])} vs {name(sys.argv[2])}: {n} px differ, max {mx}, bbox {d.convert("RGB").getbbox()}')
if len(sys.argv) > 3:
    d.point(lambda v: 255 if v else 0).save(sys.argv[3])
