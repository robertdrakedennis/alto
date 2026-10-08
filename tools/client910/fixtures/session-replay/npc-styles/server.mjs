// Ordinary generated NPC profiles and world phases; only rolls are fixed for a bounded recording.
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import {loadNpcData} from '../../../../../server/src/lostcity/data/NpcData.ts';
const HALF_ROLL = 0.5;
const world = new World();
world.random = () => HALF_ROLL;
await world.start();
world.population.profiles = loadNpcData().profiles;
