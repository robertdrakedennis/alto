// Recording-only deterministic ratings isolate the prayer reduction. The
// ordinary World, combat engine, cache presentation and socket loop run unchanged.
import {npc} from '@alto/domain';
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import {profileFromCache} from '../../../../../server/src/lostcity/systems/npc/NpcCacheProfile.ts';
import {loadNpcData} from '../../../../../server/src/lostcity/data/NpcData.ts';
const HALF_ROLL = 0.5;
const TEST_HITPOINTS = 10_000;
const TEST_MAX_HIT = 400;
const CERTAIN_ACCURACY = 100_000;
const world = new World();
world.random = () => HALF_ROLL;
await world.start();
world.population.profiles = new Map(loadNpcData().profiles);
world.population.profiles.set(npc.goblin_level_2, {...profileFromCache(CacheProvider.config.npc(npc.goblin_level_2),TEST_HITPOINTS), aggressive:false, maxHit:TEST_MAX_HIT, accuracy:CERTAIN_ACCURACY});
