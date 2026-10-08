#!/usr/bin/env python3
"""Inclusive/self time per function of one thread from a macOS `sample` file.

usage: sample_tree.py SAMPLE.txt [--thread main] [--top N] [--under FUNC] [--match RE]
                      [--callers RE]

`sample PID SECONDS -file SAMPLE.txt` (Xcode command line tools) samples
every thread's stack each millisecond. This prints the functions of the
chosen thread (default: the main thread, where logic and redraw run) by
inclusive samples (a function counted once per stack, so recursion does not
double count), with their self samples, as a share of the thread's
samples (idle included: the redraw's acquire/present waits count). `--under FUNC` restricts to stacks through a function
whose name contains FUNC. `--callers RE` instead attributes the samples of
functions matching RE (e.g. `malloc|free|memmove`) to the nearest client
frame (`client910`/`rs910`) above them. Symbols are shortened (hash
suffixes dropped).
"""
import re
import sys
from collections import Counter

args = sys.argv[1:]
path = args.pop(0)
opts = {'--thread': 'main-thread', '--top': '60', '--under': None, '--match': None,
        '--callers': None}
while args:
    k = args.pop(0)
    opts[k] = args.pop(0)
thread_key = ': main' if opts['--thread'] in ('main', 'main-thread') else opts['--thread']

LINE = re.compile(r'^(?P<indent>[\s+!:|]*?)(?P<count>\d+) (?P<name>.+?)(?:\s+\(in (?P<lib>[^)]+)\))?(?:\s+\+ \d+)?(?:\s+\[[^\]]*\])?(?:\s+\S+:\d+)?\s*$')


def short(name):
    name = re.sub(r'::h[0-9a-f]{16}$', '', name)
    name = name.replace('_$LT$', '<').replace('$GT$', '>').replace('$u20$', ' ').replace('..', '::')
    return name[:140]


text = open(path).read()
start = text.find('Call graph:')
end = text.find('Total number in stack')
graph = text[start:end].split('\n')
threads, cur = {}, None
for line in graph[1:]:
    m = re.match(r'^\s+(\d+) Thread_\d+(.*)$', line)
    if m:
        cur = []
        threads[m.group(2)] = (int(m.group(1)), cur)
        continue
    if cur is not None and line.strip():
        cur.append(line)

chosen = [(k, v) for k, v in threads.items() if k.strip() == thread_key.strip() or (thread_key != ': main' and thread_key in k)]
if not chosen and thread_key == ': main':
    chosen = [(k, v) for k, v in threads.items() if 'com.apple.main-thread' in k]
if not chosen:
    sys.exit('thread not found; threads: ' + '; '.join(threads))
label, (total, lines) = chosen[0]
# Parse the tree: depth by the column where the count starts.
nodes = []  # (depth, count, name)
for line in lines:
    m = re.match(r'^([\s+!:|]*)(\d+) (.*)$', line)
    if not m:
        continue
    depth = len(m.group(1))
    rest = m.group(3)
    name = re.sub(r'\s+\(in [^)]+\).*$', '', rest).strip()
    nodes.append((depth, int(m.group(2)), short(name)))

IDLE = ('mach_msg2_trap', '__psynch_cvwait', 'semaphore_wait_trap', '__semwait_signal',
        '__workq_kernreturn', 'kevent', 'semaphore_timedwait_trap')
inclusive, self_c = Counter(), Counter()
stack = []  # (depth, name, count)
idle = 0
under = opts['--under']
for i, (d, c, n) in enumerate(nodes):
    while stack and stack[-1][0] >= d:
        stack.pop()
    stack.append((d, n, c))
    names = [s[1] for s in stack]
    if under and not any(under in x for x in names):
        continue
    # inclusive: attribute this node's count to its own name only when it is
    # the first occurrence of that name on the stack (recursion).
    if n not in names[:-1]:
        inclusive[n] += c
# self counts: count minus the sum of direct children.
for i, (d, c, n) in enumerate(nodes):
    j, kids = i + 1, 0
    child_depth = None
    while j < len(nodes) and nodes[j][0] > d:
        if child_depth is None:
            child_depth = nodes[j][0]
        if nodes[j][0] == child_depth:
            kids += nodes[j][1]
        j += 1
    self_c[n] += c - kids
    if any(x in n for x in IDLE):
        idle += c - kids
busy = total - idle
print(f'thread {label.strip()}: {total} samples, {busy} busy')
if opts['--callers']:
    target = re.compile(opts['--callers'])
    owners = Counter()
    stack = []
    for i, (d, c, n) in enumerate(nodes):
        while stack and stack[-1][0] >= d:
            stack.pop()
        stack.append((d, n))
        # self samples of a matching function go to its nearest client frame
        if target.search(n):
            j, kids, child_depth = i + 1, 0, None
            while j < len(nodes) and nodes[j][0] > d:
                if child_depth is None:
                    child_depth = nodes[j][0]
                if nodes[j][0] == child_depth:
                    kids += nodes[j][1]
                j += 1
            own = c - kids
            if own <= 0:
                continue
            owner = next((x for _, x in reversed(stack[:-1]) if ('client910' in x or 'rs910' in x)
                          and 'perf_probe' not in x), '?')
            owners[owner] += own
    print(f'samples in {opts["--callers"]} by nearest client frame: {sum(owners.values())}')
    for n, c in owners.most_common(int(opts['--top'])):
        print(f'{c:7} {100 * c / max(total, 1):5.1f}%  {n}')
    sys.exit(0)
match = re.compile(opts['--match']) if opts['--match'] else None
top = int(opts['--top'])
print('inclusive  self  function')
for n, c in inclusive.most_common():
    if match and not match.search(n):
        continue
    print(f'{c:7} {100 * c / max(total, 1):5.1f}% {self_c[n]:5}  {n}')
    top -= 1
    if top == 0:
        break
