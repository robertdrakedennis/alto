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
REPOSITORY_PARENT_INDEX = 4
JSON_INDENT = 2
INITIAL_LAUNCH_COUNT = 0
MAXIMUM_LAUNCHES = 4
FINISHING_SAMPLE_DEATHS = 1
NO_SLAYER_POINTS = 0
EMPTY_STACK = 0
FIRST_RECEIPT = 0
BASE = Path('/private/tmp/alto-legacy-slayer-wilderness')
LAUNCH_LEDGER = BASE/'client-launch-ledger.json'
ONE_CLIENT_LAUNCH = 1
LAST_CHILD_INDEX = -1
CHILD_LEDGER_INDEX = 1
INCOMPLETE_CYCLE = -1
RECORD_MAGIC_BYTES = 4
NEXT_CYCLE_OFFSET = 1
WINDOW_WIDTH, WINDOW_HEIGHT = 1024, 768
SCREENSHOT_DIGITS = 5
ROOT = HERE.parents[REPOSITORY_PARENT_INDEX]
TARGET = Path(os.environ['CARGO_TARGET_DIR'])
CLIENT = TARGET/'debug/client910'
import tempfile
WORK = Path(tempfile.mkdtemp(prefix='alto-slayer-wilderness-record-'))
LOBBY_PORT = int(os.environ.get('LOBBY_PORT','48500'))
WORLD_PORT = int(os.environ.get('WORLD_PORT','48501'))
LANE_PORT_FIRST, LANE_PORT_END = 48500, 48600
assert LANE_PORT_FIRST <= LOBBY_PORT < LANE_PORT_END and LANE_PORT_FIRST <= WORLD_PORT < LANE_PORT_END and LOBBY_PORT != WORLD_PORT
SERVER_START_SECONDS, MAX_SECONDS, CHILD_WAIT_SECONDS, POLL_SECONDS = 90, 420, 30, 1
HEADER_BYTES, RECORD_HEADER_BYTES = 8, 12
TAIL_SCREENSHOT_CYCLE = 4200
ENV = dict(os.environ, CARGO_TARGET_DIR=str(TARGET), ALTO_LOBBY_PORT=str(LOBBY_PORT), ALTO_WORLD_PORT=str(WORLD_PORT), ALTO_PLAYER_DATA_DIR=str(WORK/'players'), ALTO_TRACE_INFO='1', ALTO_DEV_NPCS='0')
children, logs, ledger = [], [], []
result = {'passed': False, 'work': str(WORK), 'clientLaunches': INITIAL_LAUNCH_COUNT}
print(f'Recording scratch: {WORK}',flush=True)

def interrupted(_signal, _frame):
    raise KeyboardInterrupt('Owned recording interrupted; running cleanup')

signal.signal(signal.SIGTERM,interrupted)


def save():
    (WORK/'process.json').write_text(json.dumps({'result': result, 'children': ledger}, indent=JSON_INDENT))


