// Pure initial full two-kit Queen fixture, before native login.
import fs from "node:fs";
import path from "node:path";
import {createHash} from "node:crypto";
import {pathToFileURL} from "node:url";

const FIRST_ARGUMENT = 2;
const ROOM_ARGUMENT = 3;
const ACCOUNT_VERSION = 2;
const RETALIATION_DISABLED = 1;
const MELEE_WEAPON_TIER = 92;
const MELEE_ARMOUR_TIER = 90;
const RANGED_TIER = 90;
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
const [{default: CacheProvider}, {endgameFixtureLoadout}, {loadBossRooms}, {loadCombatData},
       {enums, struct, component, varp, inv, obj}, {loadEndgameLoadouts}, {foodDefinition}] = await Promise.all([
  owner("server/src/lostcity/server/CacheProvider.ts"),
  owner("server/src/lostcity/systems/equipment/EndgameFixture.testkit.ts"),
  owner("server/src/lostcity/data/BossRoomData.ts"),
  owner("server/src/lostcity/data/CombatData.ts"),
  owner("packages/domain/src/index.ts"),
  owner("server/src/lostcity/data/EndgameLoadoutData.ts"),
  owner("server/src/lostcity/data/FoodData.ts"),
]);
await CacheProvider.load(path.join(root, "server/data/pack"));
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const room = loadBossRooms().find(row => row.id === process.argv[ROOM_ARGUMENT]);
if (!room?.queen || !room.entryTile || !room.exitTile)
  throw new Error("Qualified multi-inhabitant room is absent");
const choices = {
  ground: {style: "melee", tier: MELEE_WEAPON_TIER, armourTier: MELEE_ARMOUR_TIER, armour: "power", hands: "two-handed"},
  flying: {style: "ranged", tier: RANGED_TIER, armourTier: RANGED_TIER, armour: "power", hands: "dual-wield"},
};
const ground = await endgameFixtureLoadout(choices.ground);
const flying = await endgameFixtureLoadout(choices.flying);
if (JSON.stringify(ground.skills) !== JSON.stringify(flying.skills))
  throw new Error("Two admitted native skill seeds disagree");
const catalog = loadEndgameLoadouts();
const combat = loadCombatData();
const protect = combat.prayers.filter(row => row.group === "protect-magic")
  .sort((left, right) => right.level - left.level)[FIRST];
const phasePrayers = kit => [...kit.prayers.map(action => combat.prayers.find(row => row.action === action)), protect];
const allPrayers = [...phasePrayers(ground), ...phasePrayers(flying)];
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
for (const kit of [ground, flying]) {
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
const kits = Object.fromEntries([["ground", ground], ["flying", flying]].map(([phase, kit]) => [phase, {
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
const rope = room.ropes?.find(row => row.entry);
if (!rope || rope.attached !== room.entryLoc)
  throw new Error("Native upper-hive attached rope history is absent");
const bit = config.varbit(rope.bit);
if (bit.basevarId !== rope.backing || bit.startbit !== rope.start || bit.endbit !== rope.end)
  throw new Error("Native upper-rope variable binding changed");
const attachedHistory = ONE << bit.startbit;
const account = {version: ACCOUNT_VERSION, accountKey: ACCOUNT, members: true,
  ...room.exit, coins: room.encounter.cost, skills: ground.skills,
  savedVarps: [[varp.auto_retaliate_disabled, RETALIATION_DISABLED], [rope.backing, attachedHistory]],
  backpack, worn: ground.worn};
if ("resources" in account) throw new Error("Initial fixture must not override live resource rules");
const accountPath = path.join(work, "players/accounts", createHash("sha256").update(ACCOUNT).digest("hex") + ".json");
fs.mkdirSync(path.dirname(accountPath), {recursive: true});
fs.writeFileSync(accountPath, JSON.stringify(account), {flag: "wx"});
const roomSelectors = Object.fromEntries(["id", "entrance", "exit", "spawn", "entryLoc", "entryTile", "exitLoc", "exitTile"]
  .map(name => [name, room[name]]));
const forms = [room.queen.ground.profile, room.queen.flying.profile];
const plan = {format: ONE, accountKey: ACCOUNT,
  room: {...roomSelectors, entryParent: rope.parent},
  kings: forms.map(row => ({npc: row.npc, maximumLife: row.hitpoints})),
  ground: {npc: forms[FIRST].npc, maximumLife: forms[FIRST].hitpoints},
  flying: {npc: forms[ONE].npc, maximumLife: forms[ONE].hitpoints},
  nativeForms: forms.map(row => ({npc: row.npc, bas: config.npc(row.npc).bas})),
  equipment: ground.loadout.equipment, loadout: ground.loadout, kits, currencyItem: obj.coins,
  prayers, toolbar, food,
  source: {gearChoices: choices, blockedResources: [...new Set([...ground.blockedResources, ...flying.blockedResources])],
    initialSkillsAndSavedPreferenceOnly: true,
    fixtureHistory: "Upper-hive rope already attached; not a rope/traversal proof",
    scope: "Full two-form ordinary fight, native gear switch, actual final roll/Take or legitimate empty, native exit/rejoin"}};
fs.writeFileSync(path.join(work, "plan.json"), JSON.stringify(plan, null, ONE) + "\n", {flag: "wx"});
fs.writeFileSync(path.join(work, "initial-fixture.json"), JSON.stringify({
  accountPath, accountSha256: createHash("sha256").update(fs.readFileSync(accountPath)).digest("hex"),
  account, source: {seedSha256: createHash("sha256").update(fs.readFileSync(new URL(import.meta.url))).digest("hex")},
  scope: "Scratch initial fixture only; no simulation, account replacement or live assistance",
}, null, ONE) + "\n", {flag: "wx"});
