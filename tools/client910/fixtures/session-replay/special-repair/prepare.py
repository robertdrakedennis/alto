#!/usr/bin/env python3
"""Bind finite retained selectors and helpers; no runtime stage is launched."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

SPECIAL_ROOT = Path(__file__).resolve().parents[5]
SPECIAL_TARGET = Path('/Users/robert/projects/alto-target-legacy-specials-gaps')
SPECIAL_HELPERS = Path(__file__).resolve().parent
SPECIAL_PRIVATE = SPECIAL_ROOT / '.cache/special-repair/helpers'
SPECIAL_NODE = Path('/Users/robert/Library/Application Support/Herd/config/nvm/versions/node/v24.18.0/bin')
SPECIAL_ONE = 1
SPECIAL_LOBBY_PORT = 48630
SPECIAL_WORLD_PORT = 48631
SPECIAL_MAX_LAUNCHES = 4
SPECIAL_SOCKET_PATH_BYTES = 104


def special_binding(path):
    path = Path(path).resolve(strict=True)
    return {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def special_require(condition, reason):
    if not condition:
        raise ValueError(reason)


def special_main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('binary-manifest', 'source-selectors', 'out', 'spec'):
        parser.add_argument('--' + name, required=True, type=Path)
    args = parser.parse_args()
    special_require(args.out.is_absolute() and not args.out.exists() and args.out.is_relative_to(SPECIAL_TARGET / 'proof'), 'Fresh short durable own output required')
    special_require(SPECIAL_ROOT != Path('/Users/robert/projects/alto'), 'Main runtime is forbidden')
    special_require(all(len(str(args.out / 'runtime' / name).encode()) < SPECIAL_SOCKET_PATH_BYTES for name in ('p.sock','c.sock')), 'Native socket pathname exceeds bound')
    build = json.loads(args.binary_manifest.read_text())
    special_require(build['sourceRoot'] == str(SPECIAL_ROOT) and build['sources'], 'Own compiled source manifest required')
    binary = special_binding(build['binary']['path'])
    special_require(binary == {key:build['binary'][key] for key in ('path','sha256')}, 'Compiled binary differs')
    special_require(Path(binary['path']).is_relative_to(SPECIAL_TARGET), 'Another lane binary/target is forbidden')
    retained = json.loads(args.source_selectors.read_text())
    special_require(isinstance(retained, dict) and retained, 'Finite retained source selectors required')
    selectors = set(retained) | set(build['sources'])
    changed = subprocess.check_output(['git', 'diff', '--name-only', 'main'], cwd=SPECIAL_ROOT, text=True).splitlines()
    selectors.update(changed)
    special_require(not subprocess.check_output(['git', 'status', '--porcelain'], cwd=SPECIAL_ROOT, text=True).strip(), 'Clean pre-capture source checkpoint required')
    for relative in selectors:
        selector = Path(relative)
        special_require(not selector.is_absolute() and '..' not in selector.parts, 'Unsafe source selector')
    selectors.update(str(path.relative_to(SPECIAL_ROOT)) for path in [SPECIAL_HELPERS / name for name in ('seed.mjs','server.mjs','driver.py','prepare.py','record.py','freeze.py')])
    selectors.add('tools/client910/src/scenario_special_repair.rs')
    owners = {name: SPECIAL_HELPERS / filename for name, filename in {'record':'record.py','freeze':'freeze.py','seed':'seed.mjs','server':'server.mjs','driver':'driver.py'}.items()}
    owners.update(owner=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/barrows/record.py',
        broker=SPECIAL_PRIVATE/'peer_broker.mjs', control=SPECIAL_ROOT/'tools/client910/scripts/control.py',
        endgameFixture=SPECIAL_ROOT/'server/src/lostcity/systems/equipment/EndgameFixture.testkit.ts',
        sharedDriver=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/barrows/driver.py',
        repairReceipts=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/barrows/repair-receipts.mjs',
        foodReceipts=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/barrows/food-receipts.mjs',
        peerAdapter=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/god-wars/peer_adapter.mjs',
        loginProvider=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/god-wars/login_provider.mjs',
        publications=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/god-wars/publications.py',
        loginTestkit=SPECIAL_ROOT/'server/src/lostcity/network/SkillsE2E.testkit.ts',
        socketTestkit=SPECIAL_ROOT/'server/src/lostcity/network/SocketE2E.testkit.ts',
        loginCrypto=SPECIAL_ROOT/'server/src/lostcity/network/LoginCrypto.ts',
        deviceEnvelope=SPECIAL_ROOT/'tools/client910/fixtures/session-replay/legacy-interface/session.rtr',
        wiredGuard=SPECIAL_ROOT/'ref/independence/wired-guard.sh', binaryManifest=args.binary_manifest,
        sourceSelectors=args.source_selectors)
    files = {name:special_binding(path) for name,path in owners.items()}
    files['binary'] = binary
    for relative, expected in build['sources'].items():
        special_require(special_binding(SPECIAL_ROOT / relative)['sha256'] == expected, 'Compiled source changed: '+relative)
    ledger = SPECIAL_TARGET/'proof/launch-ledger.jsonl'
    prior = [json.loads(line) for line in ledger.read_text().splitlines() if line.strip()] if ledger.exists() else []
    special_require(len(prior) < SPECIAL_MAX_LAUNCHES, 'Four actual client launches exhausted')
    source_inputs = {relative:special_binding(SPECIAL_ROOT/relative)['sha256'] for relative in sorted(selectors)}
    spec = {'root':str(SPECIAL_ROOT), 'out':str(args.out), 'files':files,
        'sourceCommit':subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=SPECIAL_ROOT, text=True).strip(),
        'sourceChanges':changed,
        'ports':{'lobby':SPECIAL_LOBBY_PORT,'world':SPECIAL_WORLD_PORT}, 'launchLedger':str(ledger),
        'sourceInputs':source_inputs, 'seedArgs':[], 'executionApproved':False,
        'priorCaptureDirectories':[row['out'] for row in prior],
        'environment':{'PATH':str(SPECIAL_NODE)+':/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin','CARGO_BUILD_JOBS':'2','NEXTEST_TEST_THREADS':'1'},
        'driverArgs':['--plan','{work}/plan.json','--food-rules',str(SPECIAL_ROOT/'server/data/generated/food/foods.json'),
            '--symbols',str(SPECIAL_ROOT/'revisions/910/symbols'),'--receipts','{work}/combat-receipts.jsonl',
            '--journal','{out}/driver-journal.jsonl','--control-module',files['control']['path'],'--socket','{control}'],
        'launchesSpentBeforeRun':len(prior),'qualification':'Source proposal; explicit lead runtime lease is still required'}
    with args.spec.open('x',encoding='utf8') as output:
        json.dump(spec,output,sort_keys=True,indent=SPECIAL_ONE+SPECIAL_ONE);output.write('\n')
    print(json.dumps({'spec':special_binding(args.spec),'executionApproved':False,'launchesSpent':len(prior)}))


if __name__ == '__main__':
    special_main()
