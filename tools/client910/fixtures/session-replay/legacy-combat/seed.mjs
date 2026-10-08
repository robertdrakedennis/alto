import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {component, location, npc, obj, struct} from '@alto/domain';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import SkillSet, {loadSkillDefinitions} from '../../../../../server/src/lostcity/systems/stats/SkillSet.ts';
import {GameplaySettingsDefinitions} from '../../../../../server/src/lostcity/definitions/GameplaySettingsDefinitions.ts';
import {loadCombatData} from '../../../../../server/src/lostcity/data/CombatData.ts';
const ONE_ITEM = 1;
const TWO = 2;
const FORMAT = 2;
const ATTACK = 0;
const DEFENCE = 1;
const STRENGTH = 2;
const LEVEL_TEN_XP = 1154;
const AMMO_COUNT = 4;
const COMMAND_COUNT = 45;
const CYCLE = {
    RETALIATION_OFF: 300,
    MELEE_EQUIP: 1000,
    MELEE_SETTINGS: 1100,
    MELEE_CATEGORY: 1150,
    MELEE_STRENGTH: 1200,
    MELEE_DEFENCE: 1250,
    MELEE_CLOSE: 1300,
    MELEE_REVIEW: 1400,
    MELEE_SPAWN: 1500,
    MELEE_ATTACK: 1600,
    MELEE_RESULT: 1800,
    RANGED_EQUIP: 2000,
    RANGED_AMMO: 2050,
    RANGED_SETTINGS: 2100,
    RANGED_CATEGORY: 2150,
    RANGED_DEFENCE: 2200,
    RANGED_CLOSE: 2300,
    RANGED_REVIEW: 2350,
    RANGED_SPAWN: 2400,
    RANGED_ATTACK: 2500,
    RANGED_RESULT: 2750,
    MAGIC_EQUIP: 3000,
    MAGIC_AUTOCAST: 3100,
    MAGIC_SETTINGS: 3200,
    MAGIC_CATEGORY: 3250,
    MAGIC_DEFENCE: 3300,
    MAGIC_CLOSE: 3400,
    MAGIC_REVIEW: 3450,
    MAGIC_SPAWN: 3500,
    MAGIC_ATTACK: 3600,
    MAGIC_RESULT: 3850,
    RETALIATION_ON: 3900,
    FINAL_REVIEW: 4300,
};
const LAST_CYCLE = CYCLE.FINAL_REVIEW;
const COMMAND_PERIOD = 100;
const FIRST_OPERATION = 1;
const EQUIP_OPERATION = 2;
const AUTOCAST_OPERATION = 2;
const CLOSE_CHILD = 1;
const NO_CHILD = -1;
const ESCAPE_KEY = 27;
const KEY_PRESS = 0;
const KEY_RELEASE = 1;
const NEXT_CYCLE = 1;
const MENU_LEAD_CYCLES = 20;
const BACKPACK_DAGGER = 0;
const BACKPACK_BOW = 1;
const BACKPACK_ARROWS = 2;
const BACKPACK_STAFF = 3;
const work = process.argv[TWO];
const accountKey = 'legacy-rules';
await CacheProvider.load('data/pack');
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const definitions = await loadSkillDefinitions(CacheProvider.js5);
let skills = new SkillSet(definitions);
for (const stat of [ATTACK, DEFENCE, STRENGTH]) skills = skills.withXp(stat, LEVEL_TEN_XP);
const directory = path.join(work, 'players', 'accounts');
fs.mkdirSync(directory, {recursive: true});
fs.writeFileSync(path.join(directory, `${createHash('sha256').update(accountKey).digest('hex')}.json`), JSON.stringify({version: FORMAT, accountKey, ...location.dev_player_spawn, skills: skills.values, backpack: [[obj.bronze_dagger, ONE_ITEM], [obj.shortbow, ONE_ITEM], [obj.bronze_arrow, AMMO_COUNT], [obj.plain_staff, ONE_ITEM], [obj.air_rune, AMMO_COUNT]]}));
const commands = Array.from({length: COMMAND_COUNT}, () => 'notimeout');
const at = (cycle, command) => { commands[cycle / COMMAND_PERIOD - ONE_ITEM] = command; };
at(CYCLE.RETALIATION_OFF, 'retaliate off');

at(CYCLE.MELEE_SPAWN, `npcadd ${npc.chicken}`);
at(CYCLE.MELEE_ATTACK, `opnpc ${config.npc(npc.chicken).op.findIndex(value => value?.toLowerCase() === 'attack') + ONE_ITEM} ${npc.chicken}`);

