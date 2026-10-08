// Declared initial account only; complete before the ordinary servers and login start.
import fs from "node:fs";
import path from "node:path";
import {createHash} from "node:crypto";
import {pathToFileURL} from "node:url";

const FIRST_ARGUMENT = 2;
const FACTION_ARGUMENT = 3;
const SINGLE_ITEM = 1;
const EMPTY_ITEM = -1;
const EMPTY_QUANTITY = 0;
const FIRST_MATCH = 0;
const ACCOUNT_VERSION = 2;
const PLAN_FORMAT = 1;
const INDENT = 2;
const INITIAL_LEVEL = 99;
const ACCOUNT = "alice";
const BANDOS = "bandos";
const AMMUNITION_COUNT = 1000;
const FOOD_LATENCY_BUFFER_PARTS = 2;
const RUN_FINE_PER_PERCENT = 100;
const INTEGER_HIGH_BIT = 31;
const INTEGER_MASK = 0xffffffff;
const PLAYER_VAR_DOMAIN = 0;
const root = process.env.ALTO_GWD_RUNTIME_ROOT;
const work = process.argv[FIRST_ARGUMENT];
if (!root || !path.isAbsolute(root) || !work || !path.isAbsolute(work))
  throw new Error("Explicit owned runtime and fresh scratch required");
const owner = relative => import(pathToFileURL(path.join(root, relative)).href);
const [{default: CacheProvider}, {default: SkillSet, loadSkillDefinitions}, {statNamed},
  {loadGwd}, {loadCombatData}, {qualifiedEquipment}, {loadItemChargeData}, {allowChargedEquipment}, {foodDefinition}, {loadRunEnergyData}, {FULL_ENERGY}, {obj, inv, location, enums, struct, component, param, varp}] = await Promise.all([
  owner("server/src/lostcity/server/CacheProvider.ts"),
  owner("server/src/lostcity/systems/stats/SkillSet.ts"),
  owner("server/src/lostcity/systems/stats/StatNames.ts"),
  owner("server/src/lostcity/data/GwdData.ts"),
  owner("server/src/lostcity/data/CombatData.ts"),
  owner("server/src/lostcity/systems/equipment/EquipmentContent.ts"),
  owner("server/src/lostcity/data/ItemChargeData.ts"),
  owner("server/src/lostcity/systems/equipment/ChargedEquipment.ts"),
  owner("server/src/lostcity/data/FoodData.ts"),
  owner("server/src/lostcity/data/RunEnergyData.ts"),
  owner("server/src/lostcity/systems/movement/RunState.ts"),
  owner("packages/domain/src/index.ts"),
]);
await CacheProvider.load(path.join(root, "server/data/pack"));
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const rules = loadGwd();
const combat = loadCombatData();
const chargeFamilies = loadItemChargeData();
allowChargedEquipment(config, chargeFamilies);
const faction = process.argv[FACTION_ARGUMENT] ?? BANDOS;
const kits = {
  bandos: {godWeapon: obj.bandos_godsword, initialLocation: location.gwd_bandos_return_landing,
    prayers: ["Protect from Melee", "Ultimate Strength", "Incredible Reflexes"]},
  armadyl: {godWeapon: obj.armadyl_godsword, initialLocation: location.gwd_armadyl_return_landing,
    combatItems: [obj.magic_shortbow, obj.rune_arrow_ammunition], arenaItems: [obj.morrigan_javelin], rejoinItems: [obj.morrigan_javelin],
    prayers: ["Protect from Missiles"]},
  saradomin: {godWeapon: obj.saradomin_godsword, initialLocation: location.gwd_saradomin_first_return_landing,
    combatItems: [obj.magic_shortbow, obj.rune_arrow_ammunition], arenaItems: [obj.morrigan_javelin], rejoinItems: [obj.saradomin_godsword],
    prayers: ["Protect from Magic"]},
  zamorak: {godWeapon: obj.zamorak_godsword, initialLocation: location.gwd_zamorak_enter_native,
    restoreBeforeArena: true, prayers: ["Protect from Melee"]},
};
const kit = kits[faction];
const arena = rules.arenas.find(row => row.faction === faction);
const initialCount = rules.entry.lobbyMinimum.value - SINGLE_ITEM;
if (!kit || !arena || initialCount < SINGLE_ITEM) throw new Error("Generated normal faction admission is absent");
const rangedKit = faction === "armadyl" || faction === "saradomin";
const armourNames = faction === BANDOS ? [] : rangedKit
  ? ["Karil's coif", "Karil's top", "Karil's skirt"]
  : ["Torag's helm", "Torag's platebody", "Torag's platelegs"];
