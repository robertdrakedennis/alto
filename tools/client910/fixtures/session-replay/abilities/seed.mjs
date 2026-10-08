import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {component, location, npc, obj, struct, varc} from '@alto/domain';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import SkillSet, {loadSkillDefinitions} from '../../../../../server/src/lostcity/systems/stats/SkillSet.ts';
import {loadCombatData} from '../../../../../server/src/lostcity/data/CombatData.ts';

const ACCOUNT_VERSION = 2;
const WORK_ARGUMENT = 2;
const ONE_ITEM = 1;
const PARENT_INDEX = 0;
const CHILD_INDEX = 1;
const NEXT_CYCLE = 1;
const LEFT_BUTTON_HELD = 1;
const FIRST_SLOT = 0;
const STAFF_SLOT = 1;
const BOW_SLOT = 2;
const AMMO_SLOT = 3;
const RESOURCE_COUNT = 200;
const TEST_SKILL_XP = 40_000;
const ATTACK_SKILL = 0;
const DEFENCE_SKILL = 1;
const STRENGTH_SKILL = 2;
const CONSTITUTION_SKILL = 3;
const RANGED_SKILL = 4;
const MAGIC_SKILL = 6;
const RECORD_SKILLS = [ATTACK_SKILL, DEFENCE_SKILL, STRENGTH_SKILL, CONSTITUTION_SKILL, RANGED_SKILL, MAGIC_SKILL];
const NO_CHILD = -1;
const FIRST_OPERATION = 1;
const SECOND_OPERATION = 2;
const ATTACK_OPERATION = 2;
const MAGIC_LABEL_CHILD = 11;
const OTHER_LABEL_CHILDREN = [3, 7, 15, 19];
const FIRST_KEY_CODE = 16;
const NEXT_KEY_CODE = 17;
const KEY_FIELD_BITS = 8;
const NO_MODIFIERS = 0;
const KEY_PRESS = 0;
const KEY_RELEASE = 1;
const AWT_DIGIT_ONE = 49;
const AWT_DIGIT_TWO = 50;
const AWT_ESCAPE = 27;
const LAST_CYCLE = 2600;
const COMMAND_COUNT = 24;
const NPC_OFFSET_X = 2;
const NPC_OFFSET_Z = 0;
const FIRST_ATTACK_COMMAND = 7;
const MELEE_ATTACK_COMMAND = 11;
const RANGED_ATTACK_COMMAND = 16;
const SETUP_CYCLE = 200;
const MAGIC_TAB_CYCLE = 280;
const SECOND_BAR_CYCLE = 360;
const DRAG_CYCLE = 440;
const DRAG_HOLD_CYCLES = 14;
const ESCAPE_CYCLE = 540;
const MAGIC_EQUIP_CYCLE = 620;
const AUTOCAST_CYCLE = 680;
const MAGIC_KEY_CYCLE = 980;
const FIRST_BAR_CYCLE = 1080;
const MELEE_EQUIP_CYCLE = 1130;
const MELEE_KEY_CYCLE = 1330;
const RANGED_EQUIP_CYCLE = 1510;
const AMMO_EQUIP_CYCLE = 1550;
const RANGED_KEY_CYCLE = 1780;
const REOPEN_CYCLE = 1980;
const FIRST_TAB_REVIEW_CYCLE = 2080;
const TAB_REVIEW_INTERVAL = 80;
const FINAL_ESCAPE_CYCLE = 2480;
const accountKey = 'abilities';
const work = process.argv[WORK_ARGUMENT];
const geometryPath = process.env.ALTO_RECORD_DRAG_GEOMETRY;
if (!geometryPath) throw new Error('ALTO_RECORD_DRAG_GEOMETRY must name an own-runtime native pointer observation');
const geometry = JSON.parse(fs.readFileSync(geometryPath, 'utf8'));
if (geometry.source?.[PARENT_INDEX] !== component.magic_book.actions || geometry.source?.[CHILD_INDEX] !== FIRST_OPERATION || geometry.target?.[PARENT_INDEX] !== component.action_bar_setup.slot_1 || !Array.isArray(geometry.press) || !Array.isArray(geometry.drop)) throw new Error('Native pointer observation does not name Wrack and Setup slot one');
await CacheProvider.load('data/pack');
let skills = new SkillSet(await loadSkillDefinitions(CacheProvider.js5));
for (const skill of RECORD_SKILLS) skills = skills.withXp(skill, TEST_SKILL_XP);
const rules = loadCombatData();
const strike = rules.spells.find(rule => rule.name === 'Air Strike');
const wrack = rules.abilities.find(rule => rule.definition === struct.wrack_ability);
if (!strike || !wrack || wrack.action !== FIRST_OPERATION) throw new Error('Initial basic/native Wrack contract changed');
const firstKeys = FIRST_KEY_CODE | (NEXT_KEY_CODE << KEY_FIELD_BITS);
const account = {version:ACCOUNT_VERSION,accountKey,...location.dev_player_spawn,backpack:[[obj.bronze_dagger,ONE_ITEM],[obj.plain_staff,ONE_ITEM],[obj.shortbow,ONE_ITEM],[obj.bronze_arrow,RESOURCE_COUNT],[obj.air_rune,RESOURCE_COUNT],[obj.mind_rune,RESOURCE_COUNT]],skills:skills.values,serverVarcs:[[varc.action_bar_main_keys_1_to_4,'int',String(firstKeys)],[varc.action_bar_main_key_modifiers,'int',String(NO_MODIFIERS)]]};
const directory = path.join(work,'players','accounts');
fs.mkdirSync(directory,{recursive:true});
fs.writeFileSync(path.join(directory,createHash('sha256').update(accountKey).digest('hex')+'.json'),JSON.stringify(account));
const operations = [
 [SETUP_CYCLE,component.action_bar.settings,NO_CHILD,SECOND_OPERATION],
 [MAGIC_TAB_CYCLE,component.game_window.modal_tab_labels,MAGIC_LABEL_CHILD,FIRST_OPERATION],
 [SECOND_BAR_CYCLE,component.action_bar_setup.selector,NO_CHILD,SECOND_OPERATION],
 [MAGIC_EQUIP_CYCLE,component.backpack.slots,STAFF_SLOT,SECOND_OPERATION],
 [AUTOCAST_CYCLE,component.magic_book.actions,strike.action,SECOND_OPERATION],
 [FIRST_BAR_CYCLE,component.action_bar.selector,NO_CHILD,FIRST_OPERATION],
 [MELEE_EQUIP_CYCLE,component.backpack.slots,FIRST_SLOT,SECOND_OPERATION],
 [RANGED_EQUIP_CYCLE,component.backpack.slots,BOW_SLOT,SECOND_OPERATION],
 [AMMO_EQUIP_CYCLE,component.backpack.slots,AMMO_SLOT,SECOND_OPERATION],
 [REOPEN_CYCLE,component.action_bar.settings,NO_CHILD,SECOND_OPERATION],
 ...OTHER_LABEL_CHILDREN.map((child,index)=>[FIRST_TAB_REVIEW_CYCLE+index*TAB_REVIEW_INTERVAL,component.game_window.modal_tab_labels,child,FIRST_OPERATION])
];
const keys = [[ESCAPE_CYCLE,AWT_ESCAPE,KEY_PRESS],[ESCAPE_CYCLE+NEXT_CYCLE,AWT_ESCAPE,KEY_RELEASE],[MAGIC_KEY_CYCLE,AWT_DIGIT_ONE,KEY_PRESS],[MAGIC_KEY_CYCLE+NEXT_CYCLE,AWT_DIGIT_ONE,KEY_RELEASE],[MELEE_KEY_CYCLE,AWT_DIGIT_ONE,KEY_PRESS],[MELEE_KEY_CYCLE+NEXT_CYCLE,AWT_DIGIT_ONE,KEY_RELEASE],[RANGED_KEY_CYCLE,AWT_DIGIT_TWO,KEY_PRESS],[RANGED_KEY_CYCLE+NEXT_CYCLE,AWT_DIGIT_TWO,KEY_RELEASE],[FINAL_ESCAPE_CYCLE,AWT_ESCAPE,KEY_PRESS],[FINAL_ESCAPE_CYCLE+NEXT_CYCLE,AWT_ESCAPE,KEY_RELEASE]];
const gestures = [[DRAG_CYCLE,...geometry.press,LEFT_BUTTON_HELD,KEY_PRESS],...Array.from({length:DRAG_HOLD_CYCLES},(_,index)=>[DRAG_CYCLE+index+NEXT_CYCLE,...geometry.drop,LEFT_BUTTON_HELD,NO_CHILD]),[DRAG_CYCLE+DRAG_HOLD_CYCLES+NEXT_CYCLE,...geometry.drop,NO_MODIFIERS,NO_CHILD]];
const commands = Array.from({length:COMMAND_COUNT},()=> 'notimeout');
commands[FIRST_SLOT] = `npcadd ${npc.chicken} ${NPC_OFFSET_X} ${NPC_OFFSET_Z}`;
for (const index of [FIRST_ATTACK_COMMAND,MELEE_ATTACK_COMMAND,RANGED_ATTACK_COMMAND]) commands[index] = `opnpc ${ATTACK_OPERATION} ${npc.chicken}`;
const rows = entries => entries.map(row=>row.join(',')).join(';');
fs.writeFileSync(path.join(work,'plan.json'),JSON.stringify({accountKey,commands,operations:rows(operations),keys:rows(keys),gestures:rows(gestures),lastCycle:LAST_CYCLE,geometry,stages:{setup:SETUP_CYCLE,magicTab:MAGIC_TAB_CYCLE,secondBar:SECOND_BAR_CYCLE,drag:DRAG_CYCLE,close:ESCAPE_CYCLE,magicKey:MAGIC_KEY_CYCLE,firstBar:FIRST_BAR_CYCLE,meleeKey:MELEE_KEY_CYCLE,rangedKey:RANGED_KEY_CYCLE,reopen:REOPEN_CYCLE,review:FIRST_TAB_REVIEW_CYCLE,finalClose:FINAL_ESCAPE_CYCLE}}));
