import fs from 'node:fs';
import path from 'node:path';
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import {npc,obj} from '@alto/domain';
const FIRST_ARGUMENT=2;
const HALF_ROLL=0.5;
const FIRST_SLOT=0;
const SLAYER_SKILL=18;
const world=new World();world.random=()=>HALF_ROLL;
const output=path.join(process.argv[FIRST_ARGUMENT],'combat-receipts.jsonl');
const write=row=>fs.appendFileSync(output,JSON.stringify(row)+'\n');
world.combat.listen({launched:(source,target,style,delay,hit)=>write({kind:'launch',tick:world.tick,source:source.kind,targetId:target.id,targetDefinition:target.kind==='npc'?target.entity.type:null,style,delay,damage:hit.damage}),died:(target,source)=>write({kind:'death',tick:world.tick,targetId:target.id,targetDefinition:target.kind==='npc'?target.entity.type:null,sourceId:source?.id,hideTick:target.kind==='npc'?target.entity.life.deathHideTick:null})});
const prior=new Map();
for(const phase of ['npcs','players'])world.phases.on(phase,'Slayer Wilderness passive receipt',frame=>{
 const players=frame.players.filter(player=>player.liveClient);if(!players.length)return;
 const state={players:players.map(player=>({pid:player.pid,x:player.x,z:player.z,level:player.level,wilderness:world.wilderness.level(player),slayer:player.slayer.save(),slayerXp:player.stats.xp(SLAYER_SKILL),backpack:player.invs.backpack.slots,worn:player.invs.worn.slots,quick:player.slayer.save().unlocks.includes('quick-kills')})),enemies:[...world.npcs].filter(target=>target.type===npc.rock_slug).map(target=>({id:target.nid,generation:target.life.generation,hitpoints:target.stats.hitpoints,visible:target.life.visible,animation:target.events.animation,loot:target.life.loot})),ground:[...world.ground.zones()].flatMap(zone=>world.ground.zoneState(zone,players[FIRST_SLOT].pid)).map(event=>event.obj)};
 const raw=JSON.stringify(state);if(prior.get(phase)!==raw){write({kind:'state',phase,tick:frame.tick,...state});prior.set(phase,raw);}
});
await world.start();
