// Passive wrappers retain the original repair, special, queue and RNG owners.
import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';
import {observeRepairCommits} from '../barrows/repair-receipts.mjs';
import {observeFoodCommits} from '../barrows/food-receipts.mjs';

const SPECIAL_FIRST_ARGUMENT = 2;
const SPECIAL_ONE = 1;
const SPECIAL_EMPTY = 0;
const SPECIAL_MAX_ERRORS = 64;
const SPECIAL_NEARBY_TILES = 104;
const specialRoot = process.env.ALTO_SPECIAL_RUNTIME_ROOT;
const specialWork = process.argv[SPECIAL_FIRST_ARGUMENT];
if (!specialRoot || !path.isAbsolute(specialRoot) || !specialWork || !path.isAbsolute(specialWork)) throw new Error('Explicit own runtime and scratch required');
const specialOwner = relative => import(pathToFileURL(path.join(specialRoot, relative)).href);
const [{World}, {default: PlayerItemCharges}, {default: Consumables}, {default: PlayerSpecials}, {default: SpecialFieldEffects}, {default: CacheProvider}, {varp}] = await Promise.all([
    specialOwner('server/src/lostcity/engine/World.ts'),
    specialOwner('server/src/lostcity/systems/equipment/PlayerItemCharges.ts'),
    specialOwner('server/src/lostcity/systems/consumables/Consumables.ts'),
    specialOwner('server/src/lostcity/systems/combat/PlayerSpecials.ts'),
    specialOwner('server/src/lostcity/systems/combat/SpecialFieldEffects.ts'),
    specialOwner('server/src/lostcity/server/CacheProvider.ts'),
    specialOwner('packages/domain/src/index.ts'),
]);
const specialPlan = JSON.parse(fs.readFileSync(path.join(specialWork, 'plan.json')));
const specialWorld = new World();
const specialErrors = [];
const specialWrite = row => fs.appendFileSync(path.join(specialWork, 'combat-receipts.jsonl'), JSON.stringify(row) + '\n');
const specialObserve = task => {try {task();} catch (error) {if (specialErrors.length < SPECIAL_MAX_ERRORS) specialErrors.push({tick: specialWorld.tick, reason: String(error)}); fs.writeFileSync(path.join(specialWork, 'passive-observer-errors.json'), JSON.stringify(specialErrors));}};
const specialAccountPath = path.join(specialWork, 'players/accounts', createHash('sha256').update(specialPlan.accountKey).digest('hex') + '.json');
const specialRepairs = observeRepairCommits(PlayerItemCharges, specialPlan.accountKey, specialAccountPath, specialWrite, () => specialWorld.tick);
const specialFood = observeFoodCommits(Consumables, specialPlan.accountKey, specialWrite);
const specialActor = actor => actor == null ? null : {kind: actor.kind, id: actor.id, generation: actor.lifeId,
    definition: actor.kind === 'npc' ? actor.entity.type : null, x: actor.x, z: actor.z, level: actor.level,
    currentLife: actor.currentLife, maximumLife: actor.kind === 'npc' ? actor.entity.stats.profile?.hitpoints : actor.entity.stats.resources?.maxLife,
    instance: actor.entity.instanceSession?.id ?? null};
