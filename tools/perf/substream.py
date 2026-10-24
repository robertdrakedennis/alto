#!/usr/bin/env python3
"""Compare the render passes with given labels between two content traces
per submission, for runs that are not deterministic as a whole (online
sessions): the post-process chain's draws depend on its targets and
uniforms, not on the scene drawn into them.

usage: substream.py BASE NEW LABEL...

Prints how many submissions carry the passes in each trace, how many
distinct per-submission sequences each has, and whether the sets of
sequences are equal (exit 0) or which are missing on either side.
"""
import sys
from collections import Counter

labels = tuple(f'S pass Some("{l}")' for l in sys.argv[3:])


def load(path):
    seqs, cur, inside = [], [], False
    for line in open(path):
        line = line.rstrip('\n')
        if not line.startswith('S '):
            continue
        if line == 'S submit':
            if cur:
                seqs.append(tuple(cur))
            cur, inside = [], False
        elif line.startswith('S pass'):
            inside = line.startswith(labels)
            if inside:
                cur.append(line)
        elif inside:
            cur.append(line)
    return Counter(seqs)


a, b = load(sys.argv[1]), load(sys.argv[2])
print(f'submissions with {sys.argv[3:]}: {sum(a.values())} vs {sum(b.values())}; '
      f'distinct sequences {len(a)} vs {len(b)}')
only_a, only_b = set(a) - set(b), set(b) - set(a)
print(f'  sequences only in base: {len(only_a)}, only in new: {len(only_b)}')
for s in list(only_b)[:2]:
    print('  new-only:', ' | '.join(x[:160] for x in s[:4]))
sys.exit(0 if not only_a and not only_b else 1)
