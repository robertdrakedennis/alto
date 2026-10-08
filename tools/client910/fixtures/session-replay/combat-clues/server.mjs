// Recording-only bounded life points shorten fights; ordinary spell ratings, World phases and socket consumers remain in use.
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
const HALF_ROLL = 0.5;
const FIXTURE_LIFE = 100;
const world = new World();
world.random = () => HALF_ROLL;
await world.start();
world.trails.load({...world.trails.data, combat: world.trails.data.combat.map(rule => ({...rule, profile: {...rule.profile, hitpoints: FIXTURE_LIFE}}))});
