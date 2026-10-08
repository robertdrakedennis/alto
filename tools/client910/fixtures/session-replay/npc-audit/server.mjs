import fs from 'node:fs';
import path from 'node:path';
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import {npc} from '@alto/domain';
const FIRST_ARGUMENT = 2;
const FIRST_PLAYER = 0;
const HALF_ROLL = 0.5;
const SAMPLE_RADIUS = 5;
const COMBAT_SKILL_COUNT = 7;
const world = new World();
world.random = () => HALF_ROLL;
const output = path.join(process.argv[FIRST_ARGUMENT], 'combat-receipts.jsonl');
const write = row => fs.appendFileSync(output, JSON.stringify(row)+'\n');
world.combat.listen({
 launched:(source,target,style,delay,hit)=>write({kind:'launch',tick:world.tick,source:source.kind,sourceId:source.id,target:target.kind,targetId:target.id,targetDefinition:target.kind==='npc'?target.entity.type:null,style,delay,damage:hit.damage}),
 died:(target,source)=>write({kind:'death',tick:world.tick,targetId:target.id,targetDefinition:target.kind==='npc'?target.entity.type:null,sourceId:source?.id,hideTick:target.kind==='npc'?target.entity.life.deathHideTick:null}),
});
const prior = new Map();
for(const phase of ['npcs','players']) world.phases.on(phase,'NPC audit passive receipt', frame=>{
 const players=frame.players.filter(player=>player.liveClient);
 if(!players.length)return;
 const near = target=>players.some(player=>player.level===target.level&&Math.abs(player.x-target.x)<=SAMPLE_RADIUS&&Math.abs(player.z-target.z)<=SAMPLE_RADIUS);
 const enemies=Array.from(world.npcs).filter(target=>[npc.chicken,npc.goblin_level_2].includes(target.type)&&near(target)).map(target=>({id:target.nid,definition:target.type,generation:target.life.generation,x:target.x,z:target.z,hitpoints:target.stats.hitpoints,visible:target.life.visible,hideTick:target.life.deathHideTick,animation:target.events.animation,configuredDeathTicks:target.stats.profile?.deathTicks,loot:target.life.loot}));
 const playerRows=players.map(player=>({pid:player.pid,x:player.x,z:player.z,xp:Array.from({length:COMBAT_SKILL_COUNT},(_,stat)=>player.stats.xp(stat)),backpack:player.invs.backpack.slots,worn:player.invs.worn.slots}));
 const ground=[...world.ground.zones()].flatMap(zone=>world.ground.zoneState(zone,players[FIRST_PLAYER].pid)).map(event=>event.obj).filter(near);
 const state=JSON.stringify({players:playerRows,enemies,ground});
 if(prior.get(phase)!==state){write({kind:'state',phase,tick:frame.tick,...JSON.parse(state)});prior.set(phase,state);}
});
await world.start();
