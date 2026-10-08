// Private initial-account proposal only. Run before login, never against a live account.
import fs from "node:fs";
import path from "node:path";
import {createHash, randomUUID} from "node:crypto";
import {pathToFileURL} from "node:url";

const FIRST_ARGUMENT = 2;
const SINGLE_ITEM = 1;
const EMPTY_ITEM = -1;
const INITIAL_COINS = 300000;
const MAXIMUM_COMBAT_XP = 13034431;
const INITIAL_SMITHING_LEVEL = 60;
const INITIAL_CONSTRUCTION_LEVEL = 55;
const HALF_CAPACITY_DENOMINATOR = 2;
const SOUTH_SIDE = 3;
const NO_ROTATION = 0;
const HOUSE_SIDES = 4;
const ACCOUNT_VERSION = 2;
const FIXTURE_FORMAT = 1;
const INDENT = 2;
const HOOD_NAME = "Ahrim's hood";
const BOB_NAME = "Bob";
const WEAR_TARGET_NAME = "Rat";
const MAXIMUM_WEAR_TARGET_LEVEL = 4;
const PRAYER_WINDOW_TITLE = "Prayer Abilities";
const PROTECTION_GROUPS = ["protect-magic", "protect-ranged", "protect-melee"];
const root = process.env.ALTO_BARROWS_RUNTIME_ROOT;
if (!root || !path.isAbsolute(root)) throw new Error("Explicit ordinary runtime checkout required");
const owner = relative => import(pathToFileURL(path.join(root, relative)).href);
const work = process.argv[FIRST_ARGUMENT];
const basePlanPath = process.argv[FIRST_ARGUMENT + SINGLE_ITEM];
if (!work || !basePlanPath) throw new Error("Supply fresh scratch and preserved initial plan");
const [{default: CacheProvider}, {default: SkillSet, loadSkillDefinitions}, {statNamed},
  {loadBarrows}, {loadCombatData}, {loadItemChargeData}, {loadNpcData}, {loadConstructionData, DEFAULT_LOCATION, PORTALS},
  {default: EstateAgent}, {beyond, roomDoors}, {obj, inv, loc, location, enums, struct, component, param, varbit, varp}] = await Promise.all([
  owner("server/src/lostcity/server/CacheProvider.ts"),
  owner("server/src/lostcity/systems/stats/SkillSet.ts"),
  owner("server/src/lostcity/systems/stats/StatNames.ts"),
  owner("server/src/lostcity/data/BarrowsData.ts"),
  owner("server/src/lostcity/data/CombatData.ts"),
  owner("server/src/lostcity/data/ItemChargeData.ts"),
  owner("server/src/lostcity/data/NpcData.ts"),
  owner("server/src/lostcity/content/skills/construction/data.ts"),
  owner("server/src/lostcity/content/skills/construction/estateAgent.ts"),
  owner("server/src/lostcity/content/skills/construction/houses.ts"),
  owner("packages/domain/src/index.ts"),
]);
await CacheProvider.load(path.join(root, "server/data/pack"));
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const windowDefinitions = config.enum(enums.native_interface_windows);
const nativeWindows = new Map(windowDefinitions.valuesArray.map((definition, key) => [key, definition]));
for (const [key, definition] of windowDefinitions.valuesMap) nativeWindows.set(key, definition);
const backpackDestinations = [...nativeWindows].filter(([, definition]) => definition === struct.backpack_window)
  .map(([key]) => key);
if (backpackDestinations.length !== SINGLE_ITEM || !Number.isInteger(backpackDestinations[0]) || backpackDestinations[0] < 0)
  throw new Error("Actual native Backpack destination is absent or ambiguous");
const prayerWindows = [...nativeWindows].filter(([, definition]) => {
  const values = [...config.struct(definition).params.values()];
  return values.includes(PRAYER_WINDOW_TITLE) && values.includes(component.game_window.prayer_slot);
});
if (prayerWindows.length !== SINGLE_ITEM || !Number.isInteger(prayerWindows[0][0]) || prayerWindows[0][0] < 0)
  throw new Error("Actual native Prayer title/parent destination is absent or ambiguous");