at(CYCLE.RANGED_SPAWN, `npcadd ${npc.chicken}`);
at(CYCLE.RANGED_ATTACK, `opnpc ${config.npc(npc.chicken).op.findIndex(value => value?.toLowerCase() === 'attack') + ONE_ITEM} ${npc.chicken}`);

at(CYCLE.MAGIC_SPAWN, `npcadd ${npc.chicken}`);
at(CYCLE.MAGIC_ATTACK, `opnpc ${config.npc(npc.chicken).op.findIndex(value => value?.toLowerCase() === 'attack') + ONE_ITEM} ${npc.chicken}`);
at(CYCLE.RETALIATION_ON, 'retaliate on');
const settings = GameplaySettingsDefinitions.of(config);
const route = definition => {
 const row = settings.categories.find(category => category.settings.includes(definition));
 if (!row) throw new Error('Missing native training route');
 return {category: row.index, slot: row.settings.indexOf(definition)};
};
const melee = route(struct.strength_training_option);
const ranged = route(struct.ranged_defence_training_option);
const magic = route(struct.magic_defence_training_option);
const spell = loadCombatData().spells.find(row => row.name === 'Air Strike');
if (!spell) throw new Error('Missing generated Air Strike');
const keys = [CYCLE.MELEE_SETTINGS,CYCLE.RANGED_SETTINGS,CYCLE.MAGIC_SETTINGS].flatMap(cycle => [[cycle - MENU_LEAD_CYCLES,ESCAPE_KEY,KEY_PRESS],[cycle - MENU_LEAD_CYCLES + NEXT_CYCLE,ESCAPE_KEY,KEY_RELEASE]]);
const operations = [
 ...[CYCLE.MELEE_SETTINGS,CYCLE.RANGED_SETTINGS,CYCLE.MAGIC_SETTINGS].map(cycle => [cycle,component.options_menu.settings_button,NO_CHILD,FIRST_OPERATION]),
 [CYCLE.MELEE_EQUIP, component.backpack.slots, BACKPACK_DAGGER, EQUIP_OPERATION],
 [CYCLE.MELEE_CATEGORY, component.gameplay_settings.categories, melee.category, FIRST_OPERATION],
 [CYCLE.MELEE_STRENGTH, component.gameplay_settings.options, melee.slot, FIRST_OPERATION],
 [CYCLE.MELEE_DEFENCE, component.gameplay_settings.options, route(struct.melee_defence_training_option).slot, FIRST_OPERATION],
 [CYCLE.MELEE_CLOSE, component.game_window.modal_close_button, CLOSE_CHILD, FIRST_OPERATION],
 [CYCLE.RANGED_EQUIP, component.backpack.slots, BACKPACK_BOW, EQUIP_OPERATION],
 [CYCLE.RANGED_AMMO, component.backpack.slots, BACKPACK_ARROWS, EQUIP_OPERATION],
 [CYCLE.RANGED_CATEGORY, component.gameplay_settings.categories, ranged.category, FIRST_OPERATION],
 [CYCLE.RANGED_DEFENCE, component.gameplay_settings.options, ranged.slot, FIRST_OPERATION],
 [CYCLE.RANGED_CLOSE, component.game_window.modal_close_button, CLOSE_CHILD, FIRST_OPERATION],
 [CYCLE.MAGIC_EQUIP, component.backpack.slots, BACKPACK_STAFF, EQUIP_OPERATION],
 [CYCLE.MAGIC_AUTOCAST, component.magic_book.actions, spell.action, AUTOCAST_OPERATION],
 [CYCLE.MAGIC_CATEGORY, component.gameplay_settings.categories, magic.category, FIRST_OPERATION],
 [CYCLE.MAGIC_DEFENCE, component.gameplay_settings.options, magic.slot, FIRST_OPERATION],
 [CYCLE.MAGIC_CLOSE, component.game_window.modal_close_button, CLOSE_CHILD, FIRST_OPERATION]
];
fs.writeFileSync(path.join(work, 'plan.json'), JSON.stringify({accountKey, commands, operations: operations.map(row => row.join(',')).join(';'), keys: keys.map(row=>row.join(',')).join(';'), lastCycle: LAST_CYCLE, semanticShots: [CYCLE.MELEE_REVIEW,CYCLE.MELEE_RESULT,CYCLE.RANGED_REVIEW,CYCLE.RANGED_RESULT,CYCLE.MAGIC_REVIEW,CYCLE.MAGIC_RESULT,CYCLE.FINAL_REVIEW], retaliationQualification: 'The recording proves preference/varp publication; actual acquisition is independently covered by real-socket E2E.', inputQualification: 'Native interface operations; recorded developer interaction commands choose the same ordinary NPC interaction as OPNPC. Real OPNPC TCP proof is separate.'}, null, TWO));
