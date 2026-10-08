#!/usr/bin/env python3
"""Summarise an engine-profiler dump (`CLIENT910_PROFILE_OUT`, a
`--features profile` client; `rs910_core::profile` module docs for the CSV
format: `kind,frame,depth,name,start_ns,dur_ns`).

    python3 tools/perf/profsum.py DUMP.csv [--skip N] [--top K] [--spikes K]
                                           [--tree NAME] [--min-ms X]
    python3 tools/perf/profsum.py --compare BASE.csv NEW.csv [--skip N]
    python3 tools/perf/profsum.py DUMP.csv --chrome OUT.json

Default report:
- frame times over the frames after `--skip` (default 60: the load);
- the top scopes by inclusive time per frame (mean over all frames), with
  exclusive (self) time, calls per frame and the p95/max of the frames
  that ran it;
- the GPU passes (frames with timestamps only);
- the `--spikes` slowest frames with their scopes of at least `--min-ms`;
- `--tree NAME` (default `region change`): the full scope tree of every
  frame that ran NAME, e.g. each region change.
`--compare` prints each scope's mean ms/frame in both dumps (A/B evidence).
`--chrome` writes the Chrome trace-event JSON (chrome://tracing, Perfetto).
"""
import argparse
import collections
import csv
import json
import sys


class Frame:
    __slots__ = ("index", "start", "dur", "events", "gpu")

    def __init__(self, index, start, dur):
        self.index, self.start, self.dur = index, start, dur
        self.events = []  # (depth, name, start, dur), pre-order
        self.gpu = []  # (name, start, dur)


def load(path):
    frames = {}
    late_gpu = collections.defaultdict(list)
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            kind, index = row["kind"], int(row["frame"])
            start, dur = int(row["start_ns"]), int(row["dur_ns"])
            if kind == "frame":
                frames[index] = Frame(index, start, dur)
            elif kind == "scope":
                frames[index].events.append((int(row["depth"]), row["name"], start, dur))
            elif kind == "gpu":
                if index in frames:
                    frames[index].gpu.append((row["name"], start, dur))
                else:
                    late_gpu[index].append((row["name"], start, dur))
    for index, passes in late_gpu.items():
        if index in frames:
            frames[index].gpu.extend(passes)
    return [frames[i] for i in sorted(frames)]


def pct(values, q):
    if not values:
        return 0.0
    values = sorted(values)
    return values[min(len(values) - 1, int((len(values) - 1) * q / 100))]


def self_times(events):
    own = [e[3] for e in events]
    stack = []
    for i, (depth, _, _, dur) in enumerate(events):
        while stack and events[stack[-1]][0] >= depth:
            stack.pop()
        if stack:
            own[stack[-1]] -= dur
        stack.append(i)
    return own


def scope_stats(frames):
    """name -> dict(total, self, calls, per_frame[list of per-frame inclusive])."""
    stats = {}
    for frame in frames:
        own = self_times(frame.events)
        per = collections.Counter()
        ancestors = []
        for i, (depth, name, _, dur) in enumerate(frame.events):
            while ancestors and ancestors[-1][0] >= depth:
                ancestors.pop()
            nested = any(n == name for _, n in ancestors)
            ancestors.append((depth, name))
            s = stats.setdefault(name, {"total": 0, "self": 0, "calls": 0, "per": [], "depth": depth})
            s["depth"] = min(s["depth"], depth)
            s["calls"] += 1
            s["self"] += own[i]
            if not nested:
                s["total"] += dur
                per[name] += dur
        for name, dur in per.items():
            stats[name]["per"].append(dur)
    return stats


def gpu_stats(frames):
    stats = {}
    timed = [f for f in frames if f.gpu]
    for frame in timed:
        for name, _, dur in frame.gpu:
            stats.setdefault(name, []).append(dur)
    return stats, len(timed)


def ms(ns):
    return ns / 1e6


def tree_lines(frame, min_ns, max_depth=99):
    lines = [f"frame {frame.index}: {ms(frame.dur):.2f} ms, {len(frame.events)} scopes"]
    for depth, name, start, dur in frame.events:
        if dur >= min_ns and depth <= max_depth:
            lines.append(f"  {'  ' * depth}{name:<{max(1, 34 - 2 * depth)}} {ms(dur):9.3f} ms  @ {ms(start):8.2f}")
    for name, start, dur in frame.gpu:
        lines.append(f"  [gpu] {name:<28} {ms(dur):9.3f} ms")
    return lines