const combatWindows = [...nativeWindows].filter(([, definition]) => definition === struct.melee_ability_window);
if (combatWindows.length !== SINGLE_ITEM || !Number.isInteger(combatWindows[0][0]) || combatWindows[0][0] < 0 ||
  ![...config.struct(combatWindows[0][1]).params.values()].includes(component.game_window.melee_slot))
  throw new Error("Actual native Legacy combat destination/parent is absent or ambiguous");
const {GameplaySettingsDefinitions} = await owner("server/src/lostcity/definitions/GameplaySettingsDefinitions.ts");
const settingsDefinitions = GameplaySettingsDefinitions.of(config);
const settingsCategories = settingsDefinitions.categories;
const settingsRoute = definition => {
  const categories = settingsCategories.filter(row => row.settings.includes(definition));
  if (categories.length !== SINGLE_ITEM) throw new Error("Native Settings route is absent or ambiguous");
  const category = categories[0];
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
    current = parents[0];
  }
  if (selectionPath.at(-SINGLE_ITEM)?.category !== category.index)
    throw new Error("Native Settings route does not select its setting category");
  return {category: category.index, slot: slots[0], definition, selectionPath};
};
const settings = {layout: settingsRoute(struct.legacy_interface_mode_option),
  slim: settingsRoute(struct.slim_window_headers_option)};

const toolbar = {backpackDestination: backpackDestinations[0], prayerDestination: prayerWindows[0][0],
  combatDestination: combatWindows[0][0], retaliationVarp: varp.auto_retaliate_disabled,
  prayerWindow: prayerWindows[0][1], prayerTitle: PRAYER_WINDOW_TITLE, settings};
const rules = loadBarrows();
const charges = loadItemChargeData();
const families = charges.filter(row => row.equipment?.name === HOOD_NAME && row.freshCharges !== null);
if (families.length !== SINGLE_ITEM) throw new Error("Ahrim hood fresh physical family is ambiguous");
const family = families[0];
if (!family.usedItem || !family.coinRepair || family.depleted.kind !== "identity") throw new Error("Incomplete native repair family");
const used = charges.find(row => row.item === family.usedItem);
if (!used || used.freshCharges !== null || used.capacity !== family.capacity || used.coinRepair?.output !== family.item)
  throw new Error("Used hood balance/output is not qualified");
const bobRows = rules.repairers.filter(row => config.npc(row.npc).name === BOB_NAME && row.allOperation !== null);
if (bobRows.length !== SINGLE_ITEM) throw new Error("Generated Bob Repair-all selector is ambiguous");
const bob = bobRows[0];
const bobOperation = config.npc(bob.npc).op?.[bob.allOperation - SINGLE_ITEM];
if (bobOperation?.toLowerCase() !== "repair-all") throw new Error("Current Bob option differs from generated repairer");
const npcData = loadNpcData();
const bobSpawns = npcData.spawns.filter(row => row.npc === bob.npc);
if (bobSpawns.length !== SINGLE_ITEM) throw new Error("Generated ordinary Bob placement is ambiguous");
const bobSpawn = bobSpawns[0];
const wearSpawns = npcData.spawns.filter(row => {
  const profile = npcData.profiles.get(row.npc);
  return profile?.name === WEAR_TARGET_NAME && profile.level > 0 && profile.level <= MAXIMUM_WEAR_TARGET_LEVEL &&
    profile.hitpoints > 0 && row.level === bobSpawn.level;
}).sort((left, right) => {
  const distance = row => Math.max(Math.abs(row.x - bobSpawn.x), Math.abs(row.z - bobSpawn.z));
  return distance(left) - distance(right) || left.x - right.x || left.z - right.z;
});
if (!wearSpawns.length) throw new Error("No generated ordinary Rat exists for outgoing wear");
const wearSpawn = wearSpawns[0];
const wearProfile = npcData.profiles.get(wearSpawn.npc);
if (!config.npc(wearSpawn.npc).op?.includes("Attack")) throw new Error("Wear target has no native Attack");
const construction = loadConstructionData(config);
const rooms = new EstateAgent(construction).starterRooms();
const garden = rooms.find(row => row.room === obj.garden_room);
const workshopRow = construction.rooms.get(obj.workshop_room);
const stand = rules.repairStand;
if (!garden || !workshopRow || !stand || stand.furniture !== obj.armour_stand_furniture || stand.loc !== loc.house_armour_repair_stand)
  throw new Error("Named house/stand contract is missing");
