#!/usr/bin/env python3
"""Record NPC ranged and magic using tracked child PIDs; retain scratch evidence on failure."""
import json
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
LOBBY_PORT = int(os.environ.get('LOBBY_PORT','48282'))
WORLD_PORT = int(os.environ.get('WORLD_PORT','48283'))
MAX_SECONDS = 420
SERVER_START_SECONDS = 90
POLL_SECONDS = 1
HEADER_BYTES = 8
RECORD_HEADER_BYTES = 12
SHOTS = '350,500,800,1350,1500,2000,2300'
assert 48000 <= LOBBY_PORT < 49000 and 48000 <= WORLD_PORT < 49000
WORK = Path(tempfile.mkdtemp(prefix='alto-npc-styles-record-'))
print(f'Recording scratch: {WORK}',flush=True)
TARGET = Path(os.environ['CARGO_TARGET_DIR'])
ENV = dict(os.environ,ALTO_LOBBY_PORT=str(LOBBY_PORT),ALTO_WORLD_PORT=str(WORLD_PORT),ALTO_PLAYER_DATA_DIR=str(WORK/'players'),ALTO_TRACE_INFO='1',ALTO_DEV_NPCS='0')
children = []
logs = []

def start(arguments, name, cwd, environment):
    log = (WORK/f'{name}.log').open('w')
    logs.append(log)
    child = subprocess.Popen(arguments,cwd=cwd,env=environment,stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT)
    children.append(child)
    print(f"Owned {name} PID: {child.pid}",flush=True)
    return child

def last_cycle():
    raw = WORK/'raw.rtr'
    if not raw.exists(): return -1
    data = raw.read_bytes()
    at, last = HEADER_BYTES, -1
    while data[:4] == b'RTR1' and at+RECORD_HEADER_BYTES <= len(data):
        cycle,tag,length = struct.unpack('<i4sI',data[at:at+RECORD_HEADER_BYTES])
        if at+RECORD_HEADER_BYTES+length > len(data): break
        last = max(last,cycle)
        at += RECORD_HEADER_BYTES+length
    return last

try:
    subprocess.run(['node',str(HERE/'seed.mjs'),str(WORK)],cwd=ROOT/'server',env=ENV,check=True)
    plan = json.loads((WORK/'plan.json').read_text())
    if max(map(int, SHOTS.split(','))) <= plan['lastCycle']: raise RuntimeError('The final screenshot must follow the replay end')
    start(['node','src/lostcity/lobby.ts'],'lobby',ROOT/'server',ENV)
    start(['node',str(HERE/'server.mjs')],'world',ROOT/'server',ENV)
    deadline = time.monotonic()+SERVER_START_SECONDS
    while not all(f'listening on port {port}' in (WORK/f'{name}.log').read_text().lower() for name,port in [('lobby',LOBBY_PORT),('world',WORLD_PORT)]):
        if any(child.poll() is not None for child in children) or time.monotonic()>deadline: raise RuntimeError('Servers failed to start; inspect scratch logs')
        time.sleep(POLL_SECONDS)
    client_dir = WORK/'client'
    client_dir.mkdir()
    warm_cache = os.environ.get('ALTO_RECORD_WARM_CACHE')
    if warm_cache: shutil.copytree(warm_cache, client_dir/'cache')
    preferences = ROOT/'server/data/players/preferences.dat'
    if preferences.exists(): shutil.copyfile(preferences,client_dir/'preferences.dat')
    subprocess.run(['bash',str(ROOT/'ref/independence/wired-guard.sh')],check=True)
    client_env = dict(ENV,CLIENT910_RECORD=str(WORK/'raw.rtr'),CLIENT910_WINDOW_SIZE='1024,768',CLIENT910_PREFERENCES_FILE=str(client_dir/'preferences.dat'),CLIENT910_VARC_FILE=str(client_dir/'client-vars.dat'),CLIENT910_UID192_FILE=str(client_dir/'random.dat'),CLIENT910_UI_OPERATIONS=plan['operations'],CLIENT910_SCREENSHOT_SERIES=SHOTS)
    arguments = [str(TARGET/'debug/client910'),'--direct-login','--lobby-port',str(LOBBY_PORT),'--world-port',str(WORLD_PORT),'--username',plan['accountKey'],'--password','password','--cache-dir',str(client_dir/'cache'),'--screenshot',str(WORK/'npc-styles.png')]
    for command in plan['commands']: arguments += ['--server-command',command]
    client = start(arguments,'client',ROOT/'tools/client910',client_env)
    deadline = time.monotonic()+MAX_SECONDS
    while last_cycle()<plan['lastCycle']:
        if client.poll() is not None or time.monotonic()>deadline: raise RuntimeError('Client did not reach final cycle; inspect scratch logs')
        time.sleep(POLL_SECONDS)
    client.send_signal(signal.SIGTERM)
    client.wait(timeout=30)
    subprocess.run(['python3',str(HERE.parent/'postprocess.py'),'--until',str(plan['lastCycle']),str(WORK/'raw.rtr'),str(WORK/'world.log'),str(HERE)],check=True)
    print(f'Recorded NPC ranged and magic; supporting logs/screenshots remain at {WORK}',flush=True)
finally:
    for child in reversed(children):
        if child.poll() is None: child.send_signal(signal.SIGTERM)
    for child in reversed(children): child.wait(timeout=30)
    for log in logs: log.close()
