import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {component, location, npc, obj} from '@alto/domain';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import SkillSet, {loadSkillDefinitions} from '../../../../../server/src/lostcity/systems/stats/SkillSet.ts';
import {loadCombatData} from '../../../../../server/src/lostcity/data/CombatData.ts';

const FIRST_SLOT = 0;
const ONE_ITEM = 1;
const NO_CHILD = -1;
const EQUIP_OPERATION = 2;
const ACTIVATE_OPERATION = 1;
const ATTACK_OPERATION = 2;
const MAX_SKILL_XP = 13_034_431;
const ATTACK = 0;
const STRENGTH = 2;
const CONSTITUTION = 3;
const PRAYER = 5;
const LAST_CYCLE = 2200;
const COMMAND_COUNT = 20;
const FIRST_SPAWN_COMMAND = 1;
const FIRST_ATTACK_COMMAND = 3;
const SECOND_SPAWN_COMMAND = 11;
const SECOND_ATTACK_COMMAND = 13;
const SPAWN_OFFSET_TILES = 3;
const RANGED_PRAYER_CYCLE = 150;
const WEAPON_CYCLE = 200;
const MAGIC_PRAYER_CYCLE = 1200;
const accountKey = 'npc-styles';
const work = process.argv[2];
await CacheProvider.load('data/pack');
let skills = new SkillSet(await loadSkillDefinitions(CacheProvider.js5));
for (const skill of [ATTACK, STRENGTH, CONSTITUTION, PRAYER]) skills = skills.withXp(skill, MAX_SKILL_XP);
const rules = loadCombatData();
const ranged = rules.prayers.find(rule => rule.name === 'Protect from Missiles');
const magic = rules.prayers.find(rule => rule.name === 'Protect from Magic');
if (!ranged || !magic) throw new Error('Missing cache prayer recording rules');
const account = {version:2, accountKey, ...location.dev_player_spawn, backpack:[[obj.abyssal_whip, ONE_ITEM]], skills:skills.values};
const directory = path.join(work,'players','accounts');
fs.mkdirSync(directory,{recursive:true});
fs.writeFileSync(path.join(directory,createHash('sha256').update(accountKey).digest('hex')+'.json'),JSON.stringify(account));
const operations = [
 [RANGED_PRAYER_CYCLE, component.prayer_book.prayer_buttons, ranged.button, ACTIVATE_OPERATION],
 [WEAPON_CYCLE, component.backpack.slots, FIRST_SLOT, EQUIP_OPERATION],
 [MAGIC_PRAYER_CYCLE, component.prayer_book.prayer_buttons, magic.button, ACTIVATE_OPERATION]
];
const commands = Array.from({length:COMMAND_COUNT},()=> 'notimeout');
commands[FIRST_SPAWN_COMMAND] = `npcadd ${npc.falador_guard_archer} ${SPAWN_OFFSET_TILES} 0`;
commands[FIRST_ATTACK_COMMAND] = `opnpc ${ATTACK_OPERATION} ${npc.falador_guard_archer}`;
commands[SECOND_SPAWN_COMMAND] = `npcadd ${npc.dark_wizard_varrock} ${-SPAWN_OFFSET_TILES} 0`;
commands[SECOND_ATTACK_COMMAND] = `opnpc ${ATTACK_OPERATION} ${npc.dark_wizard_varrock}`;
fs.writeFileSync(path.join(work,'plan.json'),JSON.stringify({accountKey,commands,operations:operations.map(row=>row.join(',')).join(';'),lastCycle:LAST_CYCLE}));
