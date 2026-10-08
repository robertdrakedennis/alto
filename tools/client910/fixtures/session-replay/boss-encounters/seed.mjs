// Pure initial endgame fixture, complete before ordinary login and simulation.
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
const ACCOUNT = "alice";
const FIRST = 0;
const ONE = 1;
const root = process.env.ALTO_BOSS_RUNTIME_ROOT;
const work = process.argv[FIRST_ARGUMENT];
if (!root || !path.isAbsolute(root) || !work || !path.isAbsolute(work))
  throw new Error("Own runtime and initial scratch required");
const owner = relative => import(pathToFileURL(path.join(root, relative)).href);
const [{default: CacheProvider}, {endgameFixtureLoadout}, {loadBossRooms}, {loadCombatData},
       {enums, struct, component, varp}] = await Promise.all([
  owner("server/src/lostcity/server/CacheProvider.ts"),
  owner("server/src/lostcity/systems/equipment/EndgameFixture.testkit.ts"),
  owner("server/src/lostcity/data/BossRoomData.ts"),
  owner("server/src/lostcity/data/CombatData.ts"),
  owner("packages/domain/src/index.ts"),
]);
await CacheProvider.load(path.join(root, "server/data/pack"));
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const room = loadBossRooms().find(row => row.id === process.argv[ROOM_ARGUMENT]);
if (!room?.inhabitants?.length || !room.entryTile || !room.exitTile)
  throw new Error("Qualified multi-inhabitant room is absent");
const choice = {style: "melee", tier: MELEE_WEAPON_TIER, armourTier: MELEE_ARMOUR_TIER, armour: "power", hands: "two-handed"};
const kit = await endgameFixtureLoadout(choice);
const combat = loadCombatData();
const offensive = kit.prayers.map(action => combat.prayers.find(row => row.action === action));
const protections = combat.prayers.filter(row => row.group === "protect-magic").sort((left, right) => right.level - left.level);
const prayers = [...offensive, protections[FIRST]];
if (prayers.some(row => !row)) throw new Error("Supported native endgame prayers are absent");
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
const target = room.inhabitants.find(row => row.npc === room.npc);
const guaranteed = target?.drops.filter(row => row.kind === "always" && !row.unavailable
  && config.obj(row.item).name.toLowerCase().includes("bones"));
if (guaranteed?.length !== ONE) throw new Error("Qualified guaranteed bones are absent or ambiguous");
const account = {version: ACCOUNT_VERSION, accountKey: ACCOUNT, members: true,
  ...room.exit, coins: room.encounter.cost, skills: kit.skills,
  savedVarps: [[varp.auto_retaliate_disabled, RETALIATION_DISABLED]],
  backpack: kit.backpack, worn: kit.worn};
if ("resources" in account) throw new Error("Initial fixture must not override live resource rules");
const accountPath = path.join(work, "players/accounts", createHash("sha256").update(ACCOUNT).digest("hex") + ".json");
fs.mkdirSync(path.dirname(accountPath), {recursive: true});
fs.writeFileSync(accountPath, JSON.stringify(account), {flag: "wx"});
const roomSelectors = Object.fromEntries(["id", "entrance", "exit", "entryLoc", "entryTile", "exitLoc", "exitTile"]
  .map(name => [name, room[name]]));
const plan = {format: ONE, accountKey: ACCOUNT, room: roomSelectors,
  kings: room.inhabitants.map(row => ({npc: row.npc, maximumLife: row.profile.hitpoints})),
  target: {npc: target.npc, profile: {hitpoints: target.profile.hitpoints}},
  equipment: kit.loadout.equipment, loadout: kit.loadout,
  prayers, toolbar, food: kit.food, loot: guaranteed[FIRST].item,
  source: {gearChoice: choice, blockedResources: kit.blockedResources,
    initialSkillsAndSavedPreferenceOnly: true}};
fs.writeFileSync(path.join(work, "plan.json"), JSON.stringify(plan, null, ONE) + "\n", {flag: "wx"});
fs.writeFileSync(path.join(work, "initial-fixture.json"), JSON.stringify({
  accountPath, accountSha256: createHash("sha256").update(fs.readFileSync(accountPath)).digest("hex"),
  account, source: {seedSha256: createHash("sha256").update(fs.readFileSync(new URL(import.meta.url))).digest("hex")},
  scope: "Scratch initial fixture only; no simulation, account replacement or live assistance",
}, null, ONE) + "\n", {flag: "wx"});
