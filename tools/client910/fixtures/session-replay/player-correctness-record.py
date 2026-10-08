#!/usr/bin/env python3
"""Record wood, farm or npcs with owned child PIDs and retained scratch evidence.
Usage: CARGO_TARGET_DIR=<lane target> python3 record.py <wood|farm|npcs>
"""
import hashlib, json, os, shutil, signal, struct, subprocess, sys, tempfile, time
from pathlib import Path
ROOT = Path(__file__).resolve().parents[4]
TARGET = Path(os.environ['CARGO_TARGET_DIR'])
MODE = sys.argv[1]
WORK = Path(tempfile.mkdtemp(prefix='alto-correctness-'+MODE+'-'))
OUT = ROOT/'tools/client910/fixtures/session-replay'/('correctness-'+MODE)
OUT.mkdir(exist_ok=True)
D = json.loads(subprocess.check_output(['node', '--input-type=module', '-e', 'import {location,obj,npc,component,loc} from "@alto/domain"; console.log(JSON.stringify({location,obj,npc,component,loc}));'], cwd=ROOT/'server', text=True))
ports = {'wood':(48080,48081),'farm':(48082,48083),'npcs':(48084,48085)}[MODE]
print(str(WORK),flush=True)
ENV = dict(os.environ,ALTO_LOBBY_PORT=str(ports[0]),ALTO_WORLD_PORT=str(ports[1]),ALTO_PLAYER_DATA_DIR=str(WORK/'players'),ALTO_TRACE_INFO='1',ALTO_NPC_SPAWNS='dev',ALTO_DEV_NPC_WANDER='0',ALTO_WC_ALWAYS='1',ALTO_SKILLS_ALWAYS='1')
KEY = 'correctness'+MODE
wear = [-1]*19
for slot,kit in [(4,19),(6,27),(7,37),(8,1),(9,34),(10,43),(11,11)]: wear[slot] = -2147483648 | kit
seed = dict(version=2,accountKey=KEY,level=0,backpack=[[-1,0]]*28,worn=[[-1,0]]*19,coins=0,identityAppearance=dict(wear=wear,colours=[1,2,3,4,0,0,0,0,0,0],textures=[0]*10))
commands = ['notimeout']; operations = ''; clicks = ''; hover = ''
if MODE == 'wood':
    seed.update({k:D['location']['lumbridge_common_tree_east'][k] for k in ['x','z','level']});seed['coins']=1000
    seed['backpack'][1]=[D['obj']['coins'],2500]
    commands += [f"invset 0 {D['obj']['bronze_hatchet']} 1", 'setxp 8 58', f"oploc 1 {D['location']['lumbridge_common_tree']['x']} {D['location']['lumbridge_common_tree']['z']} {D['loc']['tree']}"]
    clicks='';hover=''
    operations=f"650,{D['component']['backpack']['slots']},1,1"
    shots='450,480,510,540,570,600,700,800,900';last=900
elif MODE == 'farm':
    seed.update({k:D['location']['catherby_fruit_tree_patch_west'][k] for k in ['x','z','level']})
    seed['backpack'][0]=[D['obj']['rake'],1]
    patch=D['location']['catherby_fruit_tree_patch'];parent=D['loc']['catherby_fruit_tree_patch']
    commands += ['setxp 19 9730','notimeout', f"oploc 1 {patch['x']} {patch['z']} {parent}"]
    shots='480,510,540,570,600,630,700,800,850';last=850
else:
    seed.update({k:D['location']['dev_spawn_goblin_1'][k] for k in ['x','z','level']})
    seed['worn'][3]=[D['obj']['bronze_dagger'],1]
    commands += ['setxp 0 40000','setxp 2 1154',f"npcadd {D['npc']['cow']} 1 0",f"opnpc 2 {D['npc']['cow']}"]
    commands += ['notimeout']*6+[f"npcadd {D['npc']['goblin_level_2']} 0 1",f"opnpc 2 {D['npc']['goblin_level_2']}"]
    commands += ['notimeout']*6+[f"npcadd {D['npc']['chicken']} 1 0",f"opnpc 2 {D['npc']['chicken']}"]
    shots='520,570,620,720,1200,1250,1320,1420,2000,2050,2120,2200,2250,2350,2450,2500';last=2500
