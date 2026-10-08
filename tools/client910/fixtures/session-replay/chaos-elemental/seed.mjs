// Pure pre-login full-level melee fixture for the ordinary Chaos encounter.
import fs from "node:fs";
import path from "node:path";
import {createHash} from "node:crypto";
import {pathToFileURL} from "node:url";

const FIRST_ARGUMENT = 2;
const ACCOUNT_VERSION = 2;
const RETALIATION_DISABLED = 1;
const MELEE_WEAPON_TIER = 90;
const MELEE_ARMOUR_TIER = 90;
const FULL_BOSS_LIFE = 17250;
const ITEM_FIELD = 0;
const QUANTITY_FIELD = 1;
const SINGLE_ITEM_COUNT = 1;
const ACCOUNT = "alice";
const FIRST = 0;
const ONE = 1;
const root = process.env.ALTO_BOSS_RUNTIME_ROOT;
const work = process.argv[FIRST_ARGUMENT];
if (!root || !path.isAbsolute(root) || !work || !path.isAbsolute(work))
  throw new Error("Own runtime and initial scratch required");
const owner = relative => import(pathToFileURL(path.join(root, relative)).href);
const [{default: CacheProvider}, {endgameFixtureLoadout}, {loadChaosElementalData}, {loadCombatData},
       {enums, struct, component, varp, inv, obj}, {loadEndgameLoadouts}, {foodDefinition}] = await Promise.all([
  owner("server/src/lostcity/server/CacheProvider.ts"),
  owner("server/src/lostcity/systems/equipment/EndgameFixture.testkit.ts"),
  owner("server/src/lostcity/data/ChaosElementalData.ts"),
  owner("server/src/lostcity/data/CombatData.ts"),
  owner("packages/domain/src/index.ts"),
  owner("server/src/lostcity/data/EndgameLoadoutData.ts"),
  owner("server/src/lostcity/data/FoodData.ts"),
]);
await CacheProvider.load(path.join(root, "server/data/pack"));
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const data = loadChaosElementalData();
if (data.profile.hitpoints !== FULL_BOSS_LIFE) throw new Error("Full dated Chaos life changed");
const choices = {melee: {style: "melee", tier: MELEE_WEAPON_TIER, armourTier: MELEE_ARMOUR_TIER, armour: "power", hands: "dual-wield"}};
const ground = await endgameFixtureLoadout(choices.melee);
const catalog = loadEndgameLoadouts();
const combat = loadCombatData();
const protect = combat.prayers.filter(row => row.group === "protect-magic")
  .sort((left, right) => right.level - left.level)[FIRST];
const phasePrayers = kit => [...kit.prayers.map(action => combat.prayers.find(row => row.action === action)), protect];
const allPrayers = phasePrayers(ground);
if (allPrayers.some(row => !row)) throw new Error("Supported native endgame prayers are absent");
const prayers = [...new Map(allPrayers.map(row => [row.activationBit, row])).values()];
const physical = row => {
  if (row.blocked.length || !row.equipment.active) throw new Error("Blocked phase gear is not admitted");
  const family = catalog.families.find(value => value.item === row.item);
  const identities = [row.item];
  if (family?.usedItem !== null && family?.usedItem !== undefined) {
    const used = catalog.families.find(value => value.item === family.usedItem);
    if (!used?.equipment?.active || used.equipment.slot !== row.equipment.slot || used.capacity !== family.capacity)
      throw new Error("Native worn physical used identity is unqualified");
    identities.push(family.usedItem);
  }
  return {item: row.item, slot: row.equipment.slot, identities: [...new Set(identities)],
    capacity: family?.capacity ?? null, name: row.name};
};
const supplies = new Map();
const include = (item, count) => supplies.set(item, Math.max(supplies.get(item) ?? FIRST, count));
for (const kit of [ground]) {
  for (const row of kit.equipment) {
    const initial = kit.backpack.find(slot => slot[ITEM_FIELD] === row.item);
    if (!initial) throw new Error("Native kit equipment/ammunition initial count is absent");
    include(row.item, initial[QUANTITY_FIELD]);
  }
  for (const rune of kit.spell?.runes ?? []) {
    const initial = kit.backpack.find(slot => slot[ITEM_FIELD] === rune.item);
    if (!initial) throw new Error("Native kit initial rune count is absent");
    include(rune.item, initial[QUANTITY_FIELD]);
  }
}
// Shark is the existing ordinary native controller food consumer; qualify it, never relabel another item.
const foodDefinitionRow = foodDefinition(config, obj.shark);
if (!foodDefinitionRow || !catalog.resources.food.includes(obj.shark))
  throw new Error("Supported native Shark food is absent from the admitted resource catalog");
