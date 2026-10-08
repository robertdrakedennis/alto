// Initial fixture only; complete before World startup and ordinary login.
import fs from 'node:fs';
import path from 'node:path';
import {createHash, randomUUID} from 'node:crypto';
import {pathToFileURL} from 'node:url';

const SPECIAL_FIRST_ARGUMENT = 2;
const SPECIAL_FIRST = 0;
const SPECIAL_ONE = 1;
const SPECIAL_EMPTY = 0;
const SPECIAL_ACCOUNT_VERSION = 2;
const SPECIAL_INDENT = 2;
const SPECIAL_WEAPON_TIER = 92;
const SPECIAL_ARMOUR_TIER = 90;
const SPECIAL_DAMAGED_FRACTION = 2;
const SPECIAL_WALLET_MULTIPLIER = 2;
const SPECIAL_INTEGER_HIGH_BIT = 31;
const SPECIAL_INTEGER_MASK = 0xffffffff;
const SPECIAL_PLAYER_VAR_DOMAIN = 0;
const SPECIAL_ACCOUNT = 'alice';
const SPECIAL_HOOD_NAME = "Ahrim's hood";
const SPECIAL_BOB_NAME = 'Bob';
const SPECIAL_TARGET_NAME = 'Rat';
const SPECIAL_MAXIMUM_TARGET_LEVEL = 4;
const SPECIAL_PRAYER_WINDOW_TITLE = 'Prayer Abilities';
const specialRoot = process.env.ALTO_SPECIAL_RUNTIME_ROOT;
const specialWork = process.argv[SPECIAL_FIRST_ARGUMENT];
if (!specialRoot || !path.isAbsolute(specialRoot) || !specialWork || !path.isAbsolute(specialWork)) throw new Error('Explicit own checkout and fresh pre-login scratch required');
const specialOwner = relative => import(pathToFileURL(path.join(specialRoot, relative)).href);
const [{default: CacheProvider}, {endgameFixtureLoadout}, {loadBarrows}, {loadItemChargeData}, {loadNpcData}, {loadGwd}, {loadCombatData}, {loadSpecialCombatData}, {foodDefinition}, {obj, location, enums, struct, component, varp}] = await Promise.all([
    specialOwner('server/src/lostcity/server/CacheProvider.ts'),
    specialOwner('server/src/lostcity/systems/equipment/EndgameFixture.testkit.ts'),
    specialOwner('server/src/lostcity/data/BarrowsData.ts'),
    specialOwner('server/src/lostcity/data/ItemChargeData.ts'),
    specialOwner('server/src/lostcity/data/NpcData.ts'),
    specialOwner('server/src/lostcity/data/GwdData.ts'),
    specialOwner('server/src/lostcity/data/CombatData.ts'),
    specialOwner('server/src/lostcity/data/SpecialCombatData.ts'),
    specialOwner('server/src/lostcity/data/FoodData.ts'),
    specialOwner('packages/domain/src/index.ts'),
]);
await CacheProvider.load(path.join(specialRoot, 'server/data/pack'));
await CacheProvider.loadConfig();
const specialConfig = CacheProvider.config;
const specialChoice = {style: 'melee', tier: SPECIAL_WEAPON_TIER, armourTier: SPECIAL_ARMOUR_TIER, armour: 'tank', hands: 'two-handed'};
const specialKit = await endgameFixtureLoadout(specialChoice);
if (!specialKit.loadout.equipment.includes(obj.zaros_godsword)) throw new Error('Qualified ordinary Zaros godsword kit is absent');
const specialFamilies = loadItemChargeData().filter(row => row.equipment?.name === SPECIAL_HOOD_NAME && row.freshCharges !== null);
if (specialFamilies.length !== SPECIAL_ONE) throw new Error('Fresh physical repair family is ambiguous');
const specialFamily = specialFamilies[SPECIAL_FIRST];
const specialUsed = loadItemChargeData().find(row => row.item === specialFamily.usedItem);
if (!specialUsed || !specialFamily.coinRepair || specialUsed.coinRepair?.output !== specialFamily.item || specialUsed.capacity !== specialFamily.capacity) throw new Error('Used physical identity/native repair output is unqualified');
const specialKey = randomUUID();
const specialCharges = Math.floor(specialFamily.capacity / SPECIAL_DAMAGED_FRACTION);
const specialDamaged = [specialFamily.usedItem, SPECIAL_ONE, {key: specialKey, charges: specialCharges}];
const specialBackpack = specialKit.backpack.map(row => [...row]);
specialBackpack[specialKit.equipment.length] = specialDamaged;
const specialFood = foodDefinition(specialConfig, obj.shark);
if (!specialFood) throw new Error('Ordinary generated Shark is absent');
for (let index = specialKit.equipment.length + SPECIAL_ONE; index < specialBackpack.length; index += SPECIAL_ONE) specialBackpack[index] = [obj.shark, SPECIAL_ONE];
const specialNpcData = loadNpcData();
const specialBobRules = loadBarrows().repairers.filter(row => specialConfig.npc(row.npc).name === SPECIAL_BOB_NAME && row.allOperation !== null);
if (specialBobRules.length !== SPECIAL_ONE) throw new Error('Native Bob Repair-all selector is ambiguous');
const specialBob = specialBobRules[SPECIAL_FIRST];
const specialBobSpawns = specialNpcData.spawns.filter(row => row.npc === specialBob.npc);
if (specialBobSpawns.length !== SPECIAL_ONE || specialConfig.npc(specialBob.npc).op?.[specialBob.allOperation - SPECIAL_ONE]?.toLowerCase() !== 'repair-all') throw new Error('Ordinary Bob placement or native operation is absent');
const specialBobSpawn = specialBobSpawns[SPECIAL_FIRST];
const specialTargets = specialNpcData.spawns.filter(row => {
    const profile = specialNpcData.profiles.get(row.npc);
    return profile?.name === SPECIAL_TARGET_NAME && profile.level > SPECIAL_EMPTY && profile.level <= SPECIAL_MAXIMUM_TARGET_LEVEL && profile.hitpoints > SPECIAL_EMPTY && row.level === specialBobSpawn.level && specialConfig.npc(row.npc).op?.includes('Attack');
}).sort((left, right) => {
    const distance = row => Math.max(Math.abs(row.x - specialBobSpawn.x), Math.abs(row.z - specialBobSpawn.z));
    return distance(left) - distance(right) || left.x - right.x || left.z - right.z;
});
if (!specialTargets.length) throw new Error('No ordinary nearby full-health Rat candidate exists');
const specialTarget = specialTargets[SPECIAL_FIRST];
const specialGwd = loadGwd();
const specialBackings = new Map();
for (const row of specialGwd.counts) {
    const bit = specialConfig.varbit(row.varbit);
    if (bit.domain?.id !== SPECIAL_PLAYER_VAR_DOMAIN || bit.basevarId !== row.backing || bit.startbit !== row.start || bit.endbit !== row.end) throw new Error('Native saved kill-count bit contract changed');
    const mask = SPECIAL_INTEGER_MASK >>> (SPECIAL_INTEGER_HIGH_BIT - (bit.endbit - bit.startbit));
    const value = specialGwd.entry.lobbyMinimum.value;
    if (value > mask || value <= SPECIAL_EMPTY) throw new Error('Generated stale-count fixture does not fit its native field');
    const prior = specialBackings.get(row.backing) ?? SPECIAL_EMPTY;
    specialBackings.set(row.backing, (prior & ~(mask << bit.startbit)) | ((value & mask) << bit.startbit));
}
const specialWindows = specialConfig.enum(enums.native_interface_windows);
const specialWindowRows = new Map(specialWindows.valuesArray.map((definition, key) => [key, definition]));
for (const [key, definition] of specialWindows.valuesMap) specialWindowRows.set(key, definition);
const specialDestination = predicate => {
    const matches = [...specialWindowRows].filter(([, definition]) => predicate(definition));
    if (matches.length !== SPECIAL_ONE) throw new Error('Native window destination is ambiguous');
    return matches[SPECIAL_FIRST][SPECIAL_FIRST];
};
const specialCombat = loadCombatData();
const specialPrayers = specialKit.prayers.map(action => specialCombat.prayers.find(row => row.action === action));
if (specialPrayers.some(row => !row)) throw new Error('Ordinary offensive Prayer rows are absent');
const specialRules = loadSpecialCombatData();
const specialRule = specialRules.weapons.find(row => row.weapon === obj.zaros_godsword);
const specialField = specialRule?.effects.find(row => row.kind === 'special-field');
if (!specialRule || specialRule.unavailable || specialRule.activation !== 'self' || !specialField || !specialField.boost) throw new Error('Admitted source-centered Blackhole mechanic is absent');
const specialCoins = specialFamily.coinRepair.fullCost * SPECIAL_WALLET_MULTIPLIER;
const specialAccount = {version: SPECIAL_ACCOUNT_VERSION, accountKey: SPECIAL_ACCOUNT, members: true,
    ...location.lumbridge_lodestone, coins: specialCoins, skills: specialKit.skills,
    backpack: specialBackpack, worn: specialKit.worn,
    savedVarps: [...specialBackings, [varp.auto_retaliate_disabled, SPECIAL_ONE]]};
