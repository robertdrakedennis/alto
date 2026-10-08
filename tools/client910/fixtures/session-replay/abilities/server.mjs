// Recording-only target life and fixed rolls keep all three basic requests
// in one ordinary session; no ability, cooldown or resource rule is replaced.
import { npc } from "@alto/domain";
import { World } from "../../../../../server/src/lostcity/engine/World.ts";
import CacheProvider from "../../../../../server/src/lostcity/server/CacheProvider.ts";
import { loadNpcData } from "../../../../../server/src/lostcity/data/NpcData.ts";
import { profileFromCache } from "../../../../../server/src/lostcity/systems/npc/NpcCacheProfile.ts";
const HALF_ROLL = 0.5;
const RECORD_TARGET_LIFE = 100_000;
const NO_OFFENSIVE_DAMAGE = 0;
const world = new World({ combatMode: "eoc" });
world.random = () => HALF_ROLL;
await world.start();
world.population.profiles = new Map(loadNpcData().profiles);
world.population.profiles.set(npc.chicken, {
  ...profileFromCache(
    CacheProvider.config.npc(npc.chicken),
    RECORD_TARGET_LIFE,
  ),
  aggressive: false,
  maxHit: NO_OFFENSIVE_DAMAGE,
});