account=WORK/'players/accounts'/ (hashlib.sha256(KEY.encode()).hexdigest()+'.json');account.parent.mkdir(parents=True)
account.write_text(json.dumps(seed));(WORK/'plan.json').write_text(json.dumps(dict(commands=commands,operations=operations,clicks=clicks,hover=hover,last=last)))
children=[];logs=[]
def start(args,name,cwd,env):
    log=(WORK/(name+'.log')).open('w');logs.append(log)
    child=subprocess.Popen(args,cwd=cwd,env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT);children.append(child);print(name,child.pid,flush=True);return child
def cycle():
    p=WORK/'raw.rtr'
    if not p.exists():return -1
    data=p.read_bytes();pos=8;last=-1
    while data[:4]==b'RTR1' and pos+12<=len(data):
        c,t,n=struct.unpack('<i4sI',data[pos:pos+12])
        if pos+12+n>len(data):break
        last=max(last,c);pos+=12+n
    return last
try:
    start(['node','src/lostcity/lobby.ts'],'lobby',ROOT/'server',ENV)
    start(['node',str(OUT/'server.mjs')] if MODE=='npcs' else ['node','src/lostcity/world.ts'],'world',ROOT/'server',ENV)
    deadline=time.monotonic()+180
    while not all(f'listening on port {port}' in (WORK/(name+'.log')).read_text().lower() for name,port in [('lobby',ports[0]),('world',ports[1])]):
        if any(c.poll() is not None for c in children) or time.monotonic()>deadline:raise RuntimeError('server start failed')
        time.sleep(1)
    clientdir=WORK/'client';clientdir.mkdir()
    pref=Path(os.environ.get('CLIENT910_PREFERENCES_FILE',str(ROOT/'server/data/players/preferences.dat')))
    if pref.exists():shutil.copyfile(pref,clientdir/'preferences.dat')
    subprocess.run(['bash',str(ROOT/'ref/independence/wired-guard.sh')],check=True)
    clientenv=dict(ENV,CLIENT910_RECORD=str(WORK/'raw.rtr'),CLIENT910_WINDOW_SIZE='1024,768',CLIENT910_PREFERENCES_FILE=str(clientdir/'preferences.dat'),CLIENT910_VARC_FILE=str(clientdir/'client-vars.dat'),CLIENT910_UID192_FILE=str(clientdir/'random.dat'),CLIENT910_UI_OPERATIONS=operations,CLIENT910_UI_CLICKS=clicks,CLIENT910_UI_HOVER=hover,CLIENT910_SCREENSHOT_SERIES=shots)
    args=[str(TARGET/'debug/client910'),'--direct-login','--lobby-port',str(ports[0]),'--world-port',str(ports[1]),'--username',KEY,'--password','password','--cache-dir',str(clientdir/'cache'),'--screenshot',str(WORK/(MODE+'.png'))]
    for command in commands:args+=['--server-command',command]
    client=start(args,'client',ROOT/'tools/client910',clientenv);deadline=time.monotonic()+300
    while cycle()<last:
        if client.poll() is not None or time.monotonic()>deadline:raise RuntimeError('client stopped at cycle '+str(cycle()))
        time.sleep(1)
    client.send_signal(signal.SIGTERM);client.wait(timeout=30)
    subprocess.run(['python3',str(OUT.parent/'postprocess.py'),'--until',str(last),str(WORK/'raw.rtr'),str(WORK/'world.log'),str(OUT)],check=True)
    print('success',MODE,WORK,flush=True)
finally:
    for c in reversed(children):
        if c.poll() is None:c.send_signal(signal.SIGTERM)
    for c in reversed(children):c.wait(timeout=30)
    for log in logs:log.close()
