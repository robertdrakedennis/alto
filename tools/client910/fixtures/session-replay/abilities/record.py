#!/usr/bin/env python3
"""Capture native ability UI input on owned ports, retaining all failure evidence."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
LOBBY_PORT = int(os.environ.get('LOBBY_PORT', '48560'))
WORLD_PORT = int(os.environ.get('WORLD_PORT', '48561'))
LANE_PORT_FIRST = 48500
LANE_PORT_END = 48600
MAX_SECONDS = 300
SERVER_START_SECONDS = 90
CHILD_WAIT_SECONDS = 30
POLL_SECONDS = 1
HEADER_BYTES = 8
RECORD_HEADER_BYTES = 12
HASH_CHUNK_BYTES = 1_048_576
SHOTS = '180,400,454,520,610,990,1340,1790,2020,2120,2200,2280,2360,2520'
assert LANE_PORT_FIRST <= LOBBY_PORT < LANE_PORT_END
assert LANE_PORT_FIRST <= WORLD_PORT < LANE_PORT_END
assert LOBBY_PORT != WORLD_PORT
WORK = Path(tempfile.mkdtemp(prefix='alto-abilities-record-'))
TARGET = Path(os.environ['CARGO_TARGET_DIR'])
CLIENT = TARGET / 'debug' / 'client910'
if not CLIENT.is_file():
    raise RuntimeError(f'Build the exact owned client first: {CLIENT}')
ENV = dict(os.environ, ALTO_LOBBY_PORT=str(LOBBY_PORT),
           ALTO_WORLD_PORT=str(WORLD_PORT), ALTO_PLAYER_DATA_DIR=str(WORK/'players'),
           ALTO_TRACE_INFO='1', ALTO_DEV_NPCS='0')
children = []
logs = []
print(f'Recording scratch: {WORK}', flush=True)
source_files = [HERE/'seed.mjs', HERE/'server.mjs', HERE/'record.py',
                ROOT/'tools/client910/src/scenario_abilities.rs',
                ROOT/'tools/client910/src/app.rs']
source = {'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
          'status': subprocess.check_output(['git', 'status', '--short'], cwd=ROOT, text=True),
          'client': str(CLIENT),
          'sha256': {str(file.relative_to(ROOT)): hashlib.sha256(file.read_bytes()).hexdigest()
                     for file in source_files}}
client_digest = hashlib.sha256()
with CLIENT.open('rb') as binary:
    for chunk in iter(lambda: binary.read(HASH_CHUNK_BYTES), b''):
        client_digest.update(chunk)
source['clientSha256'] = client_digest.hexdigest()
(WORK/'source.json').write_text(json.dumps(source, indent=2))
(WORK/'source.patch').write_bytes(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=ROOT))


def start(arguments, name, cwd, environment):
    log = (WORK/f'{name}.log').open('w')
    logs.append(log)
    child = subprocess.Popen(arguments, cwd=cwd, env=environment,
                             stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    children.append(child)
    print(f'Owned {name} PID: {child.pid}', flush=True)
    return child


def last_cycle():
    raw = WORK/'raw.rtr'
    if not raw.exists():
        return -1
    data = raw.read_bytes()
    offset, last = HEADER_BYTES, -1
    while data[:4] == b'RTR1' and offset + RECORD_HEADER_BYTES <= len(data):
        cycle, _tag, length = struct.unpack('<i4sI', data[offset:offset+RECORD_HEADER_BYTES])
        if offset + RECORD_HEADER_BYTES + length > len(data):
            break
        last = max(last, cycle)
        offset += RECORD_HEADER_BYTES + length
    return last


try:
    subprocess.run(['node', str(HERE/'seed.mjs'), str(WORK)], cwd=ROOT/'server', env=ENV, check=True)
    plan = json.loads((WORK/'plan.json').read_text())
    start(['node', 'src/lostcity/lobby.ts'], 'lobby', ROOT/'server', ENV)
    start(['node', str(HERE/'server.mjs')], 'world', ROOT/'server', ENV)
    deadline = time.monotonic() + SERVER_START_SECONDS
    expected = [('lobby', LOBBY_PORT), ('world', WORLD_PORT)]
    while not all(f'listening on port {port}' in (WORK/f'{name}.log').read_text().lower()
                  for name, port in expected):
        if any(child.poll() is not None for child in children) or time.monotonic() > deadline:
            raise RuntimeError('Servers failed to start; inspect retained scratch logs')
        time.sleep(POLL_SECONDS)
    client_dir = WORK/'client'
    client_dir.mkdir()
    warm_cache = os.environ.get('ALTO_RECORD_WARM_CACHE')
    if warm_cache:
        shutil.copytree(warm_cache, client_dir/'cache')
    subprocess.run(['bash', str(ROOT/'ref/independence/wired-guard.sh')], cwd=ROOT, check=True)
    client_env = dict(ENV, CLIENT910_RECORD=str(WORK/'raw.rtr'),
                      CLIENT910_WINDOW_SIZE='960,640', CLIENT910_PROFILE='1',
                      CLIENT910_PREFERENCES_FILE=str(client_dir/'preferences.dat'),
                      CLIENT910_VARC_FILE=str(client_dir/'client-vars.dat'),
                      CLIENT910_UID192_FILE=str(client_dir/'random.dat'),
                      CLIENT910_OUT_TRACE='1', CLIENT910_UI_TRACE_INPUT='1',
                      CLIENT910_SCREENSHOT_SERIES=SHOTS,
                      CLIENT910_UI_OPERATIONS=plan['operations'],
                      CLIENT910_KEY_INPUT=plan['keys'], CLIENT910_UI_INPUT=plan['gestures'])
    arguments = [str(CLIENT), '--renderer', 'modern', '--direct-login',
                 '--lobby-port', str(LOBBY_PORT), '--world-port', str(WORLD_PORT),
                 '--username', plan['accountKey'], '--password', 'password',
                 '--cache-dir', str(client_dir/'cache'),
                 '--screenshot', str(WORK/'abilities.png')]
    for command in plan['commands']:
        arguments += ['--server-command', command]
    (WORK/'command.json').write_text(json.dumps(arguments, indent=2))
    client = start(arguments, 'client', ROOT/'tools/client910', client_env)
    deadline = time.monotonic() + MAX_SECONDS
    while last_cycle() < plan['lastCycle']:
        if client.poll() is not None or time.monotonic() > deadline:
            raise RuntimeError(f'Client did not reach final cycle: last={last_cycle()}, exit={client.poll()}; inspect retained scratch logs')
        time.sleep(POLL_SECONDS)
    client.send_signal(signal.SIGTERM)
    client.wait(timeout=CHILD_WAIT_SECONDS)
    output = WORK/'processed'
    output.mkdir()
    subprocess.run(['python3', str(HERE.parent/'postprocess.py'), '--until',
                    str(plan['lastCycle']), str(WORK/'raw.rtr'), str(WORK/'world.log'),
                    str(output)], cwd=ROOT, check=True)
    print(f'Recording processed in {output}; inspect/replay before installing fixtures', flush=True)
finally:
    for child in reversed(children):
        if child.poll() is None:
            child.send_signal(signal.SIGTERM)
    for child in reversed(children):
        child.wait(timeout=CHILD_WAIT_SECONDS)
    for log in logs:
        log.close()
