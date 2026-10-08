// Passive observation of the ordinary World only: no fixture assistance after login.
import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';

const FIRST_ARGUMENT = 2;
const OBSERVATION_RADIUS = 104;
const MAX_OBSERVER_ERRORS = 64;
const FIRST = 0;
const ONE = 1;
const root = process.env.ALTO_BOSS_RUNTIME_ROOT;
const work = process.argv[FIRST_ARGUMENT];
if (!root || !path.isAbsolute(root) || !work || !path.isAbsolute(work)) throw new Error('Owned runtime and fresh scratch required');
const owner = relative => import(pathToFileURL(path.join(root, relative)).href);
const [{World}, {regenerationStep}, {default: Consumables}, {SLAYER_SKILL}, {observeFoodCommits}] = await Promise.all([
    owner('server/src/lostcity/engine/World.ts'),
    owner('server/src/lostcity/systems/stats/Regeneration.ts'),
    owner('server/src/lostcity/systems/consumables/Consumables.ts'),
    owner('server/src/lostcity/systems/slayer/PlayerSlayer.ts'),
    owner('tools/client910/fixtures/session-replay/barrows/food-receipts.mjs')
]);
const plan = JSON.parse(fs.readFileSync(path.join(work, 'plan.json'), 'utf8'));
const accountPath = path.join(work, 'players/accounts', createHash('sha256').update(plan.accountKey).digest('hex') + '.json');
const world = new World();
const write = row => fs.appendFileSync(path.join(work, 'combat-receipts.jsonl'), JSON.stringify(row) + '\n');
const errors = [];
const observe = callback => {
    try { callback(); }
    catch (caught) {
        if (errors.length < MAX_OBSERVER_ERRORS) errors.push(String(caught));
        fs.writeFileSync(path.join(work, 'passive-observer-errors.json'), JSON.stringify(errors));
    }
};
const food = observeFoodCommits(Consumables, plan.accountKey, write);
const actor = value => value === null || value === undefined ? null : {
    kind: value.kind, id: value.id, generation: value.lifeId,
    definition: value.kind === 'npc' ? value.entity.type : null,
    currentLife: value.currentLife,
    maximumLife: value.kind === 'npc' ? value.entity.stats.profile?.hitpoints : value.entity.stats.resources?.maxLife,
    instance: value.entity.instanceSession?.id ?? null
};
const belongs = value => value?.kind === 'player' && value.entity.accountKey === plan.accountKey;
world.combat.listen({
    launched: (source, target, style, delay, hit) => observe(() => {
        if (belongs(source) || belongs(target)) write({kind: 'launch', tick: world.tick, source: actor(source), target: actor(target), style, delay, damage: hit.damage});
    }),
    landed: (target, hit, result) => observe(() => {
        if (belongs(hit.attacker) || belongs(target)) {
            const native = target.events.hits.at(-ONE);
            if (!native) throw new Error('Ordinary landed impact has no native hitsplat');
            write({kind: 'landed', tick: world.tick, source: actor(hit.attacker), target: actor(target),
                style: hit.style, launchedDamage: hit.damage, nativeDamage: native.damage, ...result});
        }
    }),
    died: (target, source) => observe(() => {
        if (belongs(source) || belongs(target)) write({kind: 'death', tick: world.tick, source: actor(source), target: actor(target)});
    })
});
const prior = new Map();
for (const phase of ['npcs', 'players']) world.phases.on(phase, 'Slayer passive ordinary recording receipt', frame => observe(() => {
    const players = frame.players.filter(player => player.liveClient && player.accountKey === plan.accountKey);
    if (!players.length) return;
    const enemies = [...world.npcs].filter(npc => npc.type === plan.target.npc && players.some(player => player.level === npc.level &&
        Math.max(Math.abs(player.x - npc.x), Math.abs(player.z - npc.z)) <= OBSERVATION_RADIUS)).map(npc => ({
        ...actor(npc.combat), x: npc.x, z: npc.z, level: npc.level, hitpoints: npc.stats.hitpoints,
        maximumLife: npc.stats.profile?.hitpoints, visible: npc.life.visible,
        configuredMaximumLife: npc.stats.profile?.hitpoints
    }));
    const playerRows = players.map(player => {
        const savedBytes = fs.readFileSync(accountPath);
        const saved = JSON.parse(savedBytes);
        return {pid: player.pid, x: player.x, z: player.z, level: player.level,
            life: player.stats.resources?.life, maximumLife: player.stats.resources?.maxLife,
            regeneration: player.stats.resources ? regenerationStep(player.stats.resources.maxLife) : null,
            slayer: player.slayer.save(), slayerXp: player.stats.xp(SLAYER_SKILL),
            backpack: player.invs.backpack.slots, worn: player.invs.worn.slots,
            instance: player.instanceSession?.id ?? null,
            inventoryRevision: player.invs.revision, savedSlayer: saved.slayer,
            savedBackpack: saved.backpack, savedWorn: saved.worn,
            savedAccountSha256: createHash('sha256').update(savedBytes).digest('hex')};
    });
    const state = JSON.stringify({players: playerRows, enemies});
    if (prior.get(phase) !== state) {
        write({kind: 'state', phase, tick: frame.tick, ...JSON.parse(state)});
        prior.set(phase, state);
    }
    for (const failure of food.errors)
        if (errors.length < MAX_OBSERVER_ERRORS) errors.push(failure.reason);
    if (errors.length) fs.writeFileSync(path.join(work, 'passive-observer-errors.json'), JSON.stringify(errors));
    food.errors.length = FIRST;
}));
await world.start();
