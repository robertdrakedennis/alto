// Append-only observation around ordinary World owners; no simulation writes.
import fs from "node:fs";
import path from "node:path";
import {pathToFileURL} from "node:url";
import {observeFoodCommits} from "../barrows/food-receipts.mjs";

const FIRST_ARGUMENT = 2;
const ONE = 1;
const NEARBY_TILES = 104;
const MAX_GROUND_ROWS = 4096;
const MAX_ERRORS = 64;
const COMBAT_SKILL_COUNT = 7;
const root = process.env.ALTO_BOSS_RUNTIME_ROOT;
const work = process.argv[FIRST_ARGUMENT];
if (!root || !path.isAbsolute(root) || !work || !path.isAbsolute(work))
  throw new Error("Explicit own World and scratch required");
const owner = relative => import(pathToFileURL(path.join(root, relative)).href);
const [{World}, {regenerationStep}, {default: Consumables}, {varp}, {default: NpcStats}] = await Promise.all([
  owner("server/src/lostcity/engine/World.ts"),
  owner("server/src/lostcity/systems/stats/Regeneration.ts"),
  owner("server/src/lostcity/systems/consumables/Consumables.ts"),
  owner("packages/domain/src/index.ts"),
  owner("server/src/lostcity/systems/npc/NpcStats.ts"),
]);
const plan = JSON.parse(fs.readFileSync(path.join(work, "plan.json")));
const definitions = new Set(plan.actors.map(row => row.npc));
const world = new World();
const output = path.join(work, "combat-receipts.jsonl");
const errors = [];
function observe(task) {
  try { task(); }
  catch (error) {
    if (errors.length < MAX_ERRORS) errors.push({tick: world.tick, reason: String(error)});
    fs.writeFileSync(path.join(work, "passive-observer-errors.json"), JSON.stringify(errors));
  }
}
const write = row => fs.appendFileSync(output, JSON.stringify(row) + "\n");
const food = observeFoodCommits(Consumables, plan.accountKey, write);
const belongs = actor => actor?.kind === "player" && actor.entity.accountKey === plan.accountKey;
const actorTokens = new WeakMap();
let nextActorToken = ONE;
function token(entity) {
  if (!actorTokens.has(entity)) actorTokens.set(entity, nextActorToken++);
  return actorTokens.get(entity);
}
const actor = value => value == null ? null : {
  actorToken: token(value.entity),
  kind: value.kind, id: value.id, generation: value.lifeId,
  definition: value.kind === "npc" ? value.entity.type : null,
  x: value.x, z: value.z, level: value.level, currentLife: value.currentLife,
  maximumLife: value.kind === "npc" ? value.entity.stats.profile?.hitpoints : value.entity.stats.resources?.maxLife,
  instance: value.entity.instanceSession?.id ?? null,
  poisoned: value.kind === "npc" ? value.entity.combat.statuses.poisoned : null,
};
world.combat.listen({
  launched: (source, target, style, delay, hit) => observe(() => {
    if (belongs(source) || belongs(target)) write({kind: "launch", tick: world.tick,
      source: actor(source), target: actor(target), style, delay, damage: hit.damage, weapon: hit.weapon ?? null});
  }),
  landed: (target, hit, result) => observe(() => {
    if (belongs(hit.attacker) || belongs(target)) {
      const native = target.events.hits.at(-ONE);
      if (!native) throw new Error("Ordinary landed impact has no native hitsplat");
      write({kind: "landed", tick: world.tick,
        source: actor(hit.attacker), target: actor(target), style: hit.style,
        launchedDamage: hit.damage, nativeDamage: native.damage, weapon: hit.weapon ?? null, ...result});
    }
  }),
  died: (target, source) => observe(() => {
    if (belongs(source) || belongs(target)) write({kind: "death", tick: world.tick,
      source: actor(source), target: actor(target)});
  }),
});
world.population.onKilled((enemy, player) => observe(() => {
  if (definitions.has(enemy.type)) write({kind: "reward_owner", tick: world.tick,
    target: actor(enemy.combat), owner: actor(player?.combat), contributors: enemy.combat.damageLog.ranked(), lootRollObserved: true,
    allocations: enemy.life.loot ? [enemy.life.loot, ...(enemy.life.loot.allocations ?? [])]
      .map(allocation => ({owner: allocation.owner, drops: allocation.drops})) : [],
    qualification: "Final normal RNG/admitted pending loot, including a legal empty; read after population assigned it"});
}));
// Observe the original synchronous restore exactly once. No amounts, resource
// balances, arguments or return values are replaced by this instrumentation.
const originalRestoreLife = NpcStats.prototype.restoreLife;
NpcStats.prototype.restoreLife = function (...args) {
  const beforeLife = this.hitpoints;
  const result = originalRestoreLife.apply(this, args);
  observe(() => {
    const owners = [...world.npcs].filter(enemy => enemy.stats === this && enemy.type === plan.target.npc);
    if (owners.length > ONE) throw new Error("Ambiguous NPC healing owner");
    if (owners.length === ONE) write({kind: "npc_healing", tick: world.tick,
      target: actor(owners[0].combat), requested: args[0], beforeLife,
      afterLife: this.hitpoints, acceptedHealing: this.hitpoints - beforeLife,
      qualification: "After the original normal NpcStats.restoreLife; no healing was introduced by this observer"});
  });
  return result;
};
const tokens = new WeakMap();
let nextToken = ONE;
const near = (left, right) => left.level === right.level
  && Math.max(Math.abs(left.x - right.x), Math.abs(left.z - right.z)) <= NEARBY_TILES;
