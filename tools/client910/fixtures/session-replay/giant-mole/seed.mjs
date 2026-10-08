import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {component, inv, location, obj} from '@alto/domain';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import SkillSet, {loadSkillDefinitions} from '../../../../../server/src/lostcity/systems/stats/SkillSet.ts';
import Player from '../../../../../server/src/lostcity/entity/Player.ts';
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import {loadCombatData} from '../../../../../server/src/lostcity/data/CombatData.ts';
import {loadBossRooms} from '../../../../../server/src/lostcity/data/BossRoomData.ts';
import {OPTION_STRIDE} from '../../../../../server/src/lostcity/systems/dialogue/ChatInterfaces.ts';
import {MOLE_COMMAND_PERIOD, MOLE_CYCLES_PER_WORLD_TICK, MOLE_FOOD_PERIOD, moleFightCommand, moleFoodSlot} from '../../../../../server/src/lostcity/network/GiantMoleInputs.testkit.ts';

const FIRST = 0;
const ONE = 1;
const TWO = 2;
const NO_CHILD = -1;
const ACCOUNT_VERSION = 2;
const JSON_INDENT = 2;
const ARGUMENT_INDEX = 2;
const MAXIMUM_SKILL_XP = 13_034_431;
const ATTACK_SKILL = 0;
const DEFENCE_SKILL = 1;
const STRENGTH_SKILL = 2;
const CONSTITUTION_SKILL = 3;
const PRAYER_SKILL = 5;
const COMBAT_SKILLS = [ATTACK_SKILL, DEFENCE_SKILL, STRENGTH_SKILL, CONSTITUTION_SKILL, PRAYER_SKILL];
const COMMAND_PERIOD = MOLE_COMMAND_PERIOD;
const CYCLES_PER_WORLD_TICK = MOLE_CYCLES_PER_WORLD_TICK;
const FIGHT_START_CYCLE = 1100;
const EQUIP_FIRST_CYCLE = 300;
const PROTECT_CYCLE = 950;
const STRENGTH_CYCLE = 1050;
const CREATE_CHOICE_CYCLE = 250;
const SETUP_START_CYCLE = 1000;
const FIGHT_MARGIN_TICKS = 30;
const LOOT_WINDOW_CYCLES = 600;
const EXIT_WALK_WINDOW_CYCLES = 600;
const EXIT_OPTION_WINDOW_CYCLES = 600;
const REJOIN_CHOICE_DELAY_CYCLES = 250;
const REJOIN_TERMINAL_DELAY_CYCLES = 1200;
const ENTRY_SHOT_CYCLE = 1100;
const FIRST_RETREAT_SHOT_CYCLE = 2200;
const COLLAPSE_SHOT_CYCLE = 5100;
const ENRAGE_SHOT_CYCLE = 7200;
const MINIONS_SHOT_CYCLE = 10100;
const work = process.argv[ARGUMENT_INDEX];
const rehearsalPath = process.env.ALTO_MOLE_REHEARSAL;

