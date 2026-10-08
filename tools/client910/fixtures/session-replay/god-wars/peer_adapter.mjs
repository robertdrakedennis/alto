// One actual authenticated core, with normal byte forwarding through the owned broker.
// The external owned runner launches/reaps lobby, World, Rust core and driver.
// This adapter never starts processes or writes player/game state.
import fs from "node:fs";
import path from "node:path";
import {createHash} from "node:crypto";
import {pathToFileURL} from "node:url";

const FIRST_ARGUMENT = 2;
const SINGLE_ACCOUNT = "Alice";
const MAX_MANIFEST_BYTES = 64 * 1024;
const ONE = 1;
const specPath = process.argv[FIRST_ARGUMENT];
if (!specPath || !path.isAbsolute(specPath)) throw new Error("Explicit absolute single-peer manifest required");
const bytes = fs.readFileSync(specPath);
if (bytes.length > MAX_MANIFEST_BYTES) throw new Error("Single-peer manifest bound exceeded");
const spec = JSON.parse(bytes);
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
function checked(entry) {
  if (!entry || !path.isAbsolute(entry.path) || sha(fs.readFileSync(entry.path)) !== entry.sha256)
    throw new Error("Hash-bound source changed or is absent");
  return entry.path;
}
const broker = checked(spec.broker);
for (const module of Object.values(spec.peer.modules)) checked(module);
checked(spec.loginTestkit);
checked(spec.deviceEnvelope);
if (spec.peer.name !== SINGLE_ACCOUNT || spec.peerCount !== ONE
    || spec.peer.deviceEnvelope !== spec.deviceEnvelope.path || !spec.headlessOnly)
  throw new Error("Only one declared Alice headless peer is admitted");
if (!path.isAbsolute(spec.receipts) || fs.existsSync(spec.receipts)) throw new Error("Fresh absolute backend receipt required");
const output = fs.openSync(spec.receipts, "wx");
const record = row => fs.writeSync(output, JSON.stringify(row) + "\n");
const abort = new AbortController();
const stop = () => abort.abort();
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
process.env.ALTO_PREFLIGHT_LOGIN_MODULE = JSON.stringify(spec.loginTestkit);
try {
  record({kind: "headless-source-manifest", path: specPath, sha256: sha(bytes), spec,
    scope: "shared normal authenticated core; no window/GPU/native launch"});
  const {runPeer} = await import(pathToFileURL(broker).href);
  await runPeer(spec.peer, record, abort.signal);
} finally {
  process.off("SIGTERM", stop);
  process.off("SIGINT", stop);
  fs.fsyncSync(output);
  fs.closeSync(output);
}
