// Private passive wrapper. The original prepare/commit/payment/save executes unchanged.
import fs from "node:fs";
import {createHash} from "node:crypto";

export function observeRepairCommits(Owner, accountKey, accountPath, write, tick) {
  const errors = [];
  const snapshot = player => structuredClone({pid: player.pid,
    backpack: player.invs.backpack.slots, worn: player.invs.worn.slots,
    coins: player.money.coins, revision: player.invs.revision, life: player.combat.lifeId});
  for (const method of ["prepareCoinRepair", "prepareCoinRepairBatch"]) {
    const original = Owner.prototype[method];
    if (typeof original !== "function") throw new Error(`Ordinary repair owner missing ${method}`);
    Owner.prototype[method] = function (...args) {
      const prepared = original.apply(this, args);
      const player = this.player;
      if (!prepared || player.accountKey !== accountKey) return prepared;
      const commit = prepared.commit;
      prepared.commit = (...commitArgs) => {
        const before = snapshot(player);
        const accepted = commit.apply(prepared, commitArgs);
        if (accepted) {
          try {
            const savedBytes = fs.readFileSync(accountPath);
            const saved = JSON.parse(savedBytes);
            write({kind: "repair_commit", method, tick: tick(), pid: player.pid,
              price: prepared.price, before, after: snapshot(player),
              saved: {backpack: saved.backpack, worn: saved.worn, coins: saved.coins,
                revision: saved.revision, house: saved.house},
              savedPath: accountPath,
              savedSha256: createHash("sha256").update(savedBytes).digest("hex")});
          } catch (error) {
            errors.push({kind: "repair_observer_failure", method, message: String(error)});
          }
        }
        return accepted;
      };
      return prepared;
    };
  }
  return {errors};
}
