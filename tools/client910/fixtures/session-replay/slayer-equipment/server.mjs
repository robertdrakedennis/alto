import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {varbit} from '@alto/domain';
import CacheProvider from '../../../../../server/src/lostcity/server/CacheProvider.ts';
import {World} from '../../../../../server/src/lostcity/engine/World.ts';
const FIRST_ARGUMENT = 2;
const SLAYER_SKILL = 18;
const HALF_ROLL = 0.5;
const work = process.argv[FIRST_ARGUMENT];
const output = path.join(work, 'equipment-receipts.jsonl');
const world = new World();
world.random = () => HALF_ROLL;
let previous = '';
world.phases.on('players', 'Slayer equipment passive receipt', frame => {
    const players = frame.players.filter(player => player.liveClient).map(player => {
        const accountFile = path.join(work, 'players', 'accounts', createHash('sha256').update(player.accountKey).digest('hex')+'.json');
        return {legacyCombatActive:player.vars.varbit(CacheProvider.config.varbit(varbit.legacy_combat_active)), legacyInterfaceLayout:player.vars.varbit(CacheProvider.config.varbit(varbit.legacy_interface_mode)), slimHeaders:player.vars.varbit(CacheProvider.config.varbit(varbit.slim_window_headers)), pid:player.pid, x:player.x, z:player.z, level:player.level, slayer:player.slayer.save(), slayerXp:player.stats.xp(SLAYER_SKILL), backpack:player.invs.backpack.slots, worn:player.invs.worn.slots, saved:JSON.parse(fs.readFileSync(accountFile, 'utf8'))};
    });
    if (!players.length) return;
    const raw = JSON.stringify(players);
    if (raw !== previous) {
        fs.appendFileSync(output, JSON.stringify({kind:'state', tick:frame.tick, players})+'\n');
        previous = raw;
    }
});
await world.start();