if (!rehearsalPath) throw new Error('The retained owning socket rehearsal input is required');
const rehearsal = JSON.parse(fs.readFileSync(rehearsalPath, 'utf8'));
if (rehearsal.kind !== 'Mole fixed-input rehearsal' || rehearsal.commandPeriod !== COMMAND_PERIOD || rehearsal.cyclesPerWorldTick !== CYCLES_PER_WORLD_TICK) throw new Error('A successful matched fixed-input socket rehearsal is required');
const ceilCommand = cycle => Math.ceil(cycle / COMMAND_PERIOD) * COMMAND_PERIOD;
const FIGHT_END_CYCLE = ceilCommand(FIGHT_START_CYCLE + (rehearsal.elapsedTicks + FIGHT_MARGIN_TICKS) * CYCLES_PER_WORLD_TICK);
const LEAVE_WALK_CYCLE = FIGHT_END_CYCLE + LOOT_WINDOW_CYCLES;
const LEAVE_OPTION_CYCLE = LEAVE_WALK_CYCLE + EXIT_WALK_WINDOW_CYCLES;
const REJOIN_ENTRY_CYCLE = LEAVE_OPTION_CYCLE + EXIT_OPTION_WINDOW_CYCLES;
const REJOIN_CHOICE_CYCLE = REJOIN_ENTRY_CYCLE + REJOIN_CHOICE_DELAY_CYCLES;
const LAST_CYCLE = ceilCommand(REJOIN_ENTRY_CYCLE + REJOIN_TERMINAL_DELAY_CYCLES);
const SHOTS = [ENTRY_SHOT_CYCLE, FIRST_RETREAT_SHOT_CYCLE, COLLAPSE_SHOT_CYCLE, ENRAGE_SHOT_CYCLE, MINIONS_SHOT_CYCLE, FIGHT_END_CYCLE, REJOIN_ENTRY_CYCLE, LAST_CYCLE].filter(cycle => cycle <= LAST_CYCLE);
const accountKey = 'giant-mole';
await CacheProvider.load('data/pack');
await CacheProvider.loadConfig();
const definitions = await loadSkillDefinitions(CacheProvider.js5);
let skills = new SkillSet(definitions);

for (const skill of COMBAT_SKILLS) skills = skills.withXp(skill, MAXIMUM_SKILL_XP);
const equipment = [obj.abyssal_whip, obj.wooden_shield, obj.bronze_full_helm, obj.bronze_platebody, obj.bronze_platelegs, obj.bronze_gauntlets, obj.bronze_armoured_boots];
const firstFoodSlot = equipment.length;
const foodSlots = CacheProvider.config.inv(inv.backpack).size - firstFoodSlot;
const foodDefinition = CacheProvider.config.obj(obj.shark);
const eatOption = foodDefinition.iop.findIndex(label => label?.toLowerCase() === 'eat') + ONE;
if (eatOption <= FIRST || rehearsal.food?.item !== obj.shark || rehearsal.food.firstSlot !== firstFoodSlot || rehearsal.food.slots !== foodSlots || rehearsal.food.operation !== eatOption || rehearsal.food.period !== MOLE_FOOD_PERIOD || rehearsal.food.remaining >= foodSlots) throw new Error('Matched native food consumption rehearsal is required');
const backpack = [...equipment, ...Array.from({length: foodSlots}, () => obj.shark)].map(item => [item, ONE]);
const account = {version: ACCOUNT_VERSION, accountKey, ...location.giant_mole_surface_exit, skills: skills.values, backpack};
const directory = path.join(work, 'players', 'accounts');
fs.mkdirSync(directory, {recursive: true});
fs.writeFileSync(path.join(directory, createHash('sha256').update(accountKey).digest('hex') + '.json'), JSON.stringify(account));
const room = loadBossRooms().find(found => found.id === 'giant-mole');

if (!room?.mole || !room.entryTile || !room.exitTile) throw new Error('Qualified Mole room and native placements are required');
const world = new World();
await world.loadContent();
const player = new Player();
player.accountKey = accountKey;
player.stats.restore(skills.values, undefined);
player.stats.prepare(definitions);
const session = world.bossRooms.enter(player, room.id, 'private', accountKey, {capacity: ONE});
const spawn = session.toInstance(room.spawn);
const exit = session.toInstance(room.exitTile);
const commands = Array.from({length: Math.ceil(LAST_CYCLE / COMMAND_PERIOD)}, () => 'notimeout');
const at = (cycle, command) => {commands[cycle / COMMAND_PERIOD - ONE] = command;};
at(COMMAND_PERIOD, `oploc ${ONE} ${room.entryTile.x} ${room.entryTile.z} ${room.entryLoc}`);
const controls = [];
const bones = room.mole.guaranteed.find(drop => drop.recipient === 'leader');
const definition = CacheProvider.config.npc(room.mole.normal.profile.npc);
const targets = {npc: definition.id, attackOption: definition.op.findIndex(label => label?.toLowerCase() === 'attack') + ONE, bones: bones.item};
if (targets.npc !== rehearsal.targets.npc || targets.attackOption !== rehearsal.targets.attackOption || targets.bones !== rehearsal.targets.bones) throw new Error('Rehearsal and current native targets differ');

