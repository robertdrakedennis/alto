// Real content and generated profiles; fixed random seams bound the recording.
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import {loadNpcData} from '../../../../../server/src/lostcity/data/NpcData.ts';
const HALF_ROLL = 0.5;
const FAIL_ROLL = 0.999;
const SUCCESS_ROLL = 0;
const world = new World();
world.random = () => HALF_ROLL;
await world.start();
world.population.profiles = loadNpcData().profiles;
world.skills.thieving.random = () => FAIL_ROLL;
world.skills.runecrafting.abyss.random = () => SUCCESS_ROLL;
