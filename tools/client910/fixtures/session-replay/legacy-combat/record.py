#!/usr/bin/env python3
"""Record the ordinary Legacy client with owned, waited processes and retained evidence."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
TARGET = Path(os.environ['CARGO_TARGET_DIR'])
CLIENT = TARGET/'debug/client910'
import tempfile
WORK = Path(tempfile.mkdtemp(prefix='alto-legacy-record-'))
LOBBY_PORT = int(os.environ.get('LOBBY_PORT','48100'))
WORLD_PORT = int(os.environ.get('WORLD_PORT','48101'))
LANE_PORT_FIRST, LANE_PORT_END = 48100, 48200
assert LANE_PORT_FIRST <= LOBBY_PORT < LANE_PORT_END and LANE_PORT_FIRST <= WORLD_PORT < LANE_PORT_END and LOBBY_PORT != WORLD_PORT
SERVER_START_SECONDS, MAX_SECONDS, CHILD_WAIT_SECONDS, POLL_SECONDS = 90, 420, 30, 1
HEADER_BYTES, RECORD_HEADER_BYTES = 8, 12
TAIL_SCREENSHOT_CYCLE = 4500
ENV = dict(os.environ, CARGO_TARGET_DIR=str(TARGET), ALTO_LOBBY_PORT=str(LOBBY_PORT), ALTO_WORLD_PORT=str(WORLD_PORT), ALTO_PLAYER_DATA_DIR=str(WORK/'players'), ALTO_TRACE_INFO='1', ALTO_DEV_NPCS='0')
children, logs, ledger = [], [], []
result = {'passed': False, 'work': str(WORK), 'clientLaunches': 0}
print(f'Recording scratch: {WORK}',flush=True)

def interrupted(_signal, _frame):
    raise KeyboardInterrupt('Owned recording interrupted; running cleanup')

signal.signal(signal.SIGTERM,interrupted)


def save():
    (WORK/'process.json').write_text(json.dumps({'result': result, 'children': ledger}, indent=2))


def start(arguments, name, cwd, environment):
    log = (WORK/f'{name}.log').open('w')
    logs.append(log)
    child = subprocess.Popen(arguments, cwd=cwd, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    row = {'name': name, 'pid': child.pid, 'arguments': arguments, 'waited': False}
    children.append((child, row))
    ledger.append(row)
    if name == 'client': result['clientLaunches'] += 1
    save()
    print(f'Owned {name} PID {child.pid}', flush=True)
    return child


def run(arguments, name, cwd, environment):
    child = start(arguments, name, cwd, environment)
    code = child.wait(timeout=SERVER_START_SECONDS)
    row = children[-1][1]
    row.update(exit=code, waited=True)
    save()
    if code: raise RuntimeError(f'{name} exit {code}')


def complete_cycle():
    raw = WORK/'raw.rtr'
    if not raw.exists(): return -1
    data = raw.read_bytes()
    offset, completed = HEADER_BYTES, -1
    while data[:4] == b'RTR1' and offset+RECORD_HEADER_BYTES <= len(data):
        cycle, tag, length = struct.unpack('<i4sI', data[offset:offset+RECORD_HEADER_BYTES])
        if offset+RECORD_HEADER_BYTES+length > len(data): break
        if tag == b'NOWM': completed = max(completed, cycle-1)
        offset += RECORD_HEADER_BYTES+length
    return completed


try:
    result['provenance'] = {'head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT,text=True).strip(), 'sourcePatchSha256': hashlib.sha256(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT)).hexdigest(), 'clientSha256': hashlib.sha256(CLIENT.read_bytes()).hexdigest(), 'sourceSha256': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [HERE/'seed.mjs',HERE/'server.mjs',ROOT/'server/src/lostcity/data/LegacyCombatData.ts',ROOT/'server/src/lostcity/systems/combat/CombatOptions.ts',ROOT/'server/src/lostcity/systems/combat/LegacyCombat.ts',ROOT/'server/src/lostcity/tools/data/domains/combat/LegacyStage.ts']}, 'generatedInputs': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT/'server/data/generated/combat').glob('*.json'))+sorted((ROOT/'server/data/generated/npcs').glob('*.json'))}, 'runnerSha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    (WORK/'source.patch').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    run(['node',str(HERE/'seed.mjs'),str(WORK)],'seed',ROOT/'server',ENV)
    plan = json.loads((WORK/'plan.json').read_text())
    if plan['lastCycle'] >= TAIL_SCREENSHOT_CYCLE: raise RuntimeError('Tail screenshot must follow semantic completion')
    start(['node','src/lostcity/lobby.ts'],'lobby',ROOT/'server',ENV)
    start(['node',str(HERE/'server.mjs'),str(WORK)],'world',ROOT/'server',ENV)
    deadline = time.monotonic()+SERVER_START_SECONDS
    while not all(f'listening on port {port}' in (WORK/f'{name}.log').read_text().lower() for name,port in [('lobby',LOBBY_PORT),('world',WORLD_PORT)]):
        if any(child.poll() is not None for child,row in children if row['name'] in ('lobby','world')) or time.monotonic()>deadline: raise RuntimeError('Server readiness failed')
        time.sleep(POLL_SECONDS)
    client_dir = WORK/'client'
    client_dir.mkdir()
    run(['bash',str(ROOT/'ref/independence/wired-guard.sh')],'wired-guard',ROOT,ENV)
    shots = ','.join(map(str,[*plan['semanticShots'],TAIL_SCREENSHOT_CYCLE]))
    client_env = dict(ENV, CLIENT910_RECORD=str(WORK/'raw.rtr'), CLIENT910_LOG='info', CLIENT910_WINDOW_SIZE='1024,768', CLIENT910_PREFERENCES_FILE=str(client_dir/'preferences.dat'), CLIENT910_VARC_FILE=str(client_dir/'client-vars.dat'), CLIENT910_UID192_FILE=str(client_dir/'random.dat'), CLIENT910_OUT_TRACE='1',CLIENT910_UI_TRACE_INPUT='1', CLIENT910_UI_OPERATIONS=plan['operations'], CLIENT910_KEY_INPUT=plan['keys'], CLIENT910_SCREENSHOT_SERIES=shots)
    args = [str(CLIENT),'--renderer','modern','--direct-login','--lobby-port',str(LOBBY_PORT),'--world-port',str(WORLD_PORT),'--username',plan['accountKey'],'--password','password','--cache-dir',str(client_dir/'cache'),'--screenshot',str(WORK/'legacy.png')]
    for command in plan['commands']: args += ['--server-command',command]
    client = start(args,'client',ROOT/'tools/client910',client_env)
    deadline = time.monotonic()+MAX_SECONDS
    expected = WORK/f"legacy_{plan['lastCycle']:05}.png"
    while True:
        log = (WORK/'client.log').read_text(errors='replace')
        if complete_cycle() >= plan['lastCycle'] and expected.is_file() and f'screenshot written: {expected}' in log: break
        if client.poll() is not None or time.monotonic()>deadline: raise RuntimeError(f'Incomplete client recording: completed={complete_cycle()}, exit={client.poll()}')
        time.sleep(POLL_SECONDS)
    world_log = (WORK/'world.log').read_text(errors='replace')
    if '[WORLD]: tick step' in world_log or 'Error:' in world_log or 'step failed' in world_log:
        raise RuntimeError('World step error; terminal image is not proof of valid gameplay')
    result['completeCycle'] = complete_cycle()
    result['terminalScreenshot'] = {'path':str(expected),'sha256':hashlib.sha256(expected.read_bytes()).hexdigest()}
    client.send_signal(signal.SIGTERM)
    code = client.wait(timeout=CHILD_WAIT_SECONDS)
    next(row for child,row in children if child is client).update(exit=code,waited=True,termination='SIGTERM after complete cycle + terminal readback')
    output=WORK/'processed'
    output.mkdir()
    run(['python3',str(HERE.parent/'postprocess.py'),'--until',str(plan['lastCycle']+1),str(WORK/'raw.rtr'),str(WORK/'world.log'),str(output)],'postprocess',ROOT,ENV)
    shutil.copyfile(WORK/'combat-receipts.jsonl',output/'combat-receipts.jsonl')
    result['passed']=True
except BaseException as error:
    result['error']=str(error)
    raise
finally:
    try:
        for child,row in reversed(children):
            if child.poll() is None:
                row['termination']='SIGTERM'
                child.send_signal(signal.SIGTERM)
        for child,row in reversed(children):
            if not row['waited']:
                row.update(exit=child.wait(timeout=CHILD_WAIT_SECONDS),waited=True)
    finally:
        for log in logs: log.close()
        save()
        print(json.dumps(result),flush=True)
