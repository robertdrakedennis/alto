import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const ROOT_ARGUMENT=2;
const FIXTURE_ARGUMENT=3;
const root=process.argv[ROOT_ARGUMENT];
if (!root || !path.isAbsolute(root) || root==='/Users/robert/projects/alto') throw new Error('Own source root required');
const relative='server/src/formats/network/protocol/ServerProt.ts';
const fixture=process.argv[FIXTURE_ARGUMENT];
if (!fixture || !path.isAbsolute(fixture)) throw new Error('Actual frozen fixture required');
const hash=p=>createHash('sha256').update(fs.readFileSync(p)).digest('hex');
const epoch=JSON.parse(fs.readFileSync(path.join(fixture,'source-epoch.json')));
if (hash(path.join(root,relative))!==epoch[relative]) throw new Error('Actual captured encoder owner changed');
const {hitmarkType}=await import(pathToFileURL(path.join(root,relative)).href);
const rows=fs.readFileSync(path.join(fixture,'combat-receipts.jsonl'),'utf8').trimEnd().split('\n').map(JSON.parse);
const publications=rows.filter(row=>row.kind==='native_publication').map(row=>{
 const actor=entry=>({id:entry.id,generation:entry.generation,actorToken:entry.actorToken,definition:entry.definition,
   hits:entry.hits.map(hit=>({source:hit.source,poison:hit.poison??false,damage:hit.damage,
     type:hitmarkType(entry.kind==='player'||hit.source===row.player.id,hit.damage,hit.poison)}))});
 return {tick:row.tick,player:actor(row.player),enemies:row.enemies.map(actor)};
});
if (hash(path.join(root,relative))!==epoch[relative]) throw new Error('Encoder changed during pure qualification');
const result={status:'derived_from_captured_immutable_queue_inputs_through_original_pure_encoder',
 owner:{relative,sha256:epoch[relative]},originalCombatSha256:hash(path.join(fixture,'combat-receipts.jsonl')),
 originalRtrSha256:hash(path.join(fixture,'session.rtr')),gameplayWrites:0,publications};
fs.writeFileSync(path.join(fixture,'native-hit-types.json'),JSON.stringify(result,null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({publications:publications.length,encoderSha256:epoch[relative],fixtureSha256:hash(path.join(fixture,'native-hit-types.json')),gameplayWrites:0}));