for (let cycle = FIGHT_START_CYCLE; cycle < FIGHT_END_CYCLE; cycle += COMMAND_PERIOD) {
    const index = (cycle - FIGHT_START_CYCLE) / COMMAND_PERIOD;
    const command = moleFightCommand(index, targets);
    at(cycle, command);
    controls.push({cycle, index, command});
}
for (let cycle = FIGHT_END_CYCLE; cycle < LEAVE_WALK_CYCLE; cycle += COMMAND_PERIOD) at(cycle, `takeobj ${bones.item}`);
const lootCycle = FIGHT_END_CYCLE;
at(LEAVE_WALK_CYCLE, `walk ${exit.x + ONE} ${exit.z}`);
at(LEAVE_OPTION_CYCLE, `oploc ${ONE} ${exit.x} ${exit.z} ${room.exitLoc}`);
at(REJOIN_ENTRY_CYCLE, `oploc ${ONE} ${room.entryTile.x} ${room.entryTile.z} ${room.entryLoc}`);
const prayers = loadCombatData().prayers;
const foodOperations = [];
for (let cycle = FIGHT_START_CYCLE; cycle < FIGHT_END_CYCLE; cycle += MOLE_FOOD_PERIOD) foodOperations.push([cycle, component.backpack.slots, moleFoodSlot((cycle - FIGHT_START_CYCLE) / MOLE_FOOD_PERIOD, firstFoodSlot, foodSlots), eatOption]);
const operations = [
    [CREATE_CHOICE_CYCLE, component.dialogue_options.first_option, NO_CHILD, FIRST],
    ...equipment.map((_, slot) => [EQUIP_FIRST_CYCLE + slot * COMMAND_PERIOD, component.backpack.slots, slot, TWO]),
    [PROTECT_CYCLE, component.prayer_book.prayer_buttons, prayers.find(rule => rule.name === 'Protect from Melee').button, ONE],
    [SETUP_START_CYCLE, component.encounter_setup.start, NO_CHILD, ONE],
    [STRENGTH_CYCLE, component.prayer_book.prayer_buttons, prayers.find(rule => rule.name === 'Ultimate Strength').button, ONE],
    [REJOIN_CHOICE_CYCLE, component.dialogue_options.first_option + OPTION_STRIDE, NO_CHILD, FIRST],
    ...foodOperations
].sort((left, right) => left[FIRST] - right[FIRST]);
fs.writeFileSync(path.join(work, 'plan.json'), JSON.stringify({accountKey, commands, operations: operations.map(row => row.join(',')).join(';'), clicks: '', hover: '', keys: '', lastCycle: LAST_CYCLE, semanticShots: SHOTS, controls, spawn, exit, lootCycle, food: {item: obj.shark, firstSlot: firstFoodSlot, slots: foodSlots, operation: eatOption, period: MOLE_FOOD_PERIOD}, boss: {normal: room.mole.normal.profile.npc, enraged: room.mole.normal.enraged.npc, hitpoints: room.mole.normal.profile.hitpoints, death: room.mole.normal.profile.anims.death, bones: bones.item, chamberCount: room.mole.chambers.length, homes: [room.spawn, ...room.mole.chambers.map(chamber => chamber.tile)].map(tile => session.toInstance(tile))}, rehearsalSha256: createHash('sha256').update(fs.readFileSync(rehearsalPath)).digest('hex'), qualification: 'Initial max-level skills and fixed RNG are fixtures. Entry/setup/equipment/prayers/food use native UI; approachnpc/opnpc/oploc/takeobj selection commands delegate ordinary owners. No fixture damage, healing, teleport or loot injection during the fight. Socket proof separately covers actual native MOVE_GAMECLICK and OPNPC. Replay acceptance must prove each life/map/loot event; this recipe alone is not proof.'}, null, JSON_INDENT));