const specialPlan = {format: SPECIAL_ONE, accountKey: SPECIAL_ACCOUNT, equipment: specialKit.loadout.equipment,
    equipmentFacts: specialKit.equipment, loadout: specialKit.loadout, food: {item: obj.shark, maximumHealing: specialFood.maximumHealing}, prayers: specialPrayers,
    toolbar: {backpackDestination: specialDestination(value => value === struct.backpack_window),
        combatDestination: specialDestination(value => value === struct.melee_ability_window),
        prayerDestination: specialDestination(value => {const values = [...specialConfig.struct(value).params.values()]; return values.includes(SPECIAL_PRAYER_WINDOW_TITLE) && values.includes(component.game_window.prayer_slot);})},
    repair: {key: specialKey, family: specialFamily, initialCharges: specialCharges, bob: {...specialBob, spawn: specialBobSpawn, operation: 'Repair-all'}},
    gwd: {counts: specialGwd.counts, initialSavedVarps: [...specialBackings], initialCount: specialGwd.entry.lobbyMinimum.value},
    target: {npc: specialTarget.npc, spawn: specialTarget, profile: specialNpcData.profiles.get(specialTarget.npc)},
    special: {rule: specialRule, field: specialField, activationOrder: 'self-before-Attack',
        scope: 'Cost, boost lifetime and ordinary queued/landed damage; no captured primary and therefore no scheduled periodic target pulses in this sample'},
    source: {gearChoice: specialChoice, blockedResources: specialKit.blockedResources,
        qualification: ['Placements preserve existing generated provenance; no dated geometry upgrade', 'Special presentation/weapon-damage projection remain explicitly unverified', 'No post-login count/position/skill/account/resource/RNG assistance']}};
const specialSha = bytes => createHash('sha256').update(bytes).digest('hex');
const specialAccountPath = path.join(specialWork, 'players/accounts', specialSha(Buffer.from(SPECIAL_ACCOUNT)) + '.json');
fs.mkdirSync(path.dirname(specialAccountPath), {recursive: true});
const specialWrite = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, SPECIAL_INDENT) + '\n', {flag: 'wx'});
specialWrite(specialAccountPath, specialAccount);
specialWrite(path.join(specialWork, 'plan.json'), specialPlan);
specialWrite(path.join(specialWork, 'initial-fixture.json'), {accountPath: specialAccountPath, accountSha256: specialSha(fs.readFileSync(specialAccountPath)), account: specialAccount, plan: specialPlan,
    inputs: Object.fromEntries(['instances/barrows.json', 'instances/gwd.json', 'combat/charges.json', 'combat/specials.json', 'combat/rules.json', 'equipment/endgame-loadouts.json', 'npcs/profiles.json', 'spawns/spawns.json'].map(relative => [relative, specialSha(fs.readFileSync(path.join(specialRoot, 'server/data/generated', relative)))]))});