const physicalEquipment = armourNames.map(name => {
  const matches = chargeFamilies.filter(row => row.equipment?.name === name &&
    row.equipment.active && row.freshCharges === row.capacity && row.usedItem !== null);
  if (matches.length !== SINGLE_ITEM) throw new Error(`Fresh generated armour family absent or ambiguous: ${name}`);
  const row = matches[FIRST_MATCH], native = config.obj(row.item), used = config.obj(row.usedItem);
  if (native.name !== name || !native.iop.includes("Wear") || !used.iop.includes("Wear") ||
      native.certtemplate !== EMPTY_ITEM || native.derivedFrom !== null ||
      used.certtemplate !== EMPTY_ITEM || used.derivedFrom !== null)
    throw new Error(`Generated physical armour/cache contract changed: ${name}`);
  return {item: row.item, usedItem: row.usedItem, slot: row.equipment.slot,
    capacity: row.capacity, freshCharges: row.freshCharges, wearPerTick: row.wearPerTick,
    equipment: row.equipment, qualification: row.qualification};
});
const armour = physicalEquipment.length ? physicalEquipment.map(row => row.item)
  : [obj.bronze_full_helm, obj.bronze_platebody, obj.bronze_platelegs];
const auxiliaryArmour = rangedKit ? [] : [obj.bronze_gauntlets, obj.bronze_armoured_boots];
const godAccessory = faction === "armadyl" ? obj.armadyl_stole : null;
const equipment = [...armour, ...auxiliaryArmour, ...(godAccessory === null ? [] : [godAccessory]), kit.godWeapon];
const combatEquipment = kit.combatItems ?? [kit.godWeapon];
const arenaEquipment = kit.arenaItems ?? combatEquipment;
const rejoinEquipment = kit.rejoinItems ?? combatEquipment;
const thrownAmmunition = rangedKit ? {item: obj.morrigan_javelin, count: AMMUNITION_COUNT} : null;
const equipmentFacts = [...new Set([...equipment, ...combatEquipment, ...arenaEquipment, ...rejoinEquipment])].map(item => {
  const native = config.obj(item);
  const qualified = qualifiedEquipment(native);
  return {item, name: native.name, qualified, inventoryOperations: native.iop};
});
if (!rules.godItems.some(row => row.item === kit.godWeapon && row.factions.includes(faction) && row.altarFactions.includes(faction)))
  throw new Error("The actually wieldable god weapon lacks the generated faction/altar association");
