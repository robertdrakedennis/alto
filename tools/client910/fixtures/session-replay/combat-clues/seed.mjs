import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {component, enums, npc, obj, param, varbit} from '@alto/domain';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import SkillSet, {loadSkillDefinitions} from '../../../../../server/src/lostcity/systems/stats/SkillSet.ts';
import CollisionManager from '../../../../../server/src/lostcity/engine/collision/CollisionManager.ts';
import {standable} from '../../../../../server/src/lostcity/systems/movement/Standing.ts';
import {locReached} from '../../../../../server/src/lostcity/engine/interaction/Reach.ts';
import {loadClueData} from '../../../../../server/src/lostcity/data/ClueData.ts';
import {loadCombatData} from '../../../../../server/src/lostcity/data/CombatData.ts';
import {TOOLS} from '../../../../../server/src/lostcity/content/trails/data.ts';
const ONE_ITEM = 1;
const ZERO = 0;
const NO_CHILD = -1;
const AREA_DIVISOR = 2;
const SEARCH_RADIUS = 3;
const FIRST_OPERATION = 1;
const EQUIP_OPERATION = 2;
const DIG_OPERATION = 2;
const PAUSE_OPERATION = 0;
const MAIN_HAND = 3;
const COMBAT_XP = 13_034_431;
const ATTACK_SKILL = 0;
const DEFENCE_SKILL = 1;
const STRENGTH_SKILL = 2;
const CONSTITUTION_SKILL = 3;
const PRAYER_SKILL = 5;
const WHIP_FIRST_SLOT = 4;
const MASTER_SLOT = 5;
const FIRST_HARD_SLOT = 6;
const SECOND_HARD_SLOT = 6;
const WHIP_LATER_SLOT = 6;
const EMOTE_HARD_SLOT = 7;
const MEDIUM_SLOT = 7;
const COMMAND_FIRST_CYCLE = 101;
const COMMAND_INTERVAL = 100;
const COMMAND_COUNT = 55;
const FINAL_CYCLE = 5800;
const TIMING = {
    saraTrail: 201, saraRead: 300, prayer: 350, saraDig: 400, firstWield: 650, saraAttack: 801, saraRedig: 1000,
    zamWarp: 1101, zamTrail: 1401, zamDig: 1550, zamAttack: 1801, zamRedig: 2000,
    hardUndress: 2150, hardWarp: 2201, hardTrail: 2501, hardRead: 2600, panic: 2700, hardWield: 2850, hardAttack: 3001, hardRemove: 3150, hardTalk: 3201, hardContinue: 3300,
    masterWarp: 3401, masterRead: 3700, salute: 3800, masterWield: 3950, masterAttack: 4101, masterRemove: 4250, masterTalk: 4301, masterContinue: 4400,
    mediumWarp: 4501, mediumTrail: 4801, lockedSearch: 5001, chickenSpawn: 5101, chickenWield: 5150, chickenAttack: 5201, keySearch: 5401
};
const accountKey = 'combat-clues';
const work = process.argv[2];
await CacheProvider.load('data/pack');
await CacheProvider.loadConfig();
await new CollisionManager().init(CacheProvider);
const config = CacheProvider.config;
const data = loadClueData();
const step = id => data.steps.find(row => row.obj === id);
const sara = step(obj.hard_clue_saradomin_coordinates);
const zam = step(obj.hard_clue_zamorak_coordinates);
const hard = step(obj.hard_clue_haunted_woods_panic);
const medium = step(obj.medium_clue_chicken_key);
const master = data.steps.find(row => row.variant === 'Clue scroll (master) - Salute in the Max Guild Garden');
function areaStand(row) {
    const area = row.emote.area;
    const center = {x: Math.floor((area.minX + area.maxX) / AREA_DIVISOR), z: Math.floor((area.minZ + area.maxZ) / AREA_DIVISOR)};
    const found = [];
    for (let x = area.minX; x <= area.maxX; x++) for (let z = area.minZ; z <= area.maxZ; z++) {
        if ([[x,z],[x-ONE_ITEM,z],[x+ONE_ITEM,z],[x,z-ONE_ITEM],[x,z+ONE_ITEM]].every(([x,z]) => standable(x,z,area.level))) found.push({level:area.level,x,z});
    }
    found.sort((a,b) => Math.abs(a.x-center.x)+Math.abs(a.z-center.z)-Math.abs(b.x-center.x)-Math.abs(b.z-center.z));
    if (!found.length) throw new Error(`No legal combat stand for ${row.page}`);
    return found[ZERO];
}
const hardAt = areaStand(hard);
const masterAt = areaStand(master);
const search = medium.search;
const placed = [];
const {forEachPlacement} = await import('../../../../../server/src/lostcity/tools/data/domains/world/MapPlacements.ts');
await forEachPlacement(CacheProvider, loc => {if(loc.id === search.loc && loc.x === search.origin.x && loc.z === search.origin.z && loc.level === search.origin.level) placed.push(loc);});
const type = config.loc(search.loc);
const footprint = {...placed[ZERO],width:type.width,length:type.length,forceapproach:type.forceapproach};
let mediumAt;
for(let dx=-SEARCH_RADIUS;dx<=SEARCH_RADIUS&&!mediumAt;dx++)for(let dz=-SEARCH_RADIUS;dz<=SEARCH_RADIUS&&!mediumAt;dz++) {
 const tile={x:search.origin.x+dx,z:search.origin.z+dz,level:search.origin.level};
 if(standable(tile.x,tile.z,tile.level)&&standable(tile.x+ONE_ITEM,tile.z,tile.level)&&locReached(tile,footprint)) mediumAt=tile;
}
if(!mediumAt) throw new Error('No legal search/chicken stand');
let skills = new SkillSet(await loadSkillDefinitions(CacheProvider.js5));
for(const skill of [ATTACK_SKILL,DEFENCE_SKILL,STRENGTH_SKILL,CONSTITUTION_SKILL,PRAYER_SKILL]) skills=skills.withXp(skill,COMBAT_XP);
const masterSteps = config.varbit(varbit.master_trail_steps_left);
const account = {version:2,accountKey,...sara.tile,backpack:[TOOLS.spade,...TOOLS.instruments,obj.abyssal_whip,obj.master_emote_clue].map(id=>[id,ONE_ITEM]),skills:skills.values,savedVarps:[[masterSteps.basevarId,ONE_ITEM<<masterSteps.startbit]],trails:{masterHolding:obj.master_emote_clue,selections:[{tier:'master',obj:obj.master_emote_clue,variant:master.variant}]}};
const directory=path.join(work,'players','accounts');fs.mkdirSync(directory,{recursive:true});fs.writeFileSync(path.join(directory,createHash('sha256').update(accountKey).digest('hex')+'.json'),JSON.stringify(account));
const commands=Array.from({length:COMMAND_COUNT},()=> 'notimeout');
function command(cycle,text){const index=(cycle-COMMAND_FIRST_CYCLE)/COMMAND_INTERVAL;if(!Number.isInteger(index))throw new Error(`Bad command cycle ${cycle}`);commands[index]=text;}
const warp=tile=>`warp ${tile.x} ${tile.z} ${tile.level}`;
command(TIMING.saraTrail,`trail hard ${sara.obj} ${ONE_ITEM}`);command(TIMING.saraAttack,`opnpc ${EQUIP_OPERATION} ${npc.clue_saradomin_wizard}`);
command(TIMING.zamWarp,warp(zam.tile));command(TIMING.zamTrail,`trail hard ${zam.obj} ${ONE_ITEM}`);command(TIMING.zamAttack,`opnpc ${EQUIP_OPERATION} ${npc.clue_zamorak_wizard}`);
command(TIMING.hardWarp,warp(hardAt));command(TIMING.hardTrail,`trail hard ${hard.obj} ${ONE_ITEM}`);command(TIMING.hardAttack,`opnpc ${EQUIP_OPERATION} ${npc.clue_double_agent_hard}`);command(TIMING.hardTalk,`opnpc ${FIRST_OPERATION} ${npc.uri}`);
command(TIMING.masterWarp,warp(masterAt));command(TIMING.masterAttack,`opnpc ${EQUIP_OPERATION} ${npc.clue_double_agent_master}`);command(TIMING.masterTalk,`opnpc ${FIRST_OPERATION} ${npc.uri}`);
command(TIMING.mediumWarp,warp(mediumAt));command(TIMING.mediumTrail,`trail medium ${medium.obj} ${ONE_ITEM}`);command(TIMING.lockedSearch,`oploc ${FIRST_OPERATION} ${search.origin.x} ${search.origin.z} ${search.loc}`);command(TIMING.chickenSpawn,`npcadd ${medium.key.npcs[ZERO]} ${ONE_ITEM} ${ZERO}`);command(TIMING.chickenAttack,`opnpc ${EQUIP_OPERATION} ${medium.key.npcs[ZERO]}`);command(TIMING.keySearch,`oploc ${FIRST_OPERATION} ${search.origin.x} ${search.origin.z} ${search.loc}`);
const list=config.enum(enums.emote_list);
function emote(name){for(let index=ZERO;index<list.getOutputCount();index++)if(config.struct(list.getValueInt(index)).params.get(param.emote_name)===name)return index;throw new Error(`Missing ${name}`);}
const protect=loadCombatData().prayers.find(rule=>rule.name==='Protect from Magic');
const operations=[
[TIMING.saraRead,component.backpack.slots,FIRST_HARD_SLOT,FIRST_OPERATION],[TIMING.prayer,component.prayer_book.prayer_buttons,protect.button,FIRST_OPERATION],[TIMING.saraDig,component.backpack.slots,FIRST_HARD_SLOT,DIG_OPERATION],[TIMING.firstWield,component.backpack.slots,WHIP_FIRST_SLOT,EQUIP_OPERATION],[TIMING.saraRedig,component.backpack.slots,FIRST_HARD_SLOT,DIG_OPERATION],
[TIMING.zamDig,component.backpack.slots,SECOND_HARD_SLOT,DIG_OPERATION],[TIMING.zamRedig,component.backpack.slots,SECOND_HARD_SLOT,DIG_OPERATION],
[TIMING.hardUndress,component.worn_equipment.slots,MAIN_HAND,FIRST_OPERATION],[TIMING.hardRead,component.backpack.slots,EMOTE_HARD_SLOT,FIRST_OPERATION],[TIMING.panic,component.emotes.list,emote('Panic'),FIRST_OPERATION],[TIMING.hardWield,component.backpack.slots,WHIP_LATER_SLOT,EQUIP_OPERATION],[TIMING.hardRemove,component.worn_equipment.slots,MAIN_HAND,FIRST_OPERATION],[TIMING.hardContinue,component.npc_chat.continue_button,NO_CHILD,PAUSE_OPERATION],
[TIMING.masterRead,component.backpack.slots,MASTER_SLOT,FIRST_OPERATION],[TIMING.salute,component.emotes.list,emote('Salute'),FIRST_OPERATION],[TIMING.masterWield,component.backpack.slots,WHIP_LATER_SLOT,EQUIP_OPERATION],[TIMING.masterRemove,component.worn_equipment.slots,MAIN_HAND,FIRST_OPERATION],[TIMING.masterContinue,component.npc_chat.continue_button,NO_CHILD,PAUSE_OPERATION],
[TIMING.chickenWield,component.backpack.slots,WHIP_LATER_SLOT,EQUIP_OPERATION]
];
const EQUIPMENT_TAB = {x: 885, y: 378};
const BACKPACK_TAB = {x: 842, y: 378};
const EMOTES_TAB = {x: 331, y: 580};
const CLOSE_SCROLL = {x: 758, y: 229};
const TAB_LEAD_CYCLES = 40;
const SCROLL_CLOSE_DELAY = 50;
const EMOTE_TAB_AFTER_CLOSE_CYCLES = 20;
const clicks = [];
const click = (cycle, point) => clicks.push(`l,${point.x},${point.y},${cycle}`);
click(TIMING.saraRead + SCROLL_CLOSE_DELAY, CLOSE_SCROLL);
for (const cycle of [TIMING.hardUndress, TIMING.hardRemove, TIMING.masterRemove]) {
    click(cycle - TAB_LEAD_CYCLES, EQUIPMENT_TAB);
    click(cycle + TAB_LEAD_CYCLES, BACKPACK_TAB);
}
for (const cycle of [TIMING.hardWield, TIMING.masterWield, TIMING.chickenWield]) click(cycle - TAB_LEAD_CYCLES, BACKPACK_TAB);
for (const cycle of [TIMING.hardRead, TIMING.masterRead]) {
    click(cycle + SCROLL_CLOSE_DELAY, CLOSE_SCROLL);
    click(cycle + SCROLL_CLOSE_DELAY + EMOTE_TAB_AFTER_CLOSE_CYCLES, EMOTES_TAB);
}
fs.writeFileSync(path.join(work,'plan.json'),JSON.stringify({accountKey,commands,operations:operations.map(row=>row.join(',')).join(';'),clicks:clicks.join(';'),lastCycle:FINAL_CYCLE,stands:{hardAt,masterAt,mediumAt}}));
console.log('Replay plan',JSON.stringify({timing:TIMING,stands:{hardAt,masterAt,mediumAt}}));
