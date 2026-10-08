// Passive observations: ordinary world, actual generated NPC profiles and fixed random roll.
import fs from "node:fs";
import path from "node:path";
import { varbit, varp } from "@alto/domain";
import { World } from "../../../../../server/src/lostcity/engine/World.ts";
import { loadNpcData } from "../../../../../server/src/lostcity/data/NpcData.ts";
import CacheProvider from "../../../../../server/src/lostcity/server/CacheProvider.ts";
const FIRST_ARGUMENT = 2;
const HALF_ROLL = 0.5;
const world = new World();
world.random = () => HALF_ROLL;
const output = path.join(
  process.argv[FIRST_ARGUMENT],
  "special-receipts.jsonl",
);
const write = (value) =>
  fs.appendFileSync(output, `${JSON.stringify(value)}\n`);
world.combat.listen({
  launched: (source, target, style, delay, hit) =>
    write({
      kind: "launch",
      tick: world.tick,
      source: source.kind,
      sourceId: source.id,
      target: target.kind,
      targetId: target.id,
      style,
      delay,
      damage: hit.damage,
      special: hit.ability,
      sourceEnergy:
        source.kind === "player"
          ? source.entity.combat.specials.energy.fine
          : null,
    }),
  arrived: (target, hit) =>
    write({
      kind: "arrival",
      tick: world.tick,
      targetId: target.id,
      damage: hit.damage,
      special: hit.ability,
    }),
  died: (target, source) =>
    write({
      kind: "death",
      tick: world.tick,
      targetId: target.id,
      sourceId: source?.id,
    }),
});
const prior = new Map();
for (const phase of ["npcs", "players"])
  world.phases.on(phase, "Special recording state", (frame) => {
    const players = frame.players
      .filter((player) => player.liveClient)
      .map((player) => ({
        pid: player.pid,
        energy: player.combat.specials.energy.fine,
        armed: player.combat.specials.armed,
        energyFeed: player.vars.number(varp.adrenaline_fine),
        armedFeed: player.vars.number(varp.special_attack_armed),
        selectedClient: player.vars.serverVarcs.values.get(
          CacheProvider.config.varbit(varbit.legacy_selected_window).basevarId,
        ),
        worn: player.invs.worn.slots,
        backpack: player.invs.backpack.slots,
      }));
    const enemies = Array.from(world.npcs, (target) => ({
      id: target.nid,
      definition: target.type,
      generation: target.life.generation,
      hitpoints: target.stats.hitpoints,
      visible: target.life.visible,
    }));
    const state = JSON.stringify({ players, enemies });
    if (prior.get(phase) !== state) {
      write({ kind: "state", phase, tick: frame.tick, players, enemies });
      prior.set(phase, state);
    }
  });
await world.start();
world.population.profiles = loadNpcData().profiles;