const specialBelongs = actor => actor?.kind === 'player' && actor.entity.accountKey === specialPlan.accountKey;
const specialLaunch = PlayerSpecials.prototype.launch;
PlayerSpecials.prototype.launch = function (...args) {
    const before = {fine: this.energy.fine, armed: this.armed};
    const outcome = specialLaunch.apply(this, args);
    if (this.player.accountKey === specialPlan.accountKey && outcome !== 'unprepared') specialObserve(() => specialWrite({kind: 'special_launch', tick: specialWorld.tick, outcome, before, after: {fine: this.energy.fine, armed: this.armed}, pid: this.player.pid}));
    return outcome;
};
const specialInstall = SpecialFieldEffects.prototype.install;
SpecialFieldEffects.prototype.install = function (prepared, tick, commit) {
    const accepted = specialInstall.call(this, prepared, tick, commit);
    if (specialBelongs(prepared.context.source.actor)) specialObserve(() => specialWrite({kind: 'field_install', tick, accepted, key: prepared.key, source: specialActor(prepared.context.source.actor),
        primary: specialActor(prepared.context.primary?.actor), durationTicks: prepared.plan.durationTicks, area: prepared.area == null ? null : {center: prepared.area.center, radiusTiles: prepared.area.radiusTiles},
        outcomes: prepared.outcomes.map(row => ({dueTicks: row.dueTicks, recipient: specialActor(row.recipient.actor), damage: row.hit.damage, family: row.hit.ability?.family})), qualification: prepared.qualification}));
    return accepted;
};
const specialRetire = SpecialFieldEffects.prototype.retire;
SpecialFieldEffects.prototype.retire = function (entry, cancelled) {
    const result = specialRetire.call(this, entry, cancelled);
    if (specialBelongs(entry.prepared.context.source.actor)) specialObserve(() => specialWrite({kind: 'field_retire', tick: specialWorld.tick, key: entry.prepared.key, cancelled, untilTick: entry.untilTick, nextOutcome: entry.nextOutcome}));
    return result;
};
specialWorld.combat.listen({
    launched: (source, target, style, delay, hit) => {if (specialBelongs(source) || specialBelongs(target)) specialObserve(() => specialWrite({kind: 'launch', tick: specialWorld.tick, source: specialActor(source), target: specialActor(target), style, delay, damage: hit.damage, ability: hit.ability ?? null}));},
    landed: (target, hit, result) => {if (specialBelongs(hit.attacker) || specialBelongs(target)) specialObserve(() => {const native = target.events.hits.at(-SPECIAL_ONE); if (!native) throw new Error('No ordinary native hitsplat after landed impact'); specialWrite({kind: 'landed', tick: specialWorld.tick, source: specialActor(hit.attacker), target: specialActor(target), nativeDamage: native.damage, launchedDamage: hit.damage, ...result});});},
    died: (target, source) => {if (specialBelongs(source) || specialBelongs(target)) specialObserve(() => specialWrite({kind: 'death', tick: specialWorld.tick, source: specialActor(source), target: specialActor(target)}));},
});
const specialPrior = new Map();
for (const phase of ['npcs', 'players']) specialWorld.phases.on(phase, 'Passive special repair receipt', frame => specialObserve(() => {
    const players = frame.players.filter(player => player.liveClient && player.accountKey === specialPlan.accountKey);
    if (!players.length) return;
    const enemies = [...specialWorld.npcs].filter(npc => [specialPlan.target.npc, specialPlan.repair.bob.npc].includes(npc.type) && players.some(player => player.level === npc.level && Math.max(Math.abs(player.x - npc.x), Math.abs(player.z - npc.z)) <= SPECIAL_NEARBY_TILES)).map(npc => ({...specialActor(npc.combat), hitpoints: npc.stats.hitpoints, alive: npc.life.alive, visible: npc.life.visible}));
    const playerRows = players.map(player => ({pid: player.pid, generation: player.combat.lifeId, x: player.x, z: player.z, level: player.level,
        life: player.stats.resources?.life, maximumLife: player.stats.resources?.maxLife, prayerFine: player.stats.resources?.prayerFine,
        backpack: player.invs.backpack.slots, worn: player.invs.worn.slots, coins: player.money.coins, inventoryRevision: player.invs.revision,
        savedVarps: player.vars.savedVarps(), instance: player.instanceSession?.id ?? null,
        energy: player.combat.specials.energy.fine, armed: player.combat.specials.armed,
        energyFeed: player.vars.number(varp.adrenaline_fine),
        counts: specialPlan.gwd.counts.map(row => ({varbit: row.varbit, value: player.vars.varbit(CacheProvider.config.varbit(row.varbit))})),
        conditions: player.combat.statuses.activeConditions.filter(view => view.condition.key === `special-field:${specialPlan.special.rule.weapon}`).map(view => ({generation: view.generation, key: view.condition.key})),
        target: player.combat.state.target == null ? null : {kind: player.combat.state.target.kind, id: player.combat.state.target.reference.id, generation: player.combat.state.target.reference.generation}}));
    const current = JSON.stringify({players: playerRows, enemies});
    if (specialPrior.get(phase) !== current) {specialWrite({kind: 'state', tick: frame.tick, phase, ...JSON.parse(current)}); specialPrior.set(phase, current);}
    for (const error of [...specialFood.errors, ...specialRepairs.errors]) throw new Error(error.reason ?? error.message);
    specialFood.errors.length = SPECIAL_EMPTY;
    specialRepairs.errors.length = SPECIAL_EMPTY;
}));
await specialWorld.start();
