// The ordinary world owns health, attacks, death and drops; only random draws are fixed.
import {npc} from '@alto/domain';
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
import {loadBossRooms} from '../../../../../server/src/lostcity/data/BossRoomData.ts';
const RECORDING_ROLL = 0.6;
const MAGIC_CHOICE = 0.9;
const NORMAL_FIRE_CHOICE = 0;
const UNSEEN_TICK = -1;
const world = new World(); world.random = () => RECORDING_ROLL;
await world.start();
const room = loadBossRooms()[NORMAL_FIRE_CHOICE];
const fixed = new WeakSet();
world.combat.listen({launched: (attacker, defender) => {
    const combatant = attacker.kind === 'npc' ? attacker : defender.kind === 'npc' ? defender : null;
    const boss = combatant?.entity;
    if (!boss || boss.type !== npc.king_black_dragon || fixed.has(boss)) return;
    fixed.add(boss);
    let lastTick = UNSEEN_TICK;
    // The first draw chooses magic over melee; its next draw selects ordinary fire.
    // Socket E2E covers the other breath variants. Every recorded hit still uses the real ratings.
    boss.combat.configureAttacks(room.encounter.attacks,null,()=> {
        if(lastTick !== world.tick) {lastTick = world.tick; return MAGIC_CHOICE;}
        return NORMAL_FIRE_CHOICE;
    });
}});