const food = {item: obj.shark, maximumHealing: foodDefinitionRow.maximumHealing};
const backpack = [...supplies].map(([item, count]) => [item, count]);
const capacity = config.inv(inv.backpack).size;
if (backpack.length >= capacity) throw new Error("Two merged native kits leave no ordinary food capacity");
while (backpack.length < capacity) backpack.push([food.item, SINGLE_ITEM_COUNT]);
const kits = Object.fromEntries([["melee", ground]].map(([phase, kit]) => [phase, {
  choice: choices[phase], loadout: kit.loadout, equipment: kit.equipment.map(physical), prayers: phasePrayers(kit),
}]));
const definitions = config.enum(enums.native_interface_windows);
const windows = new Map(definitions.valuesArray.map((value, key) => [key, value]));
for (const [key, value] of definitions.valuesMap) windows.set(key, value);
const destination = predicate => {
  const matching = [...windows].filter(([, definition]) => predicate(definition));
  if (matching.length !== ONE) throw new Error("Native window destination is not unique");
  return matching[FIRST][FIRST];
};
const toolbar = {
  backpackDestination: destination(definition => definition === struct.backpack_window),
  prayerDestination: destination(definition => [...config.struct(definition).params.values()].includes(component.game_window.prayer_slot)),
  retaliationVarp: varp.auto_retaliate_disabled,
};
const account = {version: ACCOUNT_VERSION, accountKey: ACCOUNT, members: true,
  ...data.fixture.retreat, coins: FIRST, skills: ground.skills,
  savedVarps: [[varp.auto_retaliate_disabled, RETALIATION_DISABLED]], backpack, worn: ground.worn};
if ("resources" in account) throw new Error("No live-resource fixture override");
const accountPath = path.join(work, "players/accounts", createHash("sha256").update(ACCOUNT).digest("hex") + ".json");
fs.mkdirSync(path.dirname(accountPath), {recursive: true});
fs.writeFileSync(accountPath, JSON.stringify(account), {flag: "wx"});
const plan = {format: ONE, accountKey: ACCOUNT, arrival: data.fixture.arrival, retreat: data.fixture.retreat,
  spawn: data.policy.spawn, scanRadius: data.policy.range,
  target: {npc: data.npc, profile: data.profile}, kings: [{npc: data.npc, maximumLife: FULL_BOSS_LIFE}],
  kits, equipment: ground.loadout.equipment, loadout: ground.loadout, prayers, toolbar, food, currencyItem: obj.coins,
  policy: data.policy, source: {choice: choices.melee, noResourcesOverride: true,
    scope: "Full-life-loot-retreat; natural Madness/Confusion observations are conditional"}};
fs.writeFileSync(path.join(work, "plan.json"), JSON.stringify(plan, null, ONE) + "\n", {flag: "wx"});
fs.writeFileSync(path.join(work, "initial-fixture.json"), JSON.stringify({accountPath,
  accountSha256: createHash("sha256").update(fs.readFileSync(accountPath)).digest("hex"), account,
  source: {seedSha256: createHash("sha256").update(fs.readFileSync(new URL(import.meta.url))).digest("hex")},
  scope: "Initial native kit only; full backpack obtains vacancies through ordinary Wear/Wield after login"}, null, ONE) + "\n", {flag: "wx"});
