// Passive receipts around the ordinary GWD world. No game-state or RNG writes.
import fs from "node:fs";
import path from "node:path";
import {pathToFileURL} from "node:url";
import {observeFoodCommits} from "../barrows/food-receipts.mjs";

const FIRST_ARGUMENT = 2;
const NEXT_OBSERVATION = 1;
const OBSERVATION_RADIUS = 104;
const MAX_GROUND_ROWS = 4096;
const MAX_OBSERVER_ERRORS = 64;
const COMBAT_SKILL_COUNT = 7;
const runtimeRoot = process.env.ALTO_GWD_RUNTIME_ROOT;
const work = process.argv[FIRST_ARGUMENT];
if (!runtimeRoot || !path.isAbsolute(runtimeRoot) || !work || !path.isAbsolute(work))
  throw new Error("Explicit owned GWD runtime and fresh scratch required");
const owner = relative => import(pathToFileURL(path.join(runtimeRoot, relative)).href);
const [{World}, {loadGwd}, {regenerationStep}, {default: CacheProvider},
       {default: Consumables}, {varp}] = await Promise.all([
  owner("server/src/lostcity/engine/World.ts"),
  owner("server/src/lostcity/data/GwdData.ts"),
  owner("server/src/lostcity/systems/stats/Regeneration.ts"),
  owner("server/src/lostcity/server/CacheProvider.ts"),
  owner("server/src/lostcity/systems/consumables/Consumables.ts"),
  owner("packages/domain/src/index.ts"),
]);
const plan = JSON.parse(fs.readFileSync(path.join(work, "plan.json"), "utf8"));
const rules = loadGwd();
const factions = new Map(rules.factionNpcs.map(row => [row.npc, row.faction]));
const roles = new Map(rules.arenas.flatMap(arena => arena.roles.map(role => [role.type, role])));
const world = new World();
// Keep World.random and every simulation/resource owner unchanged.
const output = path.join(work, "combat-receipts.jsonl");
const errors = [];
const errorPath = path.join(work, "passive-observer-errors.json");
const error = caught => {
  if (errors.length < MAX_OBSERVER_ERRORS) errors.push({tick: world.tick, reason: String(caught)});
  try { fs.writeFileSync(errorPath, JSON.stringify(errors)); }
  catch (failure) { console.error("GWD_OBSERVER_ERROR", String(failure)); }
};
const write = row => {
  try { fs.appendFileSync(output, JSON.stringify(row) + "\n"); }
  catch (caught) { error(caught); }
};
const observe = task => {
  try { task(); }
  catch (caught) { error(caught); }
};
const food = observeFoodCommits(Consumables, plan.accountKey, write);
const belongs = actor => actor?.kind === "player" && actor.entity.accountKey === plan.accountKey;
const targetRow = target => target === null || target === undefined ? null : {
  kind: target.kind, id: target.reference.id, generation: target.reference.generation,
};
const actorRow = actor => actor === null || actor === undefined ? null : {
  kind: actor.kind, id: actor.id, generation: actor.lifeId,
  definition: actor.kind === "npc" ? actor.entity.type : null,
  x: actor.x, z: actor.z, level: actor.level,
  currentLife: actor.currentLife,
  maximumLife: actor.kind === "npc" ? actor.entity.stats.profile?.hitpoints : actor.entity.stats.resources?.maxLife,
  instance: actor.entity.instanceSession?.id ?? null,
};
world.combat.listen({
  launched: (source, target, style, delay, hit) => observe(() => {
    if (!belongs(source) && !belongs(target)) return;
    write({kind: "launch", tick: world.tick, source: actorRow(source), target: actorRow(target),
      style, delay, damage: hit.damage, incomingFacts: hit.incomingFacts ?? null,
      npcPresentation: hit.npcPresentation ?? null});
  }),
  landed: (target, hit, result) => observe(() => {
    if (!belongs(hit.attacker) && !belongs(target)) return;
    write({kind: "landed", tick: world.tick, source: actorRow(hit.attacker), target: actorRow(target),
      style: hit.style, launchedDamage: hit.damage, ...result});
  }),
  died: (target, source) => observe(() => {
    if (!belongs(source) && !belongs(target)) return;
    write({kind: "death", tick: world.tick, source: actorRow(source), target: actorRow(target),
      hideTick: target.kind === "npc" ? target.entity.life.deathHideTick : null});
  }),
});
world.population.onKilled((npc, owner) => observe(() => {
  if (!factions.has(npc.type)) return;
  write({kind: "gwd_reward_owner", tick: world.tick, target: actorRow(npc.combat),
    owner: actorRow(owner?.combat), damageContributors: npc.combat.damageLog.ranked()});
}));
const groundTokens = new WeakMap();
let nextGroundToken = NEXT_OBSERVATION;
const groundRow = item => {
  if (!groundTokens.has(item)) groundTokens.set(item, nextGroundToken++);
  return {token: groundTokens.get(item), item: item.id, count: item.count, x: item.x, z: item.z,
    level: item.level, owner: item.owner, revealAt: item.revealAt, despawnAt: item.despawnAt,
    instance: item.instance ?? null};
};
const near = (left, right) => left.level === right.level &&
  Math.max(Math.abs(left.x - right.x), Math.abs(left.z - right.z)) <= OBSERVATION_RADIUS;
