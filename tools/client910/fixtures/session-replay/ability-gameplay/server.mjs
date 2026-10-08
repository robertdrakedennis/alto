// Only fixture life, initial adrenaline and the random source are controlled.
// Every activation still enters through normal native packets and combat.
import { npc, obj, param, struct, varp } from "@alto/domain";
import { World } from "../../../../../server/src/lostcity/engine/World.ts";
import CacheProvider from "../../../../../server/src/lostcity/server/CacheProvider.ts";
import { loadNpcData } from "../../../../../server/src/lostcity/data/NpcData.ts";
import { profileFromCache } from "../../../../../server/src/lostcity/systems/npc/NpcCacheProfile.ts";
import { snapshotVarp } from "../../../../../server/src/lostcity/systems/equipment/EquipmentStats.ts";
import { MAIN_HAND_SLOT } from "../../../../../server/src/lostcity/systems/equipment/RangedEquipment.ts";
import { equipmentNumber } from "../../../../../server/src/lostcity/systems/equipment/EquipmentParams.ts";
const HALF_ROLL = 0.5;
const RECORD_TARGET_LIFE = 100_000;
const NO_OFFENSIVE_DAMAGE = 0;
const INITIAL_ADRENALINE_FINE = 1000;
const OFFHAND_DIVISOR = 2;
const WORN_ITEM_ID_INDEX = 0;
const ABSENT_CONFIG = -1;
const world = new World({ combatMode: "eoc" });
world.random = () => HALF_ROLL;
await world.start();
world.population.profiles = new Map(loadNpcData().profiles);
world.population.profiles.set(npc.ability_practice_dummy, {
  ...profileFromCache(
    CacheProvider.config.npc(npc.ability_practice_dummy),
    RECORD_TARGET_LIFE,
  ),
  aggressive: false,
  maxHit: NO_OFFENSIVE_DAMAGE,
});
const initialized = new WeakSet();
const before = new WeakMap();
const automatic = new WeakMap();
const abilityLaunched = new WeakSet();
world.phases.on("inbound", "recording eligibility snapshot", (frame) => {
  for (const player of frame.players) {
    if (!initialized.has(player) && player.combat.state.target) {
      // Seed once at first ordinary engagement so startup idle decay cannot erase fixture eligibility.
      player.abilities.gain(
        INITIAL_ADRENALINE_FINE - player.abilities.fine,
        false,
      );
      initialized.add(player);
    }
    before.set(player, player.abilities.fine);
  }
});
world.combat.listen({
  launched: (attacker, defender, _style, _delay, hit) => {
    if (attacker.kind !== "player") return;
    const player = attacker.entity;
    if (!hit.ability) {
      const weapon =
        player.invs.worn.slots[MAIN_HAND_SLOT]?.[WORN_ITEM_ID_INDEX] ??
        ABSENT_CONFIG;
      const request = player.look.animations.pending;
      if (weapon === obj.bronze_2h_sword && request) {
        const category = equipmentNumber(
          CacheProvider.config.obj(weapon),
          param.weapon_category_ref,
          ABSENT_CONFIG,
        );
        const expectedSequence = Number(
          CacheProvider.config
            .struct(category)
            .params.get(param.weapon_attack_sequence) ?? ABSENT_CONFIG,
        );
        automatic.set(player, {
          tick: world.tick,
          weapon,
          category,
          expectedSequence,
          revision: request.revision,
          modes: request.modes,
          delay: request.delay,
        });
      }
      return;
    }
    abilityLaunched.add(player);
    const weaponBase =
      snapshotVarp(
        player.equipment.statSnapshot,
        varp.mainhand_ability_damage,
        NO_OFFENSIVE_DAMAGE,
      ) +
      Math.floor(
        snapshotVarp(
          player.equipment.statSnapshot,
          varp.offhand_ability_damage,
          NO_OFFENSIVE_DAMAGE,
        ) / OFFHAND_DIVISOR,
      );
    console.log(
      "[ability-fixture] " +
        JSON.stringify({
          tick: world.tick,
          pid: player.pid,
          definition: hit.ability.definition,
          beforeFine: before.get(player),
          afterFine: player.abilities.fine,
          damage: hit.damage,
          accurate: hit.ability.accurate,
          critical: hit.ability.critical,
          controlled: defender.statuses?.frozen,
          targetLife: defender.lifeId,
          targetHitpoints: defender.entity.stats.hitpoints,
          weaponBase,
        }),
    );
  },
});

// Observe the ordinary impact owner after combat and before info encoding.
world.phases.on("players", "recording life publication", (frame) => {
  for (const player of frame.players) {
    if (!player.liveClient) continue;
    const active = player.combat.statuses.activeConditions.find(
      (effect) => effect.condition.definition === struct.anticipation_ability,
    );
    const request = player.look.animations.pending;
    const ordinary = automatic.get(player);
    // Before the first damaging ability, only an exact surviving non-ability request is an auto cue.
    const survivingAuto =
      !abilityLaunched.has(player) &&
      ordinary?.tick === frame.tick &&
      ordinary.revision === request?.revision
        ? ordinary
        : null;
    console.log(
      "[ability-player-fixture] " +
        JSON.stringify({
          tick: frame.tick,
          pid: player.pid,
          life: player.combat.lifeId,
          animation: request,
          automatic: survivingAuto,
          anticipation: active
            ? {
                generation: active.generation,
                appliedTick: active.appliedTick,
                untilTick: active.untilTick,
              }
            : null,
        }),
    );
  }
  for (const target of world.npcs) {
    if (target.type !== npc.ability_practice_dummy) continue;
    console.log(
      "[ability-life-fixture] " +
        JSON.stringify({
          tick: frame.tick,
          index: target.nid,
          definition: target.type,
          life: target.stats.hitpoints,
          maximum: target.stats.maxHitpoints,
          hits: target.events.hits,
          headbars: target.events.headbars,
        }),
    );
  }
});
