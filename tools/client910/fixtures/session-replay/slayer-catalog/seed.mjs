// Fresh initial fixture only. This file never writes a logged-in account.
import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';

const FIRST_ARGUMENT = 2;
const FIRST = 0;
const ONE = 1;
const ACCOUNT_VERSION = 2;
const RETALIATION_DISABLED = 1;
const INDENT = 2;
const ACCOUNT = 'alice';
const PRAYER_WINDOW_TITLE = 'Prayer Abilities';
const root = process.env.ALTO_BOSS_RUNTIME_ROOT;
const work = process.argv[FIRST_ARGUMENT];
if (!root || !path.isAbsolute(root) || !work || !path.isAbsolute(work)) throw new Error('Owned runtime and fresh scratch required');
const owner = relative => import(pathToFileURL(path.join(root, relative)).href);
const [{default: CacheProvider}, {slayerCatalogFixture}, {struct, enums, component, varp}] = await Promise.all([
    owner('server/src/lostcity/server/CacheProvider.ts'),
    owner('server/src/lostcity/systems/slayer/SlayerFixture.testkit.ts'),
    owner('packages/domain/src/index.ts')
]);
await CacheProvider.load(path.join(root, 'server/data/pack'));
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const selected = await slayerCatalogFixture();
const {fixture, master, prayers, reward, assignment, initialState, initialPoints, previousStreak, expectedCompletionPoints} = selected;
const windows = config.enum(enums.native_interface_windows);
const windowRows = new Map(windows.valuesArray.map((value, key) => [key, value]));
for (const [key, value] of windows.valuesMap) windowRows.set(key, value);
const destination = definition => {
    const matches = [...windowRows].filter(([, value]) => value === definition);
    if (matches.length !== ONE) throw new Error('Ambiguous native window destination');
    return matches[FIRST][FIRST];
};
const prayerWindows = [...windowRows].filter(([, definition]) => {
    const values = [...config.struct(definition).params.values()];
    return values.includes(PRAYER_WINDOW_TITLE) && values.includes(component.game_window.prayer_slot);
});
if (prayerWindows.length !== ONE) throw new Error('Ambiguous native Prayer destination');
const account = {version: ACCOUNT_VERSION, accountKey: ACCOUNT, members: true,
    level: selected.startTile.level, x: selected.startTile.x, z: selected.startTile.z,
    skills: fixture.skills, backpack: fixture.backpack, worn: fixture.worn,
    savedVarps: [[varp.auto_retaliate_disabled, RETALIATION_DISABLED]],
    slayer: initialState};
const plan = {accountKey: ACCOUNT, master: {...selected.start, code: master.code, name: master.name,
        taskOperation: selected.taskOperation},
    navigationDoors: selected.navigationDoors,
    target: {npc: selected.spawn.npc, spawn: selected.spawn, profile: selected.profile, task: selected.task},
    equipment: fixture.equipment.map(row => row.item), equipmentFacts: fixture.equipment, loadout: fixture.loadout,
    food: fixture.food, prayers, reward, blockedResources: fixture.blockedResources,
    toolbar: {backpackDestination: destination(struct.backpack_window),
        prayerDestination: prayerWindows[FIRST][FIRST],
        combatDestination: destination(struct.melee_ability_window), retaliationVarp: varp.auto_retaliate_disabled},
    assignment, initialPoints, previousStreak, expectedCompletionPoints,
    qualification: 'Initial previously-earned points and a task with one kill left only; ordinary NPC spawn/profile, RNG, life, resources and native input remain unchanged',
    spawnAuthority: 'Existing generated spawn coordinates retain their actual source labels; this sample does not upgrade current-Wiki geometry to dated authority'};
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const directory = path.join(work, 'players/accounts');
fs.mkdirSync(directory, {recursive: true});
const accountPath = path.join(directory, sha(Buffer.from(ACCOUNT)) + '.json');
const write = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, INDENT), {flag: 'wx'});
write(accountPath, account);
write(path.join(work, 'plan.json'), plan);
write(path.join(work, 'initial-fixture.json'), {account, accountPath, accountSha256: sha(fs.readFileSync(accountPath)), plan,
    inputs: Object.fromEntries(['slayer/rules.json', 'equipment/endgame-loadouts.json', 'combat/rules.json', 'combat/charges.json', 'npcs/profiles.json', 'spawns/spawns.json']
        .map(relative => [relative, sha(fs.readFileSync(path.join(root, 'server/data/generated', relative)))]))});