const prior = new Map();
for (const phase of ["npcs", "players"]) {
  // Append after the ordinary phase steps. Observation never drains queued zone events.
  world.phases.on(phase, "GWD passive ordinary recording receipt", frame => observe(() => {
    const players = frame.players.filter(player => player.liveClient && player.accountKey === plan.accountKey);
    if (!players.length) return;
    const enemies = [...world.npcs].filter(npc => factions.has(npc.type) && players.some(player => near(player, npc)))
      .map(npc => ({...actorRow(npc.combat), faction: factions.get(npc.type),
        configuredRole: roles.has(npc.type), hitpoints: npc.stats.hitpoints,
        maximumLife: npc.stats.profile?.hitpoints, visible: npc.life.visible,
        hideTick: npc.life.deathHideTick, animation: npc.events.animation,
        configuredDeathTicks: npc.stats.profile?.deathTicks,
        target: targetRow(npc.combat.state.target), damageContributors: npc.combat.damageLog.ranked()}));
    const playerRows = players.map(player => {
      const ground = [];
      for (const zone of world.ground.zones()) for (const event of world.ground.zoneState(zone, player.pid)) {
        if (event.kind !== "add" || !near(player, event.obj)) continue;
        if (ground.length >= MAX_GROUND_ROWS) throw new Error("Passive visible ground exceeds finite bound");
        ground.push(groundRow(event.obj));
      }
      return {pid: player.pid, generation: player.combat.lifeId, x: player.x, z: player.z, level: player.level,
        life: player.stats.resources?.life, maximumLife: player.stats.resources?.maxLife,
        prayerFine: player.stats.resources?.prayerFine, maximumPrayerFine: player.stats.resources?.maxPrayerFine,
        prayerAllowanceFine: player.stats.resources?.prayerOverboostFine,
        regeneration: player.stats.resources ? regenerationStep(player.stats.resources.maxLife) : null,
        xp: Array.from({length: COMBAT_SKILL_COUNT}, (_, stat) => player.stats.xp(stat)),
        backpack: player.invs.backpack.slots, worn: player.invs.worn.slots,
        inventoryRevision: player.invs.revision, savedVarps: player.vars.savedVarps(),
        run: player.move.energy.state.save(),
        instance: player.instanceSession?.id ?? null, room: player.instanceSession?.kind.id ?? null,
        membership: player.instanceSession?.members.get(player.accountKey) === player,
        target: targetRow(player.combat.state.target), autoRetaliateDisabled: player.vars.number(varp.auto_retaliate_disabled),
        counts: rules.counts.map(row => ({faction: row.faction,
          value: player.vars.varbit(CacheProvider.config.varbit(row.varbit))})), ground};
    });
    const state = JSON.stringify({players: playerRows, enemies});
    if (prior.get(phase) !== state) {
      write({kind: "state", phase, tick: frame.tick, ...JSON.parse(state)});
      prior.set(phase, state);
    }
    for (const row of food.errors) error(row.reason);
    food.errors.length = 0;
  }));
}
await world.start();
