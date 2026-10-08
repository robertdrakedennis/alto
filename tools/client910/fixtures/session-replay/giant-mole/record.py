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
ONE_CLIENT_LAUNCH = 1
LAST_CHILD_INDEX = -1
CHILD_LEDGER_INDEX = 1
STACK_ITEM_INDEX = 0
STACK_QUANTITY_INDEX = 1
INCOMPLETE_CYCLE = -1
RECORD_MAGIC_BYTES = 4
NEXT_CYCLE_OFFSET = 1
WINDOW_WIDTH, WINDOW_HEIGHT = 1024, 768
SCREENSHOT_DIGITS = 5
ROOT = HERE.parents[REPOSITORY_PARENT_INDEX]
TARGET = Path(os.environ['CARGO_TARGET_DIR'])
CLIENT = TARGET/'debug/client910'
import tempfile
WORK = Path(tempfile.mkdtemp(prefix='alto-mole-record-'))
LOBBY_PORT = int(os.environ.get('LOBBY_PORT','48600'))
WORLD_PORT = int(os.environ.get('WORLD_PORT','48601'))
LANE_PORT_FIRST, LANE_PORT_END = 48600, 48700
assert LANE_PORT_FIRST <= LOBBY_PORT < LANE_PORT_END and LANE_PORT_FIRST <= WORLD_PORT < LANE_PORT_END and LOBBY_PORT != WORLD_PORT
SERVER_START_SECONDS, MAX_SECONDS, CHILD_WAIT_SECONDS, POLL_SECONDS = 90, 600, 30, 1
HEADER_BYTES, RECORD_HEADER_BYTES = 8, 12
TAIL_SCREENSHOT_DELAY_CYCLES = 200
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
    row = {'name': name, 'pid': child.pid, 'arguments': arguments, 'waited': False}
    children.append((child, row))
    ledger.append(row)
    if name == 'client': result['clientLaunches'] += ONE_CLIENT_LAUNCH
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
    result['provenance'] = {'head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT,text=True).strip(), 'sourcePatchSha256': hashlib.sha256(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT)).hexdigest(), 'clientSha256': hashlib.sha256(CLIENT.read_bytes()).hexdigest(), 'sourceSha256': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [HERE/'seed.mjs',HERE/'server.mjs',ROOT/'server/src/lostcity/network/GiantMoleInputs.testkit.ts',ROOT/'server/src/lostcity/engine/devcommands/NpcSelection.ts',ROOT/'server/src/lostcity/engine/devcommands/SkillCommands.ts',ROOT/'server/src/lostcity/engine/devcommands/PlayerCommands.ts',ROOT/'server/src/lostcity/content/world/bosses/MoleEncounter.ts',ROOT/'server/src/lostcity/content/world/bosses/MoleRewards.ts',ROOT/'server/src/lostcity/content/world/bosses/BossRooms.ts',ROOT/'server/src/lostcity/systems/npc/NpcPopulation.ts',ROOT/'server/src/lostcity/systems/loot/DropTables.ts',ROOT/'server/src/lostcity/systems/trails/ClueDrops.ts',ROOT/'server/src/lostcity/systems/consumables/Consumables.ts',ROOT/'server/src/lostcity/systems/stats/Stats.ts',ROOT/'server/src/lostcity/systems/stats/Regeneration.ts',ROOT/'server/src/lostcity/systems/inventory/InventoryOperation.ts',ROOT/'server/src/lostcity/systems/combat/Combat.ts',ROOT/'server/src/lostcity/systems/combat/PlayerCombat.ts',ROOT/'server/src/lostcity/data/FoodData.ts']}, 'generatedInputs': {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT/'server/data/generated/instances').glob('*.json'))+sorted((ROOT/'server/data/generated/combat').glob('*.json'))+sorted((ROOT/'server/data/generated/npcs').glob('*.json'))+sorted((ROOT/'server/data/generated/drops').glob('*.json'))+sorted((ROOT/'server/data/generated/food').glob('*.json'))}, 'runnerSha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    (WORK/'source.patch').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    run(['node',str(HERE/'seed.mjs'),str(WORK)],'seed',ROOT/'server',ENV)
    plan = json.loads((WORK/'plan.json').read_text())
    tail_screenshot_cycle = plan['lastCycle'] + TAIL_SCREENSHOT_DELAY_CYCLES
    start(['node','src/lostcity/lobby.ts'],'lobby',ROOT/'server',ENV)
    start(['node',str(HERE/'server.mjs'),str(WORK)],'world',ROOT/'server',ENV)
    deadline = time.monotonic()+SERVER_START_SECONDS
    while not all(f'listening on port {port}' in (WORK/f'{name}.log').read_text().lower() for name,port in [('lobby',LOBBY_PORT),('world',WORLD_PORT)]):
        if any(child.poll() is not None for child,row in children if row['name'] in ('lobby','world')) or time.monotonic()>deadline: raise RuntimeError('Server readiness failed')
        time.sleep(POLL_SECONDS)
    client_dir = WORK/'client'
    client_dir.mkdir()
    run(['bash',str(ROOT/'ref/independence/wired-guard.sh')],'wired-guard',ROOT,ENV)
    shots = ','.join(map(str,[*plan['semanticShots'],tail_screenshot_cycle]))
    client_env = dict(ENV, CLIENT910_RECORD=str(WORK/'raw.rtr'), CLIENT910_LOG='info', CLIENT910_WINDOW_SIZE=f'{WINDOW_WIDTH},{WINDOW_HEIGHT}', CLIENT910_PREFERENCES_FILE=str(client_dir/'preferences.dat'), CLIENT910_VARC_FILE=str(client_dir/'client-vars.dat'), CLIENT910_UID192_FILE=str(client_dir/'random.dat'), CLIENT910_OUT_TRACE='1',CLIENT910_UI_TRACE_INPUT='1', CLIENT910_UI_OPERATIONS=plan['operations'], CLIENT910_UI_CLICKS=plan['clicks'], CLIENT910_UI_HOVER=plan['hover'], CLIENT910_KEY_INPUT=plan['keys'], CLIENT910_SCREENSHOT_SERIES=shots)
    args = [str(CLIENT),'--renderer','modern','--direct-login','--lobby-port',str(LOBBY_PORT),'--world-port',str(WORLD_PORT),'--username',plan['accountKey'],'--password','password','--cache-dir',str(client_dir/'cache'),'--screenshot',str(WORK/'giant-mole.png')]
    for command in plan['commands']: args += ['--server-command',command]
    client = start(args,'client',ROOT/'tools/client910',client_env)
    deadline = time.monotonic()+MAX_SECONDS
    expected = WORK/f"giant-mole_{plan['lastCycle']:0{SCREENSHOT_DIGITS}d}.png"
    while True:
        log = (WORK/'client.log').read_text(errors='replace')
        if complete_cycle() >= plan['lastCycle'] and expected.is_file() and f'screenshot written: {expected}' in log: break
        if client.poll() is not None or time.monotonic()>deadline: raise RuntimeError(f'Incomplete client recording: completed={complete_cycle()}, exit={client.poll()}')
        time.sleep(POLL_SECONDS)
    world_log = (WORK/'world.log').read_text(errors='replace')
    if '[WORLD]: tick step' in world_log or 'Error:' in world_log or 'step failed' in world_log:
        raise RuntimeError('World step error; terminal image is not proof of valid gameplay')
    receipts = [json.loads(line) for line in (WORK/'combat-receipts.jsonl').read_text().splitlines() if line.strip()]
    states = [row for row in receipts if row.get('kind') == 'state' and row.get('phase') == 'npcs']
    boss_rows = [enemy for row in states for enemy in row['enemies'] if enemy['definition'] in (plan['boss']['normal'], plan['boss']['enraged'])]
    player_rows = [player for row in states for player in row['players']]
    full_health = any(enemy['hitpoints'] == plan['boss']['hitpoints'] for enemy in boss_rows)
    death = any(row.get('kind') == 'death' and row.get('targetDefinition') in (plan['boss']['normal'], plan['boss']['enraged']) for row in receipts)
    defeated = next((row for row in receipts if row.get('kind') == 'death' and row.get('targetDefinition') in (plan['boss']['normal'], plan['boss']['enraged'])), None)
    homes = {(enemy['home']['x'], enemy['home']['z']) for row in states if defeated is not None and row['tick'] <= defeated['tick'] for enemy in row['enemies'] if enemy['id'] == defeated['targetId']}
    expected_homes = {(home['x'], home['z']) for home in plan['boss']['homes']}
    visited_homes = expected_homes.issubset(homes)
    bones = any(any(stack[STACK_ITEM_INDEX] == plan['boss']['bones'] and stack[STACK_QUANTITY_INDEX] > INITIAL_LAUNCH_COUNT for stack in player['backpack']) for player in player_rows)
    food_count = lambda player: sum(stack[STACK_QUANTITY_INDEX] for stack in player['backpack'] if stack[STACK_ITEM_INDEX] == plan['food']['item'])
    food_consumptions = []
    prior_player = None
    prior_player_tick = None
    for state in states:
        for player in state['players']:
            if prior_player is not None and player['pid'] == prior_player['pid'] and player['instance'] is not None and player['instance'] == prior_player['instance']:
                consumed = food_count(prior_player) - food_count(player)
                if consumed > INITIAL_LAUNCH_COUNT:
                    food_consumptions.append({'tick':state['tick'],'consumed':consumed,'lifeBefore':prior_player['life'],'lifeAfter':player['life'],'regeneration':player['regeneration'],'observationGapTicks':state['tick']-prior_player_tick,'aboveOrdinaryRegeneration':state['tick']-prior_player_tick <= ONE_CLIENT_LAUNCH and player['life'] - prior_player['life'] > player['regeneration']})
            prior_player = player
            prior_player_tick = state['tick']
    food_healing = any(receipt['aboveOrdinaryRegeneration'] for receipt in food_consumptions)
    membership = []
    for player in player_rows:
        current = player['instance']
        if not membership or current != membership[LAST_CHILD_INDEX]: membership.append(current)
    MINIMUM_MEMBERSHIP_TRANSITIONS = 4
    PENULTIMATE_TRANSITION = -2
    rejoined = len(membership) >= MINIMUM_MEMBERSHIP_TRANSITIONS and membership[INITIAL_LAUNCH_COUNT] is None and membership[ONE_CLIENT_LAUNCH] is not None and membership[PENULTIMATE_TRANSITION] is None and membership[LAST_CHILD_INDEX] == membership[ONE_CLIENT_LAUNCH]
    survived = bool(player_rows) and all(player['life'] > INITIAL_LAUNCH_COUNT for player in player_rows)
    result['serverAcceptance'] = {'fullHealth':full_health,'ordinaryDeath':death,'chamberHomes':len(homes),'allNamedHomesVisited':visited_homes,'bonesInBackpack':bones,'membership':membership,'rejoined':rejoined,'survived':survived,'foodConsumptions':food_consumptions,'foodHealingAboveOrdinaryRegeneration':food_healing}
    if not (full_health and death and visited_homes and bones and rejoined and survived and food_consumptions and food_healing):
        raise RuntimeError('Incomplete Mole gameplay semantics; inspect retained ordinary combat receipts')
    result['completeCycle'] = complete_cycle()
    result['terminalScreenshot'] = {'path':str(expected),'sha256':hashlib.sha256(expected.read_bytes()).hexdigest()}
    client.send_signal(signal.SIGTERM)
    code = client.wait(timeout=CHILD_WAIT_SECONDS)
    next(row for child,row in children if child is client).update(exit=code,waited=True,termination='SIGTERM after complete cycle + terminal readback')
    output=WORK/'processed'
    output.mkdir()
    run(['python3',str(HERE.parent/'postprocess.py'),'--until',str(plan['lastCycle']+NEXT_CYCLE_OFFSET),str(WORK/'raw.rtr'),str(WORK/'world.log'),str(output)],'postprocess',ROOT,ENV)
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
