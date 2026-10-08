// The existing recorded login corpus supplies only the protocol login request.
// Fresh authenticated server answers and packets boot the actual headless core.
import fs from "node:fs";
import {createHash} from "node:crypto";
import {pathToFileURL} from "node:url";

const SINGLE_ACCOUNT = "Alice";
const SINGLE_MATCH = 1;
const FIRST_MATCH = 0;
const spec = JSON.parse(process.env.ALTO_PREFLIGHT_LOGIN_MODULE || "null");
if (!spec || !spec.path || !spec.sha256) throw new Error("Hash-bound owning login testkit is required");
const actual = createHash("sha256").update(fs.readFileSync(spec.path)).digest("hex");
if (actual !== spec.sha256) throw new Error("Owning login testkit changed");
const {LOGIN_CORPUS} = await import(pathToFileURL(spec.path).href);

export function loginFor(name) {
  if (name !== SINGLE_ACCOUNT) throw new Error("This preflight declares only one Alice account");
  const matches = LOGIN_CORPUS.filter(row => row.username === name);
  if (matches.length !== SINGLE_MATCH || !Array.isArray(matches[FIRST_MATCH].payload))
    throw new Error("The committed native login request is absent or ambiguous");
  return [...matches[FIRST_MATCH].payload];
}
