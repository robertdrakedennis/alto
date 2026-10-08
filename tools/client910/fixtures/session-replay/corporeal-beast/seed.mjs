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
       {enums, struct, component, varp, npc, param}] = await Promise.all([
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
if (!room?.corp || !room?.inhabitants?.length || !room.entryTile || !room.exitTile)
  throw new Error("Qualified Corp room absent; no generic boss substitution");
const choice = {style: "melee", tier: MELEE_WEAPON_TIER, armourTier: MELEE_ARMOUR_TIER, armour: "power", hands: "two-handed"};
const kit = await endgameFixtureLoadout(choice);
const combat = loadCombatData();
const protections = combat.prayers.filter(row => row.group === "protect-melee").sort((left, right) => right.level - left.level);
const prayers = [protections[FIRST]];
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
const FULL_BEAST_LIFE = 100000;
const FULL_CORE_LIFE = 3500;
const NATIVE_CORE_MAXIMUM = 600;
const CORE_FOOTPRINT_TILES = 3;
const HALVING_NUMERATOR = 1;
const HALVING_DENOMINATOR = 2;
const FIGHT_SECONDS = 120;
const CLIENT_SECONDS = 360;
const OWNER_DRIVER_SECONDS = 400;
const SESSION_SECONDS = 600;
const PUBLIC_ROOM_SUFFIX = "-public";
if (target?.npc !== npc.corporeal_beast || target.profile.hitpoints !== FULL_BEAST_LIFE
    || room.corp.core.npc !== npc.corporeal_dark_energy_core
    || room.corp.core.profile.hitpoints !== FULL_CORE_LIFE
    || room.corp.core.maximumDamage !== NATIVE_CORE_MAXIMUM
    || room.corp.core.radiusTiles * HALVING_DENOMINATOR + ONE !== CORE_FOOTPRINT_TILES
    || room.corp.otherWeaponScale.numerator !== HALVING_NUMERATOR
    || room.corp.otherWeaponScale.denominator !== HALVING_DENOMINATOR)
  throw new Error("Generated native Corp identities, health, footprint or restriction differ");
const weapon = kit.equipment.find(row => row.type === "weapon" && row.equipment.kind === "melee-main")
  ?? kit.equipment.find(row => row.type === "weapon");
if (!weapon) throw new Error("Admitted ordinary melee kit has no physical weapon");
const weaponCategory = config.obj(weapon.item).params.get(param.weapon_category_ref) ?? null;
if (room.corp.fullDamageClasses.includes(weaponCategory) || room.corp.fullDamageWeapons.includes(weapon.item))
  throw new Error("Capture scoped to admitted non-spear T92 ordinary melee");
if (!target.profile.dropTable || !target.drops.some(row => !row.unavailable))
  throw new Error("Normal qualified boss drop owner/table is missing; absence is not a legal empty roll");
const lootPolicy = {selection: "observed-normal-owner-qualified-roll-or-legal-empty", fixedItem: null,
  coreAshes: "Only after an actual normally earned core death; never substitute for boss loot"};
const recordingScope = {identity: "native_ordinary_melee_sample", required: ["enter", "ordinary_melee_sample"], desired: ["leave", "rejoin"],
  narrowing: "Bounded ordinary melee sample starts at full health and observes normal incoming hits, half damage, prayer and food. Full kill, boss loot and core hard cases remain matching socket evidence. Exit/rejoin attempted only within the fixed remaining budget.",
  core: "Observe natural full-health core, actual drain/heal and naturally occurring pause/poison/death stop; do not force a random core.",
  excluded: ["native full kill and boss loot", "boss-death core stop", "private/custom/join/login/cap matrix", "full poison/pause/stale actor matrix", "unqualified presentation and prayer amount", "pixel and FPS proof"]};
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
  actors: [{npc: target.npc, role: "boss", maximumLife: target.profile.hitpoints},
    {npc: room.corp.core.npc, role: "core", maximumLife: room.corp.core.profile.hitpoints}],
  publicRoom: room.id + PUBLIC_ROOM_SUFFIX,
  corpseTicks: room.encounter.deathTicks,
  core: {...room.corp.core, profile: {hitpoints: room.corp.core.profile.hitpoints}},
  restriction: {fullDamageClasses: room.corp.fullDamageClasses, fullDamageWeapons: room.corp.fullDamageWeapons,
    scale: room.corp.otherWeaponScale, selectedWeapon: {item: weapon.item, category: weaponCategory, native: weapon.native, forms: weapon.forms.map(item => ({item, category: config.obj(item).params.get(param.weapon_category_ref) ?? null}))}},
  budgets: {fightSeconds: FIGHT_SECONDS, clientSeconds: CLIENT_SECONDS,
    driverSeconds: OWNER_DRIVER_SECONDS, sessionSeconds: SESSION_SECONDS},
  lootPolicy, recordingScope,
  target: {npc: target.npc, profile: {hitpoints: target.profile.hitpoints}},
  equipment: kit.loadout.equipment, loadout: kit.loadout,
  prayers, toolbar, food: kit.food,
  source: {gearChoice: choice, blockedResources: kit.blockedResources,
    initialSkillsAndSavedPreferenceOnly: true}};
fs.writeFileSync(path.join(work, "plan.json"), JSON.stringify(plan, null, ONE) + "\n", {flag: "wx"});
fs.writeFileSync(path.join(work, "initial-fixture.json"), JSON.stringify({
  accountPath, accountSha256: createHash("sha256").update(fs.readFileSync(accountPath)).digest("hex"),
  account, source: {seedSha256: createHash("sha256").update(fs.readFileSync(new URL(import.meta.url))).digest("hex")},
  scope: "Scratch initial fixture only; no simulation, account replacement or live assistance",
}, null, ONE) + "\n", {flag: "wx"});