let godAccessoryFacts = null;
if (godAccessory !== null) {
  const native = config.obj(godAccessory), qualified = qualifiedEquipment(native);
  const association = rules.godItems.find(row => row.item === godAccessory && row.factions.includes(faction));
  if (!association || qualified.kind !== "jewellery" || qualified.requirement?.skill !== "prayer" ||
      native.certtemplate !== EMPTY_ITEM || native.derivedFrom !== null || !native.iop.includes("Wear") ||
      native.params.get(param.equip_requirement_stat) !== statNamed(config, "Prayer") ||
      native.params.get(param.equip_requirement_level) !== qualified.requirement.level ||
      Number(native.params.get(param.equipment_quest_requirement) ?? EMPTY_ITEM) >= FIRST_MATCH ||
      Number(native.params.get(param.secondary_equip_stat) ?? EMPTY_ITEM) >= FIRST_MATCH)
    throw new Error("Declared source-qualified god accessory/cache admission changed");
  godAccessoryFacts = {item: godAccessory, slot: qualified.slot, requirement: qualified.requirement,
    factions: association.factions, nativeOperation: "Wear",
    datedSource: {title: "Armadyl stole", oldid: 30600784, timestamp: "2019-11-28T10:33:25Z",
      contentSha256: "2e5083549fad7b3daf843d4801bfeb5094011239faf4d8b88740343c62fb2590"},
    scope: "Ordinary base-Prayer-gated Wear and new Armadyl camp acquisition tolerance only; no target clearing or Prayer-drain bonus"};
}
const combatStyle = rangedKit ? "ranged" : "melee";
if (combatStyle === "ranged") {
  const weapon = config.obj(obj.magic_shortbow), ammunition = config.obj(obj.rune_arrow_ammunition);
  if (weapon.params.get(param.ammunition_category) !== ammunition.category ||
      !combat.ranged.some(row => row.item === weapon.id)) throw new Error("Native ranged/ammunition contract changed");
}
let thrownFacts = null;
if (thrownAmmunition) {
  const weapon = config.obj(thrownAmmunition.item);
  const range = combat.ranged.find(row => row.item === weapon.id);
  const category = Number(weapon.params.get(param.weapon_category_ref));
  const sequence = Number(config.struct(category).params.get(param.weapon_attack_sequence));
  const projectile = Number(weapon.params.get(param.projectile_effect));
  const quest = Number(weapon.params.get(param.equipment_quest_requirement) ?? EMPTY_ITEM);
  const secondaryStat = Number(weapon.params.get(param.secondary_equip_stat) ?? EMPTY_ITEM);
  const secondaryLevel = Number(weapon.params.get(param.secondary_equip_level) ?? EMPTY_QUANTITY);
  if (weapon.stackable !== SINGLE_ITEM || !weapon.iop.includes("Wield") || !range ||
      !Number.isInteger(sequence) || sequence < FIRST_MATCH || !Number.isInteger(projectile) || projectile < FIRST_MATCH ||
      quest !== EMPTY_ITEM || chargeFamilies.some(row => row.item === weapon.id))
    throw new Error("Declared ordinary nondegrading thrown weapon contract changed");
  thrownFacts = {item: weapon.id, name: weapon.name, range, category, sequence, projectile, quest, secondaryStat, secondaryLevel,
    accuracy: weapon.params.get(param.ranged_accuracy), damage: weapon.params.get(param.ranged_damage), speed: weapon.params.get(param.attack_speed),
    qualification: "Retained dated Morrigan javelin oldid30676611 explicitly establishes ordinary base stackable nondegradation; generated range and native Wield/projectile/class sequence have existing normal owners; no special activation"};
}
const definitions = await loadSkillDefinitions(CacheProvider.js5);
let skills = new SkillSet(definitions);
const skillFacts = [];
for (const name of ["Attack", "Strength", "Defence", "Constitution", "Prayer", "Ranged", "Agility"]) {
  const stat = statNamed(config, name);
  const definition = definitions[stat];
  const index = INITIAL_LEVEL - definition.base - SINGLE_ITEM;
  const xp = definition.table[index];
  if (index < FIRST_MATCH || !Number.isInteger(xp)) throw new Error(`Native ${name} threshold absent`);
  skills = skills.withXp(stat, xp);
  if (skills.values[stat].level !== INITIAL_LEVEL) throw new Error(`Native ${name} threshold differs`);
  skillFacts.push({name, stat, xp, level: skills.values[stat].level});
}
if (thrownFacts && thrownFacts.secondaryStat !== EMPTY_ITEM &&
    skills.baseLevel(thrownFacts.secondaryStat) < thrownFacts.secondaryLevel) throw new Error("Declared native secondary skill requirement is unmet");
const backpackSize = config.inv(inv.backpack).size;
const tools = [...new Set(rules.crossings.filter(row => row.faction === faction && row.tool !== undefined).map(row => row.tool))];
const carried = [...new Set([...equipment, ...combatEquipment, ...arenaEquipment, ...rejoinEquipment, ...tools])];
const foodCount = backpackSize - carried.length;
if (foodCount < SINGLE_ITEM) throw new Error("Native backpack lacks ordinary food space");
const backpack = [...carried.map(item => [item, item === obj.rune_arrow_ammunition || item === thrownAmmunition?.item ? AMMUNITION_COUNT : SINGLE_ITEM]),
  ...Array.from({length: foodCount}, () => [obj.shark, SINGLE_ITEM])];
