import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { component, location, npc, obj, param } from "@alto/domain";
import CacheProvider from "../../../../../server/src/lostcity/server/CacheProvider.ts";
import SkillSet, {
  loadSkillDefinitions,
} from "../../../../../server/src/lostcity/systems/stats/SkillSet.ts";
import { loadSpecialCombatData } from "../../../../../server/src/lostcity/data/SpecialCombatData.ts";
const FIRST_ARGUMENT = 2;
const ACCOUNT_FORMAT = 2;
const FIRST_SLOT = 0;
const ONE_ITEM = 1;
const MAXIMUM_SKILL = 99;
const COMBAT_SKILLS = [0, 1, 2, 3, 4, 5, 6];
const ABSENT = -1;
const EQUIP_OPERATION = 2;
const COMMAND_PERIOD = 100;
const COMMAND_COUNT = 37;
const SPECIAL_POINTER = { x: 919, y: 613 };
const CYCLE = {
  RETALIATION_OFF: 300,
  EQUIP: 800,
  READY: 1000,
  ARM: 1100,
  ARMED_REVIEW: 1175,
  DISARM: 1250,
  DISARMED_REVIEW: 1325,
  REARM: 1400,
  REARMED_REVIEW: 1450,
  SPAWN: 1500,
  ATTACK: 1600,
  DRAIN_REVIEW: 1800,
  FINAL_REVIEW: 3500,
};
const work = process.argv[FIRST_ARGUMENT];
const accountKey = "legacy-specials";
await CacheProvider.load("data/pack");
await CacheProvider.loadConfig();
const config = CacheProvider.config;
const definitions = await loadSkillDefinitions(CacheProvider.js5);
let skills = new SkillSet(definitions);
for (const stat of COMBAT_SKILLS)
  skills = skills.withXp(
    stat,
    definitions[stat].table[MAXIMUM_SKILL - ONE_ITEM],
  );
const weapon = config.obj(obj.granite_maul);
const rules = loadSpecialCombatData();
const special = rules.weapons.find(
  (row) => row.weapon === weapon.id,
);
if (!special || special.unavailable)
  throw new Error("Missing qualified ordinary Granite maul special");
if (
  Number(weapon.params.get(param.equipment_quest_requirement) ?? ABSENT) !==
  ABSENT
)
  throw new Error("Capture weapon has an unmet quest requirement");
const directory = path.join(work, "players", "accounts");
fs.mkdirSync(directory, { recursive: true });
fs.writeFileSync(
  path.join(
    directory,
    `${createHash("sha256").update(accountKey).digest("hex")}.json`,
  ),
  JSON.stringify({
    version: ACCOUNT_FORMAT,
    accountKey,
    ...location.dev_player_spawn,
    skills: skills.values,
    backpack: [[obj.granite_maul, ONE_ITEM]],
  }),
);
const commands = Array.from({ length: COMMAND_COUNT }, () => "notimeout");
const at = (cycle, command) => {
  commands[cycle / COMMAND_PERIOD - ONE_ITEM] = command;
};
at(CYCLE.RETALIATION_OFF, "retaliate off");
at(CYCLE.SPAWN, `npcadd ${npc.chicken}`);
at(
  CYCLE.ATTACK,
  `opnpc ${config.npc(npc.chicken).op.findIndex((value) => value?.toLowerCase() === "attack") + ONE_ITEM} ${npc.chicken}`,
);
const operations = [
  [CYCLE.EQUIP, component.backpack.slots, FIRST_SLOT, EQUIP_OPERATION],
];
const clicks = [CYCLE.ARM, CYCLE.DISARM, CYCLE.REARM].map(
  (cycle) => `l,${SPECIAL_POINTER.x},${SPECIAL_POINTER.y},${cycle}`,
);
fs.writeFileSync(
  path.join(work, "plan.json"),
  JSON.stringify(
    {
      accountKey,
      commands,
      operations: operations.map((row) => row.join(",")).join(";"),
      clicks: clicks.join(";"),
      lastCycle: CYCLE.FINAL_REVIEW,
      semanticShots: [
        CYCLE.READY,
        CYCLE.ARMED_REVIEW,
        CYCLE.DISARMED_REVIEW,
        CYCLE.REARMED_REVIEW,
        CYCLE.DRAIN_REVIEW,
        CYCLE.FINAL_REVIEW,
      ],
      energy: {maximumEnergyFine: rules.maximumEnergyFine, regenerationFine: rules.regenerationFine, regenerationTicks: rules.regenerationTicks},
        special: {
        weapon: weapon.id,
        costFine: special.costFine,
        sequence: special.sequence,
        userSpot: special.userSpot,
      },
      pointerQualification:
        "Actual cache/native Legacy Combat pane bounds at1024x768 from the corrected CLIENT-domain selection; fresh normal login/pick proof is required in this recording",
      presentationQualification: special.qualification,
      inputQualification:
        "Bar uses actual pointer pick and native IF_BUTTON1; equipment uses ordinary native operation; recorded developer opnpc selects the same ordinary interaction owner as separately socket-proved OPNPC",
    },
    null,
    FIRST_ARGUMENT,
  ),
);