const hotspot = workshopRow.hotspots.findIndex(row => row.loc === loc.house_repair_space);
if (hotspot < 0 || !roomDoors(workshopRow, NO_ROTATION).includes((SOUTH_SIDE + HALF_CAPACITY_DENOMINATOR) % HOUSE_SIDES))
  throw new Error("Workshop lacks the source-qualified north doorway/repair space");
rooms.push({room: workshopRow.obj, ...beyond(garden, SOUTH_SIDE), rotation: NO_ROTATION,
  furniture: [{hotspot, obj: stand.furniture}]});
const portal = PORTALS.find(row => row.location === DEFAULT_LOCATION);
if (!portal || portal.loc !== loc.taverley_house_portal) throw new Error("Initial house must use qualified Taverley portal");
const definitions = await loadSkillDefinitions(CacheProvider.js5);
let skills = new SkillSet(definitions);
for (const name of ["Attack", "Defence", "Strength", "Constitution", "Prayer"])
  skills = skills.withXp(statNamed(config, name), MAXIMUM_COMBAT_XP);
for (const [name, level] of [["Smithing", INITIAL_SMITHING_LEVEL], ["Construction", INITIAL_CONSTRUCTION_LEVEL]]) {
  const stat = statNamed(config, name);
  const definition = definitions[stat];
  const index = level - definition.base - SINGLE_ITEM;
  if (index < 0 || !Number.isInteger(definition.table[index])) throw new Error("Native skill threshold unavailable");
  skills = skills.withXp(stat, definition.table[index]);
  if (skills.values[stat].level !== level) throw new Error("Native threshold did not give the declared initial level");
}
const combat = loadCombatData();
const prayerLevel = skills.values[statNamed(config, "Prayer")].level;
const activationBits = {
  "protect-magic": varbit.active_prayer_magic_guard,
  "protect-ranged": varbit.active_prayer_ranged_guard,
  "protect-melee": varbit.active_prayer_melee_guard,
};
const prayerOrder = config.enum(enums.prayer_selection_order).valuesArray;
const prayerActions = config.enum(enums.prayer_actions);
const prayerDefinitions = prayerActions.valuesArray.length ? prayerActions.valuesArray : [...prayerActions.valuesMap.values()];
const prayers = PROTECTION_GROUPS.map(group => {
  const rows = combat.prayers.filter(row => row.group === group && row.level <= prayerLevel);
  if (rows.length !== SINGLE_ITEM) throw new Error(`Generated unlocked ${group} is absent or ambiguous`);
  const rule = rows[0];
  const chosen = combat.prayers.filter(row => row.button === rule.button && row.level <= prayerLevel)
    .sort((left, right) => right.level - left.level)[0];
  if (chosen?.action !== rule.action || rule.activationBit !== activationBits[group])
    throw new Error(`Generated/native protection selection differs for ${rule.name}`);
  const native = prayerDefinitions.filter(definitionId => {
    const definition = config.struct(definitionId);
    const tier = definition.params.get(param.prayer_tier_group) ?? definitionId;
    return definition.params.get(param.spell_name) === rule.name &&
      definition.params.get(param.combat_action_code) === rule.action &&
      definition.params.get(param.combat_action_level) === rule.level &&
      prayerOrder.indexOf(tier) === rule.button;
  });
  if (native.length !== SINGLE_ITEM) throw new Error(`Native standard-book row differs for ${rule.name}`);
  return {name: rule.name, group: rule.group, button: rule.button, activationBit: rule.activationBit};
});
const basePlanBytes = fs.readFileSync(basePlanPath);
const basePlan = JSON.parse(basePlanBytes);
const originalEquipment = [obj.abyssal_whip, obj.wooden_shield, obj.bronze_full_helm,
  obj.bronze_platebody, obj.bronze_platelegs, obj.bronze_gauntlets, obj.bronze_armoured_boots];