const worn = Array.from({length: config.inv(inv.worn_equipment).size}, () => [EMPTY_ITEM, EMPTY_QUANTITY]);
const countBackings = new Map();
for (const row of rules.counts) {
  const bit = config.varbit(row.varbit);
  if (bit.domain?.id !== PLAYER_VAR_DOMAIN || bit.basevarId !== row.backing ||
      bit.startbit !== row.start || bit.endbit !== row.end) throw new Error("Initial native count contract changed");
  const value = row.faction === faction ? initialCount : EMPTY_QUANTITY;
  const mask = INTEGER_MASK >>> (INTEGER_HIGH_BIT - (bit.endbit - bit.startbit));
  const prior = countBackings.get(row.backing) ?? EMPTY_QUANTITY;
  countBackings.set(row.backing, (prior & ~(mask << bit.startbit)) | ((value & mask) << bit.startbit));
}
const windowDefinitions = config.enum(enums.native_interface_windows);
const windows = new Map(windowDefinitions.valuesArray.map((definition, key) => [key, definition]));
for (const [key, definition] of windowDefinitions.valuesMap) windows.set(key, definition);
const backpackDestinations = [...windows].filter(([, definition]) => definition === struct.backpack_window).map(([key]) => key);
if (backpackDestinations.length !== SINGLE_ITEM) throw new Error("Native Backpack destination absent or ambiguous");
// The finite actual-cache reader independently resolved this native parent.
// Loaded rooting/serial/operation and selection are still required in the core.
const prayerDestinations = [...windows].filter(([, definition]) =>
  [...config.struct(definition).params.values()].includes(component.game_window.prayer_slot)).map(([key]) => key);
if (prayerDestinations.length !== SINGLE_ITEM) throw new Error("Native Prayer parent destination absent or ambiguous");
const combatWindows = [...windows].filter(([, definition]) => definition === struct.melee_ability_window);
if (combatWindows.length !== SINGLE_ITEM || !Number.isInteger(combatWindows[FIRST_MATCH][FIRST_MATCH]) ||
    combatWindows[FIRST_MATCH][FIRST_MATCH] < FIRST_MATCH ||
    ![...config.struct(combatWindows[FIRST_MATCH][SINGLE_ITEM]).params.values()].includes(component.game_window.melee_slot))
  throw new Error("Actual native Combat destination/parent is absent or ambiguous");
const {GameplaySettingsDefinitions} = await owner("server/src/lostcity/definitions/GameplaySettingsDefinitions.ts");
const settingsDefinitions = GameplaySettingsDefinitions.of(config);
const settingsCategories = settingsDefinitions.categories;
const settingsRoute = definition => {
  const categories = settingsCategories.filter(row => row.settings.includes(definition));
  if (categories.length !== SINGLE_ITEM) throw new Error("Native Settings route is absent or ambiguous");
  const category = categories[FIRST_MATCH];
  const slots = category.settings.flatMap((value, index) => value === definition ? [index] : []);
  if (slots.length !== SINGLE_ITEM) throw new Error("Native Settings definition occurs more than once");
  const selectionPath = [], visited = new Set();
  let current = category;
  while (current) {
    if (!current.desktop || visited.has(current.index) || selectionPath.length >= settingsCategories.length)
      throw new Error("Native Settings ancestry is unsupported or cyclic");
    visited.add(current.index);
    const selected = settingsDefinitions.select(current.index);
    if (!selected) throw new Error("Native Settings category has no supported selection");
    selectionPath.unshift({index: current.index, parent: selected.parent, category: selected.category});
    const parents = settingsCategories.filter(row => row.children.includes(current.index));
    if (parents.length > SINGLE_ITEM) throw new Error("Native Settings ancestry is ambiguous");
    current = parents[FIRST_MATCH];
  }
  if (selectionPath.at(-SINGLE_ITEM)?.category !== category.index)
    throw new Error("Native Settings route does not select its setting category");
  return {category: category.index, slot: slots[FIRST_MATCH], definition, selectionPath};
};
const settings = {layout: settingsRoute(struct.legacy_interface_mode_option),
  slim: settingsRoute(struct.slim_window_headers_option)};

