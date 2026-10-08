import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {component, location, npc, obj} from '@alto/domain';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import SkillSet, {loadSkillDefinitions} from '../../../../../server/src/lostcity/systems/stats/SkillSet.ts';
import {loadCombatData} from '../../../../../server/src/lostcity/data/CombatData.ts';

const FIRST_SLOT = 0;
const AMMO_SLOT = 1;
const STAFF_SLOT = 2;
const DAGGER_SLOT = 4;
const ONE_ITEM = 1;
const RESOURCE_COUNT = 100;
const SWORD_COUNT = 5;
const NO_CHILD = -1;
const EQUIP_OPERATION = 2;
const ACTIVATE_OPERATION = 1;
const AUTOCAST_OPERATION = 2;
const ATTACK_OPERATION = 2;
const TEST_XP = 40_000;
const PRAYER_XP = 1_000_000;
const CONSTITUTION = 3;
const RANGED = 4;
const PRAYER = 5;
const MAGIC = 6;
const LAST_CYCLE = 3300;
const FATAL_DAMAGE = 10_000;
const accountKey = 'combat';
const work = process.argv[2];
await CacheProvider.load('data/pack');
let skills = new SkillSet(await loadSkillDefinitions(CacheProvider.js5));
for (const skill of [CONSTITUTION, RANGED, MAGIC]) skills = skills.withXp(skill, TEST_XP);
skills = skills.withXp(PRAYER, PRAYER_XP);
const rules = loadCombatData();
const strike = rules.spells.find(rule => rule.name === 'Air Strike');
const protect = rules.prayers.find(rule => rule.name === 'Protect from Melee');
if (!strike || !protect) throw new Error('Missing combat recording rules');
const backpack = [[obj.shortbow, ONE_ITEM], [obj.bronze_arrow, RESOURCE_COUNT], [obj.plain_staff, ONE_ITEM], [obj.air_rune, RESOURCE_COUNT], [obj.bronze_dagger, ONE_ITEM], ...Array.from({length:SWORD_COUNT}, () => [obj.smithing_rune_sword, ONE_ITEM])];
const account = {version:2, accountKey, ...location.standard_lumbridge_teleport, backpack, skills:skills.values};
const directory = path.join(work,'players','accounts');
fs.mkdirSync(directory,{recursive:true});
fs.writeFileSync(path.join(directory,createHash('sha256').update(accountKey).digest('hex')+'.json'),JSON.stringify(account));
const operations = [
 [200,component.backpack.slots,FIRST_SLOT,EQUIP_OPERATION],
 [210,component.backpack.slots,AMMO_SLOT,EQUIP_OPERATION],
 [350,component.action_bar.piercing_shot_slot,NO_CHILD,ACTIVATE_OPERATION],
 [950,component.backpack.slots,STAFF_SLOT,EQUIP_OPERATION],
 [1000,component.magic_book.spell_buttons,strike.action,AUTOCAST_OPERATION],
 [1400,component.action_bar.wrack_slot,NO_CHILD,ACTIVATE_OPERATION],
 [1850,component.backpack.slots,DAGGER_SLOT,EQUIP_OPERATION],
 [2250,component.action_bar.slice_slot,NO_CHILD,ACTIVATE_OPERATION],
 [2400,component.prayer_book.prayer_buttons,protect.button,ACTIVATE_OPERATION]
];
const commands = Array.from({length:27},()=> 'notimeout');
commands[1] = `npcadd ${npc.chicken} 2 0`;
commands[2] = `opnpc ${ATTACK_OPERATION} ${npc.chicken}`;
commands[10] = `npcadd ${npc.chicken} -2 0`;
commands[11] = `opnpc ${ATTACK_OPERATION} ${npc.chicken}`;
commands[19] = `npcadd ${npc.goblin_level_2} 1 0`;
commands[20] = `opnpc ${ATTACK_OPERATION} ${npc.goblin_level_2}`;
commands[26] = `hit ${FATAL_DAMAGE}`;
fs.writeFileSync(path.join(work,'plan.json'),JSON.stringify({accountKey,commands,operations:operations.map(row=>row.join(',')).join(';'),lastCycle:LAST_CYCLE}));
