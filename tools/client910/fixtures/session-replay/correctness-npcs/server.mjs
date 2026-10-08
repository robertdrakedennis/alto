import {World} from '../../../../../server/src/lostcity/engine/World.ts';
const HALF_ROLL = 0.5;
const world = new World();
world.random = () => HALF_ROLL;
await world.start();
