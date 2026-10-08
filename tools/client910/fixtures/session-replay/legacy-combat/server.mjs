// Recording observers do not replace combat, profiles, damage, reward or resource owners.
import fs from 'node:fs';
import path from 'node:path';
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import {loadNpcData} from '../../../../../server/src/lostcity/data/NpcData.ts';
const HALF_ROLL = 0.5;
const COMBAT_SKILL_COUNT = 7;
const FIRST_ARGUMENT = 2;
const world = new World();
world.random = () => HALF_ROLL;
const output = path.join(process.argv[FIRST_ARGUMENT], 'combat-receipts.jsonl');
const write = value => fs.appendFileSync(output, `${JSON.stringify(value)}\n`);
world.combat.listen({
 launched: (source, target, style, delay, hit) => write({kind:'launch',tick:world.tick,source:source.kind,sourceId:source.id,target:target.kind,targetId:target.id,style,delay,damage:hit.damage,training:hit.training,weapon:source.kind==='player'?source.entity.invs.worn.slots:null}),
 died: (target, source) => write({kind:'death',tick:world.tick,targetId:target.id,targetDefinition:target.entity.type,sourceId:source?.id,hideTick:target.kind==='npc'?target.entity.life.deathHideTick:null}),
 retaliated: (target,source) => write({kind:'retaliation',tick:world.tick,targetId:target.id,sourceId:source.id})
});
const prior = new Map();
// Read-only steps append after the ordinary NPC population and player phases.
// They neither deliver hits nor change profiles, rewards, variables or resources.
for (const phase of ['npcs', 'players']) world.phases.on(phase, 'Legacy recording state', frame => {
    const players = frame.players.filter(player => player.liveClient).map(player => ({pid: player.pid, xp: Array.from({length: COMBAT_SKILL_COUNT}, (_, stat) => player.stats.xp(stat)), retaliates: player.combat.retaliates, worn: player.invs.worn.slots, backpack: player.invs.backpack.slots}));
    const enemies = Array.from(world.npcs, target => ({id: target.nid, definition: target.type, generation: target.life.generation, hitpoints: target.stats.hitpoints, visible: target.life.visible, hideTick: target.life.deathHideTick}));
    const state = JSON.stringify({players, enemies});
    if (prior.get(phase) !== state) {
        write({kind: 'state', phase, tick: frame.tick, players, enemies});
        prior.set(phase, state);
    }
});
await world.start();
world.population.profiles = loadNpcData().profiles;
