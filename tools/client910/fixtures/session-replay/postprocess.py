#!/usr/bin/env python3
"""Trim a CLIENT910_RECORD trace into the committed fixture.

usage: postprocess.py [--respawn] <raw.rtr> <world.log> <out dir>
       postprocess.py --done <raw.rtr>   (exit 0 once the cutscene map is installed)
       postprocess.py --done-respawn <raw.rtr>
                     (exit 0 once the woodcutting tree has respawned, see below)
       postprocess.py --until <cycle> <raw.rtr> <world.log> <out dir>
       postprocess.py --done-cycle <cycle> <raw.rtr>
                     (fixed-length sessions: keep the trace below `cycle`)

- session.rtr: the trace up to (not including) the first map transaction
  acknowledged after startup (the cutscene rebuild; its install needs the
  renderer's scene build and is not replayed). ENV records keep only the
  CLIENT910_* input injectors (no local paths).
- server-trace.jsonl: the dev server's own PLAYER_INFO / NPC_INFO positions
  (ALTO_TRACE_INFO=1), in send order, for the replay's independence checks.

Woodcutting (`woodcutting/record.sh`, `--respawn`): the session has no map
transaction after startup; it ends RESPAWN_TAIL cycles after the world socket
delivered the tree's respawn, the dev server's LOC_ADD_CHANGE of loc 38760 at
(3228, 3228) (ServerProt.ts framing: 0x80 139, var-byte length 6, p1 shape
10 << 2 | angle 3, p4_alt3 38760, p1_alt2 coord (4 << 4 | 4)). The fell sends the same frame
with the stump 40350 first, so the first 38760 frame is the respawn.
"""
import json
import os
import struct
import sys

INJECTORS = {
    'CLIENT910_UI_CLICK', 'CLIENT910_UI_CLICKS', 'CLIENT910_KEY_INPUT', 'CLIENT910_TYPE_INPUT',
    'CLIENT910_WHEEL_INPUT', 'CLIENT910_TOOLKIT_INPUT', 'CLIENT910_UI_OPERATIONS',
    'CLIENT910_UI_INPUT', 'CLIENT910_UI_HOVER', 'CLIENT910_CUTSCENE', 'CLIENT910_WINDOW_RESIZES',
    'CLIENT910_DROP_CONNECTION',
}
RESPAWN = bytes([0x80, 139, 6, 10 << 2 | 3, 0x00, 0x00, 0x68, 0x97, (-(4 << 4 | 4)) & 0xFF])
RESPAWN_TAIL = 150


def read(raw):
    data = open(raw, 'rb').read() if os.path.exists(raw) else b''
    records, pos = [], 8
    while data[:4] == b'RTR1' and pos + 12 <= len(data):
        cycle, tag, n = struct.unpack('<i4sI', data[pos:pos + 12])
        if pos + 12 + n > len(data):
            break
        records.append((cycle, tag, data[pos + 12:pos + 12 + n]))
        pos += 12 + n
    end = min([c for c, t, _ in records if t == b'MAPI' and c > 0], default=None)
    return data, records, end


def respawn_end(records):
    """The cycle the woodcutting recording is complete at, or None."""
    stream, at = b'', None
    for cycle, tag, body in records:
        if tag == b'IN  ':
            stream += body
            if at is None and RESPAWN in stream:
                at = cycle
    last = max([c for c, _, _ in records], default=-1)
    return at + RESPAWN_TAIL if at is not None and last >= at + RESPAWN_TAIL else None


if sys.argv[1] == '--done':
    sys.exit(0 if read(sys.argv[2])[2] is not None else 1)
if sys.argv[1] == '--done-respawn':
    sys.exit(0 if respawn_end(read(sys.argv[2])[1]) is not None else 1)
if sys.argv[1] == '--done-cycle':
    sys.exit(0 if any(c >= int(sys.argv[2]) for c, _, _ in read(sys.argv[3])[1]) else 1)
respawn = sys.argv[1] == '--respawn'
until = int(sys.argv[2]) if sys.argv[1] == '--until' else None
raw, log, out = sys.argv[3:6] if until is not None else sys.argv[2:5] if respawn else sys.argv[1:4]
data, records, end = read(raw)
if until is not None:
    if max([c for c, _, _ in records], default=-1) < until:
        sys.exit('the recording is shorter than --until')
    end = until
elif respawn:
    if end is not None:
        sys.exit('the woodcutting recording rebuilt the map mid-session')
    end = respawn_end(records)
    if end is None:
        sys.exit('the recording never reached the tree respawn')
    end += 1
if end is None:
    sys.exit('the recording never reached the cutscene map installation')
kept = bytearray(data[:8])
for cycle, tag, body in records:
    if end is not None and cycle >= end:
        continue
    if tag == b'ENV ' and body.split(b'=', 1)[0].decode() not in INJECTORS:
        continue
    kept += struct.pack('<i4sI', cycle, tag, len(body)) + body
open(f'{out}/session.rtr', 'wb').write(kept)
head = next(body for _, tag, body in records if tag == b'HEAD').decode()
pid = int(dict(line.split('=', 1) for line in head.splitlines())['pid'])
with open(f'{out}/server-trace.jsonl', 'w') as f:
    for line in open(log, errors='replace'):
        if '[trace-info] ' in line:
            entry = json.loads(line.split('[trace-info] ', 1)[1])
            if entry['pid'] == pid:
                f.write(json.dumps(entry, separators=(',', ':')) + '\n')
print(f'session.rtr: {len(kept)} bytes, cycles < {end}; pid {pid}')
