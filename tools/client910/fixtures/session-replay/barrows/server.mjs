// Private composition of the existing Barrows passive recorder, no action loop.
import fs from "node:fs";
import path from "node:path";
import {pathToFileURL} from "node:url";
import {observeFoodCommits} from "./food-receipts.mjs";
import {observeRepairCommits} from "./repair-receipts.mjs";
import {createHash} from "node:crypto";

const FIRST_ARGUMENT = 2;
const COMBAT_SKILL_COUNT = 7;
const runtimeRoot = process.env.ALTO_BARROWS_RUNTIME_ROOT;
if (!runtimeRoot || !path.isAbsolute(runtimeRoot)) {
  throw new Error("An explicit ordinary runtime checkout is required");
}
const owner = relative => import(pathToFileURL(path.join(runtimeRoot, relative)).href);
const [{World}, {loadBarrows}, {regenerationStep}, {default: CacheProvider},
       {default: Consumables}, {default: PlayerItemCharges}, {iface, varbit, varp}] = await Promise.all([
  owner("server/src/lostcity/engine/World.ts"),
  owner("server/src/lostcity/data/BarrowsData.ts"),
  owner("server/src/lostcity/systems/stats/Regeneration.ts"),
  owner("server/src/lostcity/server/CacheProvider.ts"),
  owner("server/src/lostcity/systems/consumables/Consumables.ts"),
  owner("server/src/lostcity/systems/equipment/PlayerItemCharges.ts"),
  owner("packages/domain/src/index.ts"),
]);
const world = new World();
const work = process.argv[FIRST_ARGUMENT];
const plan = JSON.parse(fs.readFileSync(path.join(work, "plan.json"), "utf8"));
// Ordinary World randomness is retained; the driver observes actual choices.
const output = path.join(work, "combat-receipts.jsonl");
const write = row => fs.appendFileSync(output, JSON.stringify(row) + "\n");
const foodObserver = observeFoodCommits(Consumables, plan.accountKey, write);
const accountPath = path.join(work, "players/accounts",
  createHash("sha256").update(plan.accountKey).digest("hex") + ".json");
const repairObserver = observeRepairCommits(PlayerItemCharges, plan.accountKey, accountPath, write, () => world.tick);
world.combat.listen({
  launched: (source, target, style, delay, hit) => write({
    kind: "launch", tick: world.tick, source: source.kind, sourceId: source.id,
    target: target.kind, targetId: target.id,
    targetDefinition: target.kind === "npc" ? target.entity.type : null,
    style, delay, damage: hit.damage,
  }),
  died: (target, source) => write({
    kind: "death", tick: world.tick, target: target.kind, targetId: target.id,
    targetDefinition: target.kind === "npc" ? target.entity.type : null,
    sourceId: source?.id,
    hideTick: target.kind === "npc" ? target.entity.life.deathHideTick : null,
    generation: target.kind === "npc" ? target.entity.life.generation : null,
    instance: target.kind === "npc" ? target.entity.instanceSession?.id ?? null : null,
    animation: target.kind === "npc" ? target.entity.events.animation : null,
  }),
});
const prior = new Map();
for (const phase of ["npcs", "players"]) {
  world.phases.on(phase, "Barrows passive ordinary recording receipt", frame => {
    const players = frame.players.filter(player => player.liveClient && player.accountKey === plan.accountKey);
    if (!players.length) return;
    const rules = loadBarrows();
    const definitions = new Set([...rules.crypts.map(crypt => crypt.profile.npc),
      ...rules.creatures.map(creature => creature.profile.npc), plan.repair.wear.spawn.npc]);
    const enemies = [...world.npcs].filter(target => definitions.has(target.type) &&
      players.some(player => target.instanceSession === player.instanceSession &&
        (player.instanceSession !== null || target.type === plan.repair.wear.spawn.npc)))
      .map(target => ({
        id: target.nid, definition: target.type, generation: target.life.generation,
        x: target.x, z: target.z, level: target.level, hitpoints: target.stats.hitpoints,
        maximumHitpoints: target.stats.profile?.hitpoints ?? null,
        visible: target.life.visible, hideTick: target.life.deathHideTick,
        animation: target.events.animation, configuredDeathTicks: target.stats.profile?.deathTicks,
        instance: target.instanceSession?.id ?? null,
      }));
    const playerRows = players.map(player => ({
      pid: player.pid, x: player.x, z: player.z, level: player.level,
      life: player.stats.resources?.life, maximumLife: player.stats.resources?.maxLife,
      prayerFine: player.stats.resources?.prayerFine, maximumPrayerFine: player.stats.resources?.maxPrayerFine,
      regeneration: player.stats.resources ? regenerationStep(player.stats.resources.maxLife) : null,
      xp: Array.from({length: COMBAT_SKILL_COUNT}, (_, stat) => player.stats.xp(stat)),
      backpack: player.invs.backpack.slots, worn: player.invs.worn.slots, coins: player.money.coins,
      house: player.house.save(), smithing: player.stats.level(rules.repairStand.smithingStat),
      accountRevision: player.invs.revision,
      instance: player.instanceSession?.id ?? null, run: player.barrowsRun?.snapshot() ?? null,
      target: player.combat.state.target?.kind ?? null,
      autoRetaliateDisabled: player.vars.number(varp.auto_retaliate_disabled),
      slain: rules.crypts.map(crypt => ({brother: crypt.brother,
        value: player.vars.varbit(CacheProvider.config.varbit(crypt.slainBit))})),
      totalKills: player.vars.varbit(CacheProvider.config.varbit(varbit.barrows_total_kills)),
      rewardOpened: player.vars.varbit(CacheProvider.config.varbit(varbit.barrows_reward_opened)),
      puzzleOpen: player.ui.isOpen(iface.barrows_pattern_choices),
      rewardsOpen: player.ui.isOpen(iface.loot_claim_window),
    }));
    const state = JSON.stringify({players: playerRows, enemies});
    if (prior.get(phase) !== state) {
      write({kind: "state", phase, tick: frame.tick, ...JSON.parse(state)});
      prior.set(phase, state);
    }
    if (foodObserver.errors.length || repairObserver.errors.length) {
      fs.writeFileSync(path.join(work, "passive-observer-errors.json"), JSON.stringify([...foodObserver.errors, ...repairObserver.errors]));
    }
  });
}
await world.start();