def start(arguments, name, cwd, environment):
    log = (WORK/f'{name}.log').open('w')
    logs.append(log)
    child = subprocess.Popen(arguments, cwd=cwd, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    row = {'name': name, 'pid': child.pid, 'arguments': arguments, 'waited': False, 'start': subprocess.check_output(['ps', '-p', str(child.pid), '-o', 'lstart='], text=True).strip()}
    children.append((child, row))
    ledger.append(row)
    if name == 'client':
        result['clientLaunches'] += ONE_CLIENT_LAUNCH
        prior = json.loads(LAUNCH_LEDGER.read_text()) if LAUNCH_LEDGER.exists() else []
        prior.append({'work':str(WORK), 'pid':child.pid, 'start':row['start'], 'launch':len(prior)+ONE_CLIENT_LAUNCH})
        LAUNCH_LEDGER.write_text(json.dumps(prior, indent=JSON_INDENT))
    save()
    print(f'Owned {name} PID {child.pid}', flush=True)
    return child


def run(arguments, name, cwd, environment):
    child = start(arguments, name, cwd, environment)
    code = child.wait(timeout=SERVER_START_SECONDS)
    row = children[LAST_CHILD_INDEX][CHILD_LEDGER_INDEX]
    row.update(exit=code, waited=True)
    save()
    if code: raise RuntimeError(f'{name} exit {code}')


def finish(child, row):
    if child.poll() is None:
        row['termination'] = 'SIGTERM'
        child.send_signal(signal.SIGTERM)
        try:
            child.wait(timeout=CHILD_WAIT_SECONDS)
        except subprocess.TimeoutExpired:
            row['termination'] = 'SIGTERM30 then exact-PID kill30'
            child.kill()
            child.wait(timeout=CHILD_WAIT_SECONDS)
    row.update(exit=child.wait(), waited=True)


def complete_cycle():
    raw = WORK/'raw.rtr'
    if not raw.exists(): return INCOMPLETE_CYCLE
    data = raw.read_bytes()
    offset, completed = HEADER_BYTES, INCOMPLETE_CYCLE
    while data[:RECORD_MAGIC_BYTES] == b'RTR1' and offset+RECORD_HEADER_BYTES <= len(data):
        cycle, tag, length = struct.unpack('<i4sI', data[offset:offset+RECORD_HEADER_BYTES])
        if offset+RECORD_HEADER_BYTES+length > len(data): break
        if tag == b'NOWM': completed = max(completed, cycle-NEXT_CYCLE_OFFSET)
        offset += RECORD_HEADER_BYTES+length
    return completed


try:
    prior = json.loads(LAUNCH_LEDGER.read_text()) if LAUNCH_LEDGER.exists() else []
    if len(prior) >= MAXIMUM_LAUNCHES: raise RuntimeError('Client launch budget exhausted')
    result['provenance'] = {'head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT,text=True).strip(), 'sourcePatchSha256': hashlib.sha256(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT)).hexdigest(), 'clientSha256': hashlib.sha256(CLIENT.read_bytes()).hexdigest(), 'sourceSha256': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [HERE/'seed.mjs',HERE/'server.mjs',ROOT/'server/src/lostcity/systems/slayer/SlayerFinishing.ts',ROOT/'server/src/lostcity/systems/slayer/Slayer.ts',ROOT/'server/src/lostcity/systems/wilderness/Wilderness.ts',ROOT/'server/src/lostcity/systems/combat/PlayerDeath.ts']}, 'generatedInputs': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT/'server/data/generated/combat').glob('*.json'))+sorted((ROOT/'server/data/generated/npcs').glob('*.json'))+sorted((ROOT/'server/data/generated/drops').glob('*.json'))+sorted((ROOT/'server/data/generated/slayer').glob('*.json'))+sorted((ROOT/'server/data/generated/wilderness').glob('*.json'))}, 'runnerSha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    (WORK/'source.patch').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    paths = subprocess.check_output(['git','diff','--name-only','main'], cwd=ROOT, text=True).splitlines()
    for relative in paths:
        source = ROOT/relative
        if not source.is_file(): continue
        destination = WORK/'source'/relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(source.read_bytes())
    result['provenance']['fullSourceSha256'] = {relative:hashlib.sha256((ROOT/relative).read_bytes()).hexdigest() for relative in paths if (ROOT/relative).is_file()}
    binary = json.loads((BASE/'client-build-1-binary.json').read_text())
    if result['provenance']['clientSha256'] != binary['sha256']: raise RuntimeError('Client binary differs from checked build')
    result['provenance']['checkedBuild'] = binary
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
    client_env = dict(ENV, CLIENT910_RECORD=str(WORK/'raw.rtr'), CLIENT910_LOG='info', CLIENT910_WINDOW_SIZE=f'{WINDOW_WIDTH},{WINDOW_HEIGHT}', CLIENT910_PREFERENCES_FILE=str(client_dir/'preferences.dat'), CLIENT910_VARC_FILE=str(client_dir/'client-vars.dat'), CLIENT910_UID192_FILE=str(client_dir/'random.dat'), CLIENT910_OUT_TRACE='1',CLIENT910_UI_TRACE_INPUT='1', CLIENT910_UI_OPERATIONS=plan['operations'], CLIENT910_UI_CLICKS=plan['clicks'], CLIENT910_UI_HOVER=plan['hover'], CLIENT910_KEY_INPUT=plan['keys'], CLIENT910_SCREENSHOT_SERIES=shots)
    args = [str(CLIENT),'--renderer','modern','--direct-login','--lobby-port',str(LOBBY_PORT),'--world-port',str(WORLD_PORT),'--username',plan['accountKey'],'--password','password','--cache-dir',str(client_dir/'cache'),'--screenshot',str(WORK/'slayer-wilderness.png')]
    for command in plan['commands']: args += ['--server-command',command]
    client = start(args,'client',ROOT/'tools/client910',client_env)
    deadline = time.monotonic()+MAX_SECONDS
    expected = WORK/f"slayer-wilderness_{plan['lastCycle']:0{SCREENSHOT_DIGITS}d}.png"
    while True:
        log = (WORK/'client.log').read_text(errors='replace')
        if complete_cycle() >= plan['lastCycle'] and expected.is_file() and f'screenshot written: {expected}' in log: break
        if client.poll() is not None or time.monotonic()>deadline: raise RuntimeError(f'Incomplete client recording: completed={complete_cycle()}, exit={client.poll()}')
        time.sleep(POLL_SECONDS)
    world_log = (WORK/'world.log').read_text(errors='replace')
    if '[WORLD]: tick step' in world_log or 'Error:' in world_log or 'step failed' in world_log:
        raise RuntimeError('World step error; terminal image is not proof of valid gameplay')
    rows = [json.loads(line) for line in (WORK/'combat-receipts.jsonl').read_text().splitlines()]
    states = [player for row in rows if row['kind']=='state' for player in row['players']]
    deaths = [row for row in rows if row['kind']=='death' and row['targetDefinition']==plan['slug']]
    if len(deaths) != FINISHING_SAMPLE_DEATHS: raise RuntimeError('Expected one ordinary finishing-tool kill')
    if not any(player['quick'] and player['slayer']['points']==NO_SLAYER_POINTS for player in states): raise RuntimeError('Native quick-kills purchase was not durably adopted')
    if not any(player['quick'] and not any(item==plan['salt'] and count>EMPTY_STACK for item,count in player['backpack']) and any(item==plan['weapon'] and count>EMPTY_STACK for item,count in player['worn']) for player in states): raise RuntimeError('Finishing salt consumption/equipment was not observed')
    if not any(player['wilderness']==plan['pad']['level'] for player in states): raise RuntimeError('Source Wilderness level never published')
    destination = plan['expectedPad']
    if not any(player['x']==destination['centre']['x'] and player['z']==destination['centre']['z'] and player['level']==destination['centre']['level'] and player['wilderness']==destination['level'] for player in states): raise RuntimeError('Ordinary obelisk transport never reached its generated destination')
    result['observed'] = {'finishingDeath':deaths[FIRST_RECEIPT], 'nativeQuickKillsPurchased':True, 'saltConsumed':True, 'sourcePad':plan['pad'], 'destinationPad':destination}
    result['completeCycle'] = complete_cycle()
    result['terminalScreenshot'] = {'path':str(expected),'sha256':hashlib.sha256(expected.read_bytes()).hexdigest()}
    finish(client, next(row for child,row in children if child is client))
    output=WORK/'processed'
    output.mkdir()
    run(['python3',str(HERE.parent/'postprocess.py'),'--until',str(plan['lastCycle']+NEXT_CYCLE_OFFSET),str(WORK/'raw.rtr'),str(WORK/'world.log'),str(output)],'postprocess',ROOT,ENV)
    shutil.copyfile(WORK/'combat-receipts.jsonl',output/'combat-receipts.jsonl')
    result['passed']=True
except BaseException as error:
    result['error']=str(error)
    raise
finally:
    failures = []
    for child, row in reversed(children):
        try:
            finish(child, row)
        except BaseException as error:
            failures.append({'pid':child.pid, 'error':repr(error)})
    for log in logs: log.close()
    result['cleanupFailures'] = failures
    result['allWaited'] = all(row['waited'] for row in ledger)
    save()
    print(json.dumps(result),flush=True)