def report(frames, args):
    n = len(frames)
    durs = [f.dur for f in frames]
    print(f"{n} frames (after --skip {args.skip}): mean {ms(sum(durs) / max(n, 1)):.2f} ms, "
          f"p50 {ms(pct(durs, 50)):.2f}, p95 {ms(pct(durs, 95)):.2f}, "
          f"p99 {ms(pct(durs, 99)):.2f}, max {ms(max(durs, default=0)):.2f}")
    stats = scope_stats(frames)
    print(f"\ntop {args.top} scopes by inclusive ms per frame (mean over all {n} frames):")
    print(f"  {'scope':<26} {'ms/f':>8} {'self/f':>8} {'n/f':>6} {'frames':>6} {'p95':>8} {'max':>9}")
    for name, s in sorted(stats.items(), key=lambda kv: -kv[1]["total"])[: args.top]:
        print(f"  {'  ' * min(s['depth'], 3) + name:<26} {ms(s['total']) / n:8.3f} {ms(s['self']) / n:8.3f} "
              f"{s['calls'] / n:6.2f} {len(s['per']):6d} {ms(pct(s['per'], 95)):8.3f} {ms(max(s['per'])):9.3f}")
    gstats, timed = gpu_stats(frames)
    if timed:
        print(f"\nGPU passes ({timed} frames timed):")
        for name, d in sorted(gstats.items(), key=lambda kv: -sum(kv[1])):
            print(f"  {name:<26} mean {ms(sum(d) / len(d)):7.3f} ms  p95 {ms(pct(d, 95)):7.3f}  max {ms(max(d)):7.3f}")
        totals = [sum(p[2] for p in f.gpu) for f in frames if f.gpu]
        print(f"  {'(all passes)':<26} mean {ms(sum(totals) / len(totals)):7.3f} ms  p95 {ms(pct(totals, 95)):7.3f}")
    if args.spikes:
        print(f"\n{args.spikes} slowest frames (scopes >= {args.min_ms} ms):")
        for frame in sorted(frames, key=lambda f: -f.dur)[: args.spikes]:
            for line in tree_lines(frame, args.min_ms * 1e6, 3):
                print("  " + line)
    if args.tree:
        hits = [f for f in frames if any(e[1] == args.tree for e in f.events)]
        print(f"\nframes running {args.tree!r}: {len(hits)}")
        for frame in hits:
            for line in tree_lines(frame, args.min_ms * 1e6):
                print("  " + line)


def compare(base, new, args):
    a, b = scope_stats(base), scope_stats(new)
    na, nb = len(base), len(new)
    da = [f.dur for f in base]
    db = [f.dur for f in new]
    print(f"frames: {na} vs {nb}; frame mean {ms(sum(da) / max(na, 1)):.3f} -> {ms(sum(db) / max(nb, 1)):.3f} ms, "
          f"p50 {ms(pct(da, 50)):.3f} -> {ms(pct(db, 50)):.3f}")
    print(f"  {'scope':<26} {'base ms/f':>10} {'new ms/f':>10} {'delta':>8}")
    names = sorted(set(a) | set(b), key=lambda k: -max(a.get(k, {}).get("total", 0) / max(na, 1),
                                                         b.get(k, {}).get("total", 0) / max(nb, 1)))
    for name in names[: args.top]:
        x = ms(a.get(name, {}).get("total", 0)) / max(na, 1)
        y = ms(b.get(name, {}).get("total", 0)) / max(nb, 1)
        print(f"  {name:<26} {x:10.3f} {y:10.3f} {y - x:+8.3f}")


def chrome(frames, out):
    events = []
    for frame in frames:
        events.append({"name": f"frame {frame.index}", "ph": "X", "pid": 1, "tid": 1,
                       "ts": frame.start / 1e3, "dur": frame.dur / 1e3})
        for depth, name, start, dur in frame.events:
            events.append({"name": name, "ph": "X", "pid": 1, "tid": 1,
                           "ts": (frame.start + start) / 1e3, "dur": dur / 1e3, "args": {"depth": depth}})
        for name, start, dur in frame.gpu:
            events.append({"name": name, "ph": "X", "pid": 1, "tid": 2,
                           "ts": (frame.start + start) / 1e3, "dur": dur / 1e3,
                           "args": {"note": "GPU; start relative to the frame's first mark"}})
    with open(out, "w") as f:
        json.dump({"traceEvents": events, "displayTimeUnit": "ms"}, f)
    print(f"wrote {len(events)} events to {out}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dumps", nargs="+")
    ap.add_argument("--skip", type=int, default=60, help="frames to skip at the start (the load)")
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--spikes", type=int, default=5)
    ap.add_argument("--min-ms", type=float, default=0.5)
    ap.add_argument("--tree", default="region change")
    ap.add_argument("--compare", action="store_true")
    ap.add_argument("--chrome")
    args = ap.parse_args()
    loaded = [load(p)[args.skip:] for p in args.dumps]
    if args.chrome:
        chrome(loaded[0], args.chrome)
    elif args.compare:
        if len(loaded) != 2:
            sys.exit("--compare takes BASE and NEW")
        compare(loaded[0], loaded[1], args)
    else:
        for path, frames in zip(args.dumps, loaded):
            if len(args.dumps) > 1:
                print(f"== {path}")
            report(frames, args)


if __name__ == "__main__":
    main()