const prior = new Map();
for (const phase of ["npcs", "players"]) world.phases.on(phase, "Passive encounter recording", frame => observe(() => {
  const players = frame.players.filter(row => row.liveClient && row.accountKey === plan.accountKey);
  if (!players.length) return;
  const enemies = [...world.npcs].filter(row => definitions.has(row.type) && players.some(player => near(player, row)))
    .map(row => ({...actor(row.combat), hitpoints: row.stats.hitpoints, visible: row.life.visible,
      hideTick: row.life.deathHideTick, animation: row.events.animation}));
  const playerRows = players.map(player => {
    const ground = [];
    for (const zone of world.ground.zones()) for (const event of world.ground.zoneState(zone, player.pid)) {
      if (event.kind !== "add" || !near(player, event.obj)) continue;
      if (ground.length >= MAX_GROUND_ROWS) throw new Error("Passive ground observation exceeds bound");
      const item = event.obj;
      if (!tokens.has(item)) tokens.set(item, nextToken++);
      ground.push({token: tokens.get(item), item: item.id, count: item.count,
        x: item.x, z: item.z, level: item.level, owner: item.owner, instance: item.instance ?? null});
    }
    return {pid: player.pid, generation: player.combat.lifeId, actorToken: token(player), x: player.x, z: player.z, level: player.level,
      life: player.stats.resources?.life, maximumLife: player.stats.resources?.maxLife,
      prayerFine: player.stats.resources?.prayerFine,
      regeneration: player.stats.resources ? regenerationStep(player.stats.resources.maxLife) : null,
      xp: Array.from({length: COMBAT_SKILL_COUNT}, (_, stat) => player.stats.xp(stat)),
      backpack: player.invs.backpack.slots, worn: player.invs.worn.slots, inventoryRevision: player.invs.revision,
      savedVarps: player.vars.savedVarps(), instance: player.instanceSession?.id ?? null,
      room: player.instanceSession?.kind.id ?? null,
      membership: player.instanceSession?.members.get(player.accountKey) === player,
      templateTile: player.instanceSession?.toTemplate({level: player.level, x: player.x, z: player.z}) ?? null,
      target: player.combat.state.target === null ? null : {
        kind: player.combat.state.target.kind,
        id: player.combat.state.target.reference.id,
        generation: player.combat.state.target.reference.generation},
      autoRetaliateDisabled: player.vars.number(varp.auto_retaliate_disabled), ground};
  });
  const current = JSON.stringify({players: playerRows, enemies});
  // Per-phase passive rows keep a finite consecutive-tick denominator for any
  // naturally observed pause/poison window; 1500 seconds remains below the
  // inherited 20,000 receipt bound without hidden state queries or mutations.
  if (prior.get(phase) !== current || enemies.some(enemy => enemy.definition === plan.core.npc)) {
    write({kind: "state", phase, tick: frame.tick, ...JSON.parse(current)});
    prior.set(phase, current);
  }
  for (const error of food.errors) throw new Error(error.reason);
  food.errors.length = 0;
}));

// After normal zones sync and before the normal flush/info step. Observe the
// actual event queues once per tick; no packet, amount or scheduling is changed.
world.phases.on("zones", "Passive native Corp publication", frame => observe(() => {
  const players = frame.players.filter(player => player.liveClient && player.accountKey === plan.accountKey);
  if (players.length > ONE) throw new Error("Ambiguous ordinary publication owner");
  if (!players.length) return;
  const player = players[0];
  const publication = entity => ({...actor(entity.combat),
    hitpoints: entity.combat.currentLife,
    hits: entity.events.hits.map(hit => ({...hit})),
    headbars: entity.events.headbars.map(bar => ({...bar})),
  });
  const enemies = [...world.npcs].filter(enemy => definitions.has(enemy.type) && near(player, enemy))
    .map(enemy => ({...publication(enemy), visible: enemy.life.visible}));
  write({kind: "native_publication", tick: frame.tick, player: publication(player), enemies,
    qualification: "Read normal queues before flush; received PLAYER/NPC roster still determines which events reached the native client"});
}));

await world.start();
