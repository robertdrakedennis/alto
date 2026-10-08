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
LOBBY_PORT = int(os.environ.get('LOBBY_PORT', '48570'))
WORLD_PORT = int(os.environ.get('WORLD_PORT', '48571'))
LANE_PORT_FIRST = 48500
LANE_PORT_END = 48600
MAX_SECONDS = 300
SERVER_START_SECONDS = 90
CHILD_WAIT_SECONDS = 30
POLL_SECONDS = 1
HEADER_BYTES = 8
RECORD_HEADER_BYTES = 12
HASH_CHUNK_BYTES = 1_048_576
FIRST_TRACE_ENTRY = 0
FIRST_PUBLICATION_ORDINAL = 0
ONE_PUBLICATION = 1
MARKER_PAYLOAD_SPLIT = 1
MARKER_PAYLOAD_INDEX = 1
SHOTS = '220,300,400,800,1100,1460,1600,1800,2150'
assert LANE_PORT_FIRST <= LOBBY_PORT < LANE_PORT_END
assert LANE_PORT_FIRST <= WORLD_PORT < LANE_PORT_END
assert LOBBY_PORT != WORLD_PORT
WORK = Path(tempfile.mkdtemp(prefix='alto-ability-gameplay-record-'))
TARGET = Path(os.environ['CARGO_TARGET_DIR'])
CLIENT = Path(os.environ.get('ALTO_RECORD_CLIENT', str(TARGET / 'debug' / 'client910')))
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
          'clientBuildProfile': os.environ.get('ALTO_RECORD_BUILD_PROFILE', 'unqualified'),
          'sha256': {str(file.relative_to(ROOT)): hashlib.sha256(file.read_bytes()).hexdigest()
                     for file in source_files}}
input_files = [ROOT/'server/data/generated/abilities/rules.json',
               ROOT/'server/data/generated/combat/rules.json',
               ROOT/'server/data/generated/npcs/profiles.json']
source['inputSha256'] = {str(file.relative_to(ROOT)): hashlib.sha256(file.read_bytes()).hexdigest()
                        for file in input_files}
source['inputStatus'] = 'Private generated inputs; source-qualified epoch, not committed game data'
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
                      CLIENT910_WINDOW_SIZE='960,640', CLIENT910_PERF_NO_GPU_TIME='1',
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
    ability_events = []
    marker = '[ability-fixture] '
    for line in (WORK/'world.log').read_text().splitlines():
        if marker in line:
            ability_events.append(json.loads(line.split(marker, 1)[1]))
    if not ability_events:
        raise RuntimeError('No ordinary ability hits were observed; keep raw evidence')
    (output/'ability-events.json').write_text(json.dumps(ability_events, indent=2) + '\n')
    life_events = []
    life_marker = '[ability-life-fixture] '
    for line in (WORK/'world.log').read_text().splitlines():
        if life_marker in line:
            life_events.append(json.loads(line.split(life_marker, 1)[1]))
    if not life_events:
        raise RuntimeError('No ordinary NPC life publications were observed; keep raw evidence')
    (output/'life-events-raw.json').write_text(json.dumps(life_events, indent=2) + '\n')
    # Bind the pre-flush snapshots to actual sent info records, rather than
    # assuming every raw world tick reached the recorded socket cutoff.
    sent_trace = [json.loads(line) for line in (output/'server-trace.jsonl').read_text().splitlines()]
    trace_pid = sent_trace[FIRST_TRACE_ENTRY]['pid']
    player_publications = []
    life_publications = []
    latest_players = {}
    latest_lives = {}
    player_ordinal = FIRST_PUBLICATION_ORDINAL
    npc_ordinal = FIRST_PUBLICATION_ORDINAL
    latest_world_tick = None
    for line in (WORK/'world.log').read_text().splitlines():
        if '[ability-player-fixture] ' in line:
            row = json.loads(line.split('[ability-player-fixture] ', MARKER_PAYLOAD_SPLIT)[MARKER_PAYLOAD_INDEX])
            latest_players[row['pid']] = row
            latest_world_tick = row['tick']
        elif life_marker in line:
            row = json.loads(line.split(life_marker, MARKER_PAYLOAD_SPLIT)[MARKER_PAYLOAD_INDEX])
            latest_lives[row['index']] = row
        elif '[trace-info] ' in line:
            entry = json.loads(line.split('[trace-info] ', MARKER_PAYLOAD_SPLIT)[MARKER_PAYLOAD_INDEX])
            if entry['pid'] != trace_pid:
                continue
            if entry['t'] == 'player_info':
                row = latest_players.get(trace_pid)
                if row is None or row['tick'] != latest_world_tick:
                    raise RuntimeError('A sent PLAYER_INFO has no exact pre-flush player receipt')
                player_publications.append(dict(row, playerInfoOrdinal=player_ordinal))
                player_ordinal += ONE_PUBLICATION
            elif entry['t'] == 'npc_info':
                for index, definition, *_tile in entry['npcs']:
                    row = latest_lives.get(index)
                    if row is None:
                        continue  # Other NPCs are checked by the ordinary roster observer.
                    if row['tick'] != latest_world_tick or row['definition'] != definition:
                        raise RuntimeError('A sent NPC_INFO has a stale actor life publication')
                    life_publications.append(dict(row, npcInfoOrdinal=npc_ordinal))
                npc_ordinal += ONE_PUBLICATION
    (output/'player-publications.json').write_text(json.dumps(player_publications, indent=2) + '\n')
    (output/'life-events.json').write_text(json.dumps(life_publications, indent=2) + '\n')
    (output/'publication-alignment.json').write_text(json.dumps({'playerInfoCount': player_ordinal, 'npcInfoCount': npc_ordinal, 'rawLifeRows': len(life_events), 'boundLifeRows': len(life_publications), 'scope': 'Actual server sends; replay consumes the captured packet prefix. Raw terminal world-only ticks are preserved separately.'}, indent=2) + '\n')
    (output/'plan.json').write_text(json.dumps(plan, indent=2) + '\n')
    print(f'Recording processed in {output}; inspect/replay before installing fixtures', flush=True)
finally:
    for child in reversed(children):
        if child.poll() is None:
            child.send_signal(signal.SIGTERM)
    process_results = []
    for child in reversed(children):
        forced = False
        try:
            child.wait(timeout=CHILD_WAIT_SECONDS)
        except subprocess.TimeoutExpired:
            child.kill()
            forced = True
            child.wait(timeout=CHILD_WAIT_SECONDS)
        process_results.append({'pid': child.pid, 'exit': child.returncode,
                                'waited': True, 'forced': forced})
    (WORK/'processes.json').write_text(json.dumps(process_results, indent=2) + '\n')
    for log in logs:
        log.close()