const carriedEquipment = originalEquipment.filter(item => item !== obj.bronze_full_helm);
const hoodKey = randomUUID();
const damaged = [family.usedItem, SINGLE_ITEM, {key: hoodKey, charges: Math.floor(family.capacity / HALF_CAPACITY_DENOMINATOR)}];
const backpackSize = config.inv(inv.backpack).size;
const originalFoodCount = backpackSize - originalEquipment.length - SINGLE_ITEM;
const backpack = [...carriedEquipment.map(item => [item, SINGLE_ITEM]), [obj.spade, SINGLE_ITEM],
  ...Array.from({length: originalFoodCount}, () => [obj.shark, SINGLE_ITEM]), damaged];
if (backpack.length !== backpackSize) throw new Error("Initial fixture must preserve all original Shark slots");
const worn = Array.from({length: config.inv(inv.worn_equipment).size}, () => [EMPTY_ITEM, 0]);
const helmetSlot = config.obj(obj.bronze_full_helm).wearpos;
if (!Number.isInteger(helmetSlot) || helmetSlot < 0 || helmetSlot >= worn.length) throw new Error("Native initial helmet slot missing");
worn[helmetSlot] = [obj.bronze_full_helm, SINGLE_ITEM];
const accountKey = basePlan.accountKey;
if (typeof accountKey !== "string" || !accountKey) throw new Error("Preserved initial plan lacks the initial account identity");
const account = {version: ACCOUNT_VERSION, accountKey, ...rules.crypts[0].surface, members: true,
  coins: INITIAL_COINS, skills: skills.values, backpack, worn,
  house: {location: DEFAULT_LOCATION, buildMode: false, rooms}};
const digest = bytes => createHash("sha256").update(bytes).digest("hex");
const repair = {
  format: FIXTURE_FORMAT, sourceOnlyInitialFixture: true,
  hood: {name: HOOD_NAME, key: hoodKey, family},
  bob: {...bob, operation: bobOperation, spawn: bobSpawn},
  wear: {spawn: wearSpawn, profile: wearProfile, operation: "Attack"},
  house: {portal, saved: account.house, stand},
  travel: {lumbridge: {...location.lumbridge_lodestone}, burthorpe: {...location.burthorpe_lodestone}},
  smithing: {stat: stand.smithingStat, initialLevel: INITIAL_SMITHING_LEVEL},
  originalFoodCount, originalEquipment, initiallyWorn: obj.bronze_full_helm,
  qualification: ["Bob/Rat placements retain their generated spawn provenance, not pinned-2019 coordinates",
    "Initial owned house uses the existing starter-room policy and the previously tested south Workshop connection",
    "No lodestone/quest unlock variable is seeded: Lumbridge and Burthorpe are normal start-unlocked destinations",
    "No charge, account, position, item or skill mutation occurs after login"]
};
const plan = {accountKey, equipment: carriedEquipment, toolbar, prayers,
  prayerPolicy: {selection: "Only the unique generated ordinary incoming brother style",
    depletion: "Observed genuine Prayer exhaustion, no replenishment or blind action retry",
    namespace: "Actual loaded prayer_book.prayer_buttons, serial, native operation and PLAYER activation publication",
    qualification: "Native cache title/mount/order facts; fresh CS2 row and actual activation remain required observations"},
  repair, preservedSourcePlan: {path: basePlanPath, sha256: digest(basePlanBytes)},
  initialFixture: {sourceOnlyProposal: true, coins: INITIAL_COINS,
    smithing: INITIAL_SMITHING_LEVEL, construction: INITIAL_CONSTRUCTION_LEVEL,
    meaning: "Same fight equipment and twenty carried Sharks; only the bronze helmet begins worn to free one damaged-hood slot"}};
const directory = path.join(work, "players/accounts");
fs.mkdirSync(directory, {recursive: true});
const accountPath = path.join(directory, digest(Buffer.from(accountKey)) + ".json");
const writeFresh = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, INDENT), {flag: "wx"});
writeFresh(accountPath, account);
writeFresh(path.join(work, "plan.json"), plan);
writeFresh(path.join(work, "initial-repair-fixture.json"), {kind: "Declared pre-login fixture, not live actions",
  accountPath, accountSha256: digest(fs.readFileSync(accountPath)), account, repair,
  source: {runtimeRoot: root, basePlanSha256: digest(basePlanBytes)}});
