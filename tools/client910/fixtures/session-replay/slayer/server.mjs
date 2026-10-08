// Recording-only Bat life points bound the capture; cache attack/defence,
// animations, normal combat deaths, assignment counts and XP remain real.
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import {profileFromCache} from '../../../../../server/src/lostcity/systems/npc/NpcCacheProfile.ts';
import {loadNpcData} from '../../../../../server/src/lostcity/data/NpcData.ts';
import {loadSlayerData} from '../../../../../server/src/lostcity/data/SlayerData.ts';
const LOWEST_ROLL=0;
const HALF_ROLL=0.5;
const RECORDING_LIFE_POINTS=100;
const world=new World();
world.random=()=>[...world.players].some(player=>player.slayer.assignment)?HALF_ROLL:LOWEST_ROLL;
await world.start();
world.population.profiles=new Map(loadNpcData().profiles);
for(const rule of loadSlayerData().monsters) {
 const type=CacheProvider.config.npc(rule.npc);
 if(type.name==='Bat') world.population.profiles.set(type.id,{...(world.population.profiles.get(type.id)??profileFromCache(type,RECORDING_LIFE_POINTS)),hitpoints:RECORDING_LIFE_POINTS});
}
world.population.onKilled(npc=>{if(CacheProvider.config.npc(npc.type).name==='Bat') npc.life.summoned=true;});
