#!/usr/bin/env python3
"""Record native Slayer equipment with the shared lane launch ledger and owned, waited processes."""
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
EXPECTED_PRIOR_LAUNCHES = 3
READBACK_FOLLOWUP_CYCLES = 3
ONE_ITEM = 1
NO_BALANCE = 0
HEAD_SLOT = 0
ITEM_FIELD = 0
COUNT_FIELD = 1
INSTANCE_FIELD = 2
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
WORK = Path(tempfile.mkdtemp(prefix='alto-slayer-equipment-record-'))
LOBBY_PORT = int(os.environ.get('LOBBY_PORT','48500'))
WORLD_PORT = int(os.environ.get('WORLD_PORT','48501'))
LANE_PORT_FIRST, LANE_PORT_END = 48500, 48600
assert LANE_PORT_FIRST <= LOBBY_PORT < LANE_PORT_END and LANE_PORT_FIRST <= WORLD_PORT < LANE_PORT_END and LOBBY_PORT != WORLD_PORT
SERVER_START_SECONDS, MAX_SECONDS, CHILD_WAIT_SECONDS, POLL_SECONDS = 90, 420, 30, 1
HEADER_BYTES, RECORD_HEADER_BYTES = 8, 12
TAIL_SCREENSHOT_CYCLE = 4800
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
    assert LAUNCH_LEDGER.is_file(), 'Existing lane launch ledger is required'
    prior = json.loads(LAUNCH_LEDGER.read_text())
    if name == 'client' and len(prior) != EXPECTED_PRIOR_LAUNCHES:
        raise RuntimeError('Client launch budget exhausted immediately before Popen')
    log = (WORK/f'{name}.log').open('w')
    logs.append(log)
    child = subprocess.Popen(arguments, cwd=cwd, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    row = {'name': name, 'pid': child.pid, 'arguments': arguments, 'waited': False, 'start': None}
    children.append((child, row))
    ledger.append(row)
    if name == 'client':
        result['clientLaunches'] += ONE_CLIENT_LAUNCH
        prior.append({'work':str(WORK), 'pid':child.pid, 'start':None, 'launch':len(prior)+ONE_CLIENT_LAUNCH})
        LAUNCH_LEDGER.write_text(json.dumps(prior, indent=JSON_INDENT))
    save()
    row['start'] = subprocess.check_output(['ps', '-p', str(child.pid), '-o', 'lstart='], text=True).strip()
    if name == 'client':
        prior[-ONE_CLIENT_LAUNCH]['start'] = row['start']
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
    assert LAUNCH_LEDGER.is_file(), 'Existing lane launch ledger is required'
    prior = json.loads(LAUNCH_LEDGER.read_text())
    if len(prior) != EXPECTED_PRIOR_LAUNCHES: raise RuntimeError('The fourth capture requires the exact three prior entries')
    result['provenance'] = {'head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT,text=True).strip(), 'sourcePatchSha256': hashlib.sha256(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT)).hexdigest(), 'clientSha256': hashlib.sha256(CLIENT.read_bytes()).hexdigest(), 'sourceSha256': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [HERE/'seed.mjs',HERE/'server.mjs',ROOT/'server/src/lostcity/systems/slayer/SlayerRings.ts',ROOT/'server/src/lostcity/systems/slayer/Slayer.ts',ROOT/'server/src/lostcity/systems/equipment/PlayerEquipment.ts']}, 'generatedInputs': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT/'server/data/generated').rglob('*')) if p.is_file()}, 'runnerSha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    (WORK/'source.patch').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    paths = subprocess.check_output(['git','diff','--name-only','main'], cwd=ROOT, text=True).splitlines()
    for relative in paths:
        source = ROOT/relative
        if not source.is_file(): continue
        destination = WORK/'source'/relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(source.read_bytes())
    result['provenance']['fullSourceSha256'] = {relative:hashlib.sha256((ROOT/relative).read_bytes()).hexdigest() for relative in paths if (ROOT/relative).is_file()}
    binary = json.loads(Path(os.environ['CLIENT_BUILD_RECEIPT']).read_text())
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
    shots = ','.join(map(str,[*plan['semanticShots'],TAIL_SCREENSHOT_CYCLE]))
    client_env = dict(ENV, CLIENT910_RECORD=str(WORK/'raw.rtr'), CLIENT910_LOG='info', CLIENT910_WINDOW_SIZE=f'{WINDOW_WIDTH},{WINDOW_HEIGHT}', CLIENT910_PREFERENCES_FILE=str(client_dir/'preferences.dat'), CLIENT910_VARC_FILE=str(client_dir/'client-vars.dat'), CLIENT910_UID192_FILE=str(client_dir/'random.dat'), CLIENT910_OUT_TRACE='1',CLIENT910_UI_TRACE_INPUT='1', CLIENT910_WHEEL_INPUT=plan['wheel'],CLIENT910_UI_OPERATIONS=plan['operations'], CLIENT910_UI_CLICKS=plan['clicks'], CLIENT910_UI_HOVER=plan['hover'], CLIENT910_KEY_INPUT=plan['keys'], CLIENT910_SCREENSHOT_SERIES=shots)
    args = [str(CLIENT),'--renderer','modern','--direct-login','--lobby-port',str(LOBBY_PORT),'--world-port',str(WORLD_PORT),'--username',plan['accountKey'],'--password','password','--cache-dir',str(client_dir/'cache'),'--screenshot',str(WORK/'slayer-equipment.png')]
    for command in plan['commands']: args += ['--server-command',command]
    run(['bash',str(ROOT/'ref/independence/wired-guard.sh')],'wired-guard',ROOT,ENV)
    client = start(args,'client',ROOT/'tools/client910',client_env)
    deadline = time.monotonic()+MAX_SECONDS
    expected = WORK/f"slayer-equipment_{TAIL_SCREENSHOT_CYCLE:0{SCREENSHOT_DIGITS}d}.png"
    readback_cycle = None
    while True:
        log = (WORK/'client.log').read_text(errors='replace')
        if complete_cycle() >= TAIL_SCREENSHOT_CYCLE and expected.is_file() and f'screenshot written: {expected}' in log:
            if readback_cycle is None:
                readback_cycle = complete_cycle()
                result['readbackObservedAtCompletedCycle'] = readback_cycle
                save()
            # The native frame tail exits after the last asynchronous readback.
            if client.poll() == 0 or complete_cycle() >= readback_cycle + READBACK_FOLLOWUP_CYCLES:
                result['readbackCompletion'] = 'native clean screenshot-series exit' if client.poll() == 0 else 'later completed cycles'
                break
        if client.poll() is not None or time.monotonic()>deadline: raise RuntimeError(f'Incomplete client recording: completed={complete_cycle()}, exit={client.poll()}')
        time.sleep(POLL_SECONDS)
    world_log = (WORK/'world.log').read_text(errors='replace')
    if '[WORLD]: tick step' in world_log or 'Error:' in world_log or 'step failed' in world_log:
        raise RuntimeError('World step error; terminal image is not proof of valid gameplay')
    rows = [json.loads(line) for line in (WORK/'equipment-receipts.jsonl').read_text().splitlines()]
    players = [player for row in rows for player in row['players']]
    if not players or any(player['legacyCombatActive'] != ONE_ITEM or player['legacyInterfaceLayout'] != NO_BALANCE or player['slimHeaders'] != ONE_ITEM for player in players): raise RuntimeError('The content session must remain Legacy combat with the qualified saved normal interface preference')
    identity = plan['initialInstance']['key']
    def head(player):
        return player['worn'][HEAD_SLOT]
    def physical(player, item, slaying=ONE_ITEM, ferocious=ONE_ITEM):
        slot = head(player)
        return len(slot) > INSTANCE_FIELD and slot[ITEM_FIELD] == item and slot[COUNT_FIELD] == ONE_ITEM and slot[INSTANCE_FIELD]['key'] == identity and slot[INSTANCE_FIELD]['resources'] == {'slayingTeleports':slaying,'ferociousTeleports':ferocious}
    if not any(physical(player, plan['initialHelmet']) for player in players): raise RuntimeError('Initial native Wear was not observed')
    points = plan['initialPoints']
    for upgrade in plan['upgrades']:
        points -= upgrade['cost']
        if not any(physical(player, upgrade['fusedOutput']) and player['slayer']['points'] <= points and upgrade['unlock'] in player['slayer']['unlocks'] for player in players): raise RuntimeError('Native helmet tier upgrade/physical resource preservation absent')
    def has_items(player):
        return all(any(slot[ITEM_FIELD] == stack['item'] and slot[COUNT_FIELD] >= stack['count'] for slot in player['backpack']) for stack in plan['expectedItems'])
    if not any(has_items(player) and player['slayerXp'] == plan['expectedXp'] for player in players): raise RuntimeError('Native scaled rune/ammunition/XP purchase absent')
    if not any(head(player)[ITEM_FIELD] == plan['initialHelmet'] and len(head(player)) > INSTANCE_FIELD and head(player)[INSTANCE_FIELD]['key'] != identity and head(player)[INSTANCE_FIELD]['resources'] == {} and player['slayer']['points'] == plan['expectedPoints'] for player in players): raise RuntimeError('Native fusion purchase did not create an independent empty resource identity')
    final_item = plan['upgrades'][-ONE_ITEM]['fusedOutput']
    destination = plan['destination']['to']
    completed = [player for player in players if physical(player, final_item, NO_BALANCE, ONE_ITEM) and all(player[field] == destination[field] for field in ('x','z','level')) and player['slayer']['points'] == plan['expectedPoints'] and has_items(player)]
    if not completed: raise RuntimeError('Native worn teleport did not consume exactly its Slaying balance and arrive normally')
    final = completed[-ONE_ITEM]
    if final['saved']['worn'] != final['worn'] or final['saved']['slayer'] != final['slayer']: raise RuntimeError('Final physical resources/points differ from the durable account')
    if any(any(slot[ITEM_FIELD] == material['item'] and slot[COUNT_FIELD] > NO_BALANCE for slot in final['backpack']) for material in plan['materials']): raise RuntimeError('Native corrupt upgrade material was not consumed')
    result['observed'] = {'initialPhysicalKey':identity,'nativeTierUpgrades':len(plan['upgrades']),'nativeScaledRewards':plan['expectedItems'],'nativeXp':plan['expectedXp'],'nativeFusionCreatedEmptyIdentity':True,'independentSlayingTeleport':True,'durableFinal':final}
    result['completeCycle'] = complete_cycle()
    result['terminalScreenshot'] = {'path':str(expected),'sha256':hashlib.sha256(expected.read_bytes()).hexdigest(),'filenameRequestCycle':TAIL_SCREENSHOT_CYCLE,'readbackObservedAtCompletedCycle':readback_cycle,'laterCompletedCycle':complete_cycle(),'qualification':'Async screenshot readback; filename does not prove the exact submitted scene cycle'}
    finish(client, next(row for child,row in children if child is client))
    output=WORK/'processed'
    output.mkdir()
    run(['python3',str(HERE.parent/'postprocess.py'),'--until',str(plan['lastCycle']+NEXT_CYCLE_OFFSET),str(WORK/'raw.rtr'),str(WORK/'world.log'),str(output)],'postprocess',ROOT,ENV)
    shutil.copyfile(WORK/'equipment-receipts.jsonl',output/'equipment-receipts.jsonl')
    shutil.copyfile(WORK/'plan.json',output/'plan.json')
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
