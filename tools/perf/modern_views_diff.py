#!/usr/bin/env python3
"""Strict byte comparison of complete canonical `modern_bench.sh views` captures.

usage: modern_views_diff.py DIR_A TAG_A DIR_B TAG_B [REPEAT_DIR_A REPEAT_TAG_A]

A repeated baseline is a separate exact check. It never relaxes the comparison
between builds. Canonical views also provide capture manifests, whose frame IDs,
clocks, dimensions and settled state must agree.
"""
import argparse
import csv
import sys
from pathlib import Path

RGBA_CHANNELS = 4
EXIT_SUCCESS = 0
EXIT_DIFFERENT = 1
REPEAT_PAIR_ARGUMENTS = 2
MINIMUM_CAPTURE_EXTENT = 1
FIRST_CAPTURE_FRAME_ID = 1
NO_DIFFERENCES = 0
ONE_PIXEL = 1
MANIFEST_COMPLETION_FIELDS = 2
MANIFEST_FIELDS = ("frame", "width", "height", "frame_id", "clock_ms", "pending")


def frames(directory, tag):
    root = Path(directory)
    prefix = f"{tag}-"
    captures = {
        path.name.removeprefix(prefix): path
        for path in root.iterdir()
        if path.is_file() and path.name.startswith(prefix) and path.suffix == ".rgba"
    }
    if not captures:
        raise ValueError(f"{root} {tag}: no RGBA frames")
    manifest_path = root / f"{tag}-manifest.tsv"
    if not manifest_path.is_file():
        raise ValueError(f"{manifest_path}: completed canonical capture manifest is required")
    metadata = {}
    with manifest_path.open(newline="") as source:
        rows = list(csv.reader(source, delimiter="\t"))
    if not rows or tuple(rows.pop(NO_DIFFERENCES)) != MANIFEST_FIELDS:
        raise ValueError(f"{manifest_path}: invalid manifest columns")
    if (not rows or len(rows[-ONE_PIXEL]) != MANIFEST_COMPLETION_FIELDS
            or rows[-ONE_PIXEL][NO_DIFFERENCES] != "complete"):
        raise ValueError(f"{manifest_path}: run did not complete")
    completion = rows.pop()
    expected_captures = int(completion[-ONE_PIXEL])
    for values in rows:
        if len(values) != len(MANIFEST_FIELDS):
            raise ValueError(f"{manifest_path}: malformed manifest row")
        row = dict(zip(MANIFEST_FIELDS, values))
        name = row["frame"]
        if name in metadata or Path(name).name != name or not name.endswith(".rgba"):
            raise ValueError(f"{manifest_path}: invalid or duplicate frame {name!r}")
        dimensions = (int(row["width"]), int(row["height"]))
        frame_id, clock = int(row["frame_id"]), int(row["clock_ms"])
        if (min(dimensions) < MINIMUM_CAPTURE_EXTENT
                or frame_id < FIRST_CAPTURE_FRAME_ID or row["pending"] != "false"):
            raise ValueError(f"{manifest_path}: incomplete capture {name}")
        metadata[name] = (*dimensions, frame_id, clock, row["pending"])
    if len(metadata) != expected_captures:
        raise ValueError(f"{manifest_path}: expected {expected_captures} captures, got {len(metadata)}")
    if metadata.keys() != captures.keys():
        missing = sorted(metadata.keys() - captures.keys())
        extra = sorted(captures.keys() - metadata.keys())
        raise ValueError(f"{manifest_path}: missing frames {missing}; extra frames {extra}")
    for name, (width, height, *_rest) in metadata.items():
        if captures[name].stat().st_size != width * height * RGBA_CHANNELS:
            raise ValueError(f"{captures[name]}: size does not match manifest dimensions")
    return captures, metadata


def diff(before, after):
    left, right = before.read_bytes(), after.read_bytes()
    if not left or len(left) != len(right) or len(left) % RGBA_CHANNELS:
        raise ValueError(f"{before.name} / {after.name}: invalid or different RGBA sizes")
    if left == right:
        return NO_DIFFERENCES, NO_DIFFERENCES
    pixels, largest = NO_DIFFERENCES, NO_DIFFERENCES
    for offset in range(NO_DIFFERENCES, len(left), RGBA_CHANNELS):
        a, b = left[offset:offset + RGBA_CHANNELS], right[offset:offset + RGBA_CHANNELS]
        if a != b:
            pixels += ONE_PIXEL
            largest = max(largest, *(abs(x - y) for x, y in zip(a, b)))
    return pixels, largest


def compare(label, left, right):
    before, before_metadata = left
    after, after_metadata = right
    failed = False
    if before.keys() != after.keys():
        print(f"{label}: missing frames {sorted(before.keys() - after.keys())}; "
              f"extra frames {sorted(after.keys() - before.keys())}")
        failed = True
    if before_metadata != after_metadata:
        print(f"{label}: capture manifests differ (including manifest presence)")
        failed = True
    for name in sorted(before.keys() & after.keys()):
        try:
            pixels, largest = diff(before[name], after[name])
        except ValueError as error:
            print(f"{label} {name}: {error}")
            failed = True
            continue
        print(f"{label} {name}: {pixels} pixels differ, max channel difference {largest}")
        failed |= bool(pixels)
    print(f"{label}: {'FAIL' if failed else 'PASS'}; "
          f"{len(before)} / {len(after)} frames")
    return failed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory_a")
    parser.add_argument("tag_a")
    parser.add_argument("directory_b")
    parser.add_argument("tag_b")
    parser.add_argument("repeat_pair", nargs="*")
    args = parser.parse_args()
    if args.repeat_pair and len(args.repeat_pair) != REPEAT_PAIR_ARGUMENTS:
        parser.error("a repeated baseline needs both its directory and tag")
    try:
        before = frames(args.directory_a, args.tag_a)
        after = frames(args.directory_b, args.tag_b)
        failed = compare("A vs B", before, after)
        if args.repeat_pair:
            repeat = frames(*args.repeat_pair)
            failed |= compare("A vs A repeat", before, repeat)
        return EXIT_DIFFERENT if failed else EXIT_SUCCESS
    except (OSError, ValueError, TypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return EXIT_DIFFERENT


if __name__ == "__main__":
    sys.exit(main())