const prayerNames = kit.prayers;
const prayers = prayerNames.map(name => {
  const rows = combat.prayers.filter(row => row.name === name);
  if (rows.length !== SINGLE_ITEM) throw new Error(`Generated ${name} is absent or ambiguous`);
  return rows[FIRST_MATCH];
});
for (const prayer of prayers) {
  const chosen = combat.prayers.filter(row => row.button === prayer.button && row.level <= INITIAL_LEVEL)
    .sort((first, second) => second.level - first.level)[FIRST_MATCH];
  if (chosen?.action !== prayer.action) throw new Error(`Native higher-level prayer replaces ${prayer.name}`);
}
const campCrossings = rules.crossings.filter(row => row.faction === faction && row.stage === "camp");
const returnCrossings = rules.crossings.filter(row => row.faction === faction && row.stage === "camp-return").reverse();
if (!campCrossings.length || returnCrossings.length !== campCrossings.length) throw new Error("Native outward/return route cardinality changed");
const account = {version: ACCOUNT_VERSION, accountKey: ACCOUNT, members: true,
  ...kit.initialLocation, skills: skills.values, backpack, worn,
  savedVarps: [...countBackings]};
const restoreBeforeArena = kit.restoreBeforeArena === true;
let roomFoodPolicy = null;
if (restoreBeforeArena) {
  const general = arena.roles.find(row => row.general);
  const maximumIncomingLifeDamage = general?.splitSpecial?.maximum?.value;
  const food = foodDefinition(config, obj.shark);
  if (!campCrossings.some(row => row.prayerAfter === "empty") || !food ||
      !Number.isSafeInteger(maximumIncomingLifeDamage) || maximumIncomingLifeDamage < SINGLE_ITEM)
    throw new Error("Declared pre-fight altar/health strategy lacks its current source-qualified special/food/crossing facts");
  roomFoodPolicy = {maximumIncomingLifeDamage,
    latencyBuffer: Math.floor(food.maximumHealing / FOOD_LATENCY_BUFFER_PARTS),
    minimumLife: maximumIncomingLifeDamage + Math.floor(food.maximumHealing / FOOD_LATENCY_BUFFER_PARTS),
    qualification: "Generated solo split maximum plus an explicitly local half-Shark UI-latency buffer; ordinary native Eat only, not deterministic survival or a historic threshold"};
}
let saraKite = null;
if (faction === "saradomin") {
  const run = loadRunEnergyData();
  const generals = arena.roles.filter(row => row.general);
  const weapon = config.obj(obj.morrigan_javelin), qualified = qualifiedEquipment(weapon);
  const range = combat.ranged.find(row => row.item === weapon.id);
  const speedTicks = Number(weapon.params.get(param.attack_speed));
  if (generals.length !== SINGLE_ITEM || !generals[FIRST_MATCH].closePair ||
      !generals[FIRST_MATCH].attacks.every(row => row.closeOnly) || !range ||
      qualified.kind !== "ranged-thrown" || qualified.slot !== weapon.wearpos ||
      !Number.isSafeInteger(speedTicks) || speedTicks < SINGLE_ITEM ||
      !Number.isSafeInteger(run.baseDrainFine) || run.baseDrainFine < SINGLE_ITEM)
    throw new Error("Declared local ranged movement lacks its unchanged normal owners");
  const input = "combat/run-energy.json";
  saraKite = {role: generals[FIRST_MATCH].title, weapon: weapon.id, mainHandSlot: qualified.slot,
    range: range.range, speedTicks, runModeVarp: varp.run_mode,
    runDrainFine: run.baseDrainFine, runFinePerPercent: RUN_FINE_PER_PERCENT,
    runInitialFine: FULL_ENERGY,
    source: {input, sha256: createHash("sha256").update(fs.readFileSync(path.join(root, "server/data/generated", input))).digest("hex")},
    policy: "Local player strategy: general first, an actual ranged launch then ordinary map-bounded movement and source-cadence wait before a distinct fresh Attack; finite observed Run energy, no historic guide or guaranteed survival claim"};
}
const plan = {format: PLAN_FORMAT, accountKey: ACCOUNT, faction, arenaId: arena.id,
  ...(saraKite === null ? {} : {saraKite}),
  equipment, godAccessory: godAccessoryFacts, physicalEquipment, combatEquipment, arenaEquipment, rejoinEquipment, thrownAmmunition, thrownFacts, tools,
  recoveredAmmunition: [obj.rune_arrow_ammunition, ...(thrownAmmunition ? [thrownAmmunition.item] : [])],
  altarEquipment: [kit.godWeapon], combatStyle, equipmentFacts,
  food: {item: obj.shark, count: foodCount},
  campApproach: {mainHandSlot: config.obj(kit.godWeapon).wearpos,
    weapons: [...new Set([...combatEquipment, ...arenaEquipment, ...rejoinEquipment])].map(item =>
      ({item, range: combat.ranged.find(row => row.item === item)?.range ?? SINGLE_ITEM}))},
  campSelection: {slayerStat: statNamed(config, "Slayer"),
    policy: "Current native base level plus generated requirement; conservatively exclude task/protection/gear/special-attack restricted camp rows"},
  ammunition: combatStyle === "ranged" ? {item: obj.rune_arrow_ammunition, count: AMMUNITION_COUNT,
    meaning: "Declared normal initial stack; every ordinary shot must consume through ranged owner"} : null,
  route: {camp: campCrossings.map(row => row.id), campReturn: returnCrossings.map(row => row.id)},
  prayerPolicy: {campDrain: campCrossings.some(row => row.prayerAfter === "empty") ? "ordinary-empty" : "unchanged",
    depleted: "Crossing genuinely empties Prayer; only one ordinary admitted Pray-at may restore it before arena fights, with two later Teleports and no cooldown retry or fixture refill"},
  toolbar: {backpackDestination: backpackDestinations[FIRST_MATCH], prayerDestination: prayerDestinations[FIRST_MATCH],
    combatDestination: combatWindows[FIRST_MATCH][FIRST_MATCH], retaliationVarp: varp.auto_retaliate_disabled, settings},
  transportPolicy: "Ordinary Settings enables Slim headers and preserves original layout before native retaliation OFF; no seeded preferences; explicit Attack unchanged",
  prayers, restoreBeforeArena, roomFoodPolicy, initialCount, initialLocation: kit.initialLocation,
  initialFixture: {qualification: "declared-before-login-only", skillFacts,
    worn: "empty; actual Wield/Wear required after authentication",
    counts: "Previously earned 39 are declared; one actual camp kill must supply the fortieth",
    entry: "Starts inside the accessible dungeon; first surface quest progression is held",
    armadyl: "Initial camp uses actual MSB/Rune arrows, later roles/rejoin use the declared nondegrading javelin stack; Godsword supplies travel/altar association and the Prayer-gated worn stole blocks new own-faction camp acquisition only; no Prayer-drain bonus is counted",
    armour: physicalEquipment.length ? "Fresh generated Barrows armour is carried without UUID/charge metadata; actual Wear installs the sole physical balance, ordinary combat must produce the used identity and charge wear" : "Previously recorded Bandos Bronze kit remains separate",
    random: "World ordinary RNG unchanged; no fixture stream"}};
const digest = bytes => createHash("sha256").update(bytes).digest("hex");
const directory = path.join(work, "players/accounts");
fs.mkdirSync(directory, {recursive: true});
const accountPath = path.join(directory, digest(Buffer.from(ACCOUNT)) + ".json");
const writeFresh = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, INDENT), {flag: "wx"});
writeFresh(accountPath, account);
writeFresh(path.join(work, "plan.json"), plan);
writeFresh(path.join(work, "initial-fixture.json"), {accountPath,
  accountSha256: digest(fs.readFileSync(accountPath)), account, plan,
  source: {root, seedSha256: digest(fs.readFileSync(new URL(import.meta.url))),
    inputs: Object.fromEntries(["instances/gwd.json", "combat/rules.json", "food/foods.json", "slayer/rules.json", "combat/charges.json",
      ...(saraKite === null ? [] : [saraKite.source.input])]
      .map(relative => [relative, digest(fs.readFileSync(path.join(root, "server/data/generated", relative)))]))}});
