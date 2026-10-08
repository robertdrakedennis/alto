import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { component, location, npc, obj, varc } from "@alto/domain";
import CacheProvider from "../../../../../server/src/lostcity/server/CacheProvider.ts";
import SkillSet, {
  loadSkillDefinitions,
} from "../../../../../server/src/lostcity/systems/stats/SkillSet.ts";
import { loadAbilityData } from "../../../../../server/src/lostcity/data/AbilityData.ts";
import {
  ACTION_BAR_VARIABLES,
  ACTION_BOOK_BITS,
} from "../../../../../server/src/lostcity/systems/action-bar/ActionBarContract.ts";

const WORK_ARGUMENT = 2;
const ACCOUNT_VERSION = 2;
const ONE_ITEM = 1;
const EMPTY_ACTION = 0;
const NO_ITEM = -1;
const FIRST_INDEX = 0;
const SECOND_INDEX = 1;
const FIRST_OPERATION = 1;
const SECOND_OPERATION = 2;
const ATTACK_OPERATION = 2;
const FIXTURE_SKILL_XP = 40_000;
const ATTACK_SKILL = 0;
const DEFENCE_SKILL = 1;
const STRENGTH_SKILL = 2;
const CONSTITUTION_SKILL = 3;
const RANGED_SKILL = 4;
const MAGIC_SKILL = 6;
const COMBAT_SKILLS = [
  ATTACK_SKILL,
  DEFENCE_SKILL,
  STRENGTH_SKILL,
  CONSTITUTION_SKILL,
  RANGED_SKILL,
  MAGIC_SKILL,
];
const NATIVE_DIGIT_ONE = 16;
const AWT_DIGIT_ONE = 49;
const KEY_PRESS = 0;
const KEY_RELEASE = 1;
const NEXT_CYCLE = 1;
const LAST_CYCLE = 2150;
const COMMAND_COUNT = 20;
const ATTACK_COMMAND = 1;
const NPC_OFFSET_X = 2;
const NPC_OFFSET_Z = 0;
const EQUIP_CYCLE = 180;
const SECOND_BAR_CYCLE = 240;
const FIRST_BAR_CYCLE = 340;
const BASIC_CYCLE = 760;
const THRESHOLD_CYCLE = 900;
const BONUS_CYCLE = 1000;
const REFILL_BASIC_CYCLE = 1260;
const ULTIMATE_CYCLE = 1400;
const BUFF_CYCLE = 1550;
const accountKey = "abilityflow";
const work = process.argv[WORK_ARGUMENT];
await CacheProvider.load("data/pack");
const catalog = loadAbilityData();
const names = ["Slice", "Forceful Backhand", "Overpower", "Anticipation"];
const rules = names.map((name) => {
  const ability = catalog.abilities.find((row) => row.name === name);
  if (!ability) throw new Error(`Missing gameplay fixture ability ${name}`);
  return ability;
});
let skills = new SkillSet(await loadSkillDefinitions(CacheProvider.js5));
for (const skill of COMBAT_SKILLS)
  skills = skills.withXp(skill, FIXTURE_SKILL_XP);
const savedVarps = ACTION_BAR_VARIABLES.slice(
  FIRST_INDEX,
  SECOND_INDEX + ONE_ITEM,
).flatMap((bar, index) =>
  bar.flatMap((variables, slot) => {
    const ability =
      index === FIRST_INDEX
        ? rules[slot]
        : slot === FIRST_INDEX
          ? rules.at(-ONE_ITEM)
          : undefined;
    return [
      [
        variables.action,
        ability
          ? (ability.action << ACTION_BOOK_BITS) | ability.book
          : EMPTY_ACTION,
      ],
      [variables.item, NO_ITEM],
    ];
  }),
);
const account = {
  version: ACCOUNT_VERSION,
  accountKey,
  ...location.dev_player_spawn,
  backpack: [[obj.bronze_2h_sword, ONE_ITEM]],
  skills: skills.values,
  savedVarps,
  serverVarcs: [
    [varc.action_bar_main_keys_1_to_4, "int", String(NATIVE_DIGIT_ONE)],
  ],
};
const directory = path.join(work, "players", "accounts");
fs.mkdirSync(directory, { recursive: true });
fs.writeFileSync(
  path.join(
    directory,
    createHash("sha256").update(accountKey).digest("hex") + ".json",
  ),
  JSON.stringify(account),
);
const operations = [
  [EQUIP_CYCLE, component.backpack.slots, FIRST_INDEX, SECOND_OPERATION],
  [SECOND_BAR_CYCLE, component.action_bar.selector, NO_ITEM, SECOND_OPERATION],
  [FIRST_BAR_CYCLE, component.action_bar.selector, NO_ITEM, FIRST_OPERATION],
  [
    THRESHOLD_CYCLE,
    component.action_bar.piercing_shot_slot,
    NO_ITEM,
    FIRST_OPERATION,
  ],
  [BONUS_CYCLE, component.action_bar.slice_slot, NO_ITEM, FIRST_OPERATION],
  [
    REFILL_BASIC_CYCLE,
    component.action_bar.slice_slot,
    NO_ITEM,
    FIRST_OPERATION,
  ],
  [ULTIMATE_CYCLE, component.action_bar.wrack_slot, NO_ITEM, FIRST_OPERATION],
  [BUFF_CYCLE, component.action_bar.slot_4, NO_ITEM, FIRST_OPERATION],
];
const keys = [
  [BASIC_CYCLE, AWT_DIGIT_ONE, KEY_PRESS],
  [BASIC_CYCLE + NEXT_CYCLE, AWT_DIGIT_ONE, KEY_RELEASE],
];
const commands = Array.from({ length: COMMAND_COUNT }, () => "notimeout");
commands[FIRST_INDEX] =
  `npcadd ${npc.ability_practice_dummy} ${NPC_OFFSET_X} ${NPC_OFFSET_Z}`;
commands[ATTACK_COMMAND] =
  `opnpc ${ATTACK_OPERATION} ${npc.ability_practice_dummy}`;
const rows = (entries) => entries.map((row) => row.join(",")).join(";");
fs.writeFileSync(
  path.join(work, "plan.json"),
  JSON.stringify({
    accountKey,
    commands,
    operations: rows(operations),
    keys: rows(keys),
    gestures: "",
    lastCycle: LAST_CYCLE,
    definitions: Object.fromEntries(
      rules.map((rule) => [rule.name, rule.definition]),
    ),
    stages: {
      equip: EQUIP_CYCLE,
      secondBar: SECOND_BAR_CYCLE,
      firstBar: FIRST_BAR_CYCLE,
      basic: BASIC_CYCLE,
      threshold: THRESHOLD_CYCLE,
      bonus: BONUS_CYCLE,
      refillBasic: REFILL_BASIC_CYCLE,
      ultimate: ULTIMATE_CYCLE,
      buff: BUFF_CYCLE,
    },
  }),
);
