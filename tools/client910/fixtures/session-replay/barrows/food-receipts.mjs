// Private passive instrumentation of the existing synchronous food transaction.
// The original owner runs once with unchanged arguments, result and exceptions.
const ITEM_FIELD = 0;
const QUANTITY_FIELD = 1;

export function observeFoodCommits(Consumables, accountKey, write) {
  const original = Consumables.prototype.eatFood;
  const errors = [];
  Consumables.prototype.eatFood = function (lease, slot, item, tick) {
    const player = this.player;
    if (player.accountKey !== accountKey) {
      return original.call(this, lease, slot, item, tick);
    }
    const before = player.stats.resources;
    const held = player.invs.backpack.slots[slot]?.slice();
    const accepted = original.call(this, lease, slot, item, tick);
    if (accepted) {
      const after = player.stats.resources;
      const remaining = player.invs.backpack.slots[slot]?.slice();
      try {
        write({
          kind: "food_commit",
          tick,
          pid: player.pid,
          item,
          slot,
          beforeItem: held?.[ITEM_FIELD],
          beforeCount: held?.[QUANTITY_FIELD],
          afterItem: remaining?.[ITEM_FIELD],
          afterCount: remaining?.[QUANTITY_FIELD],
          lifeBefore: before?.life,
          lifeAfter: after?.life,
          maximumLife: after?.maxLife,
          healed: before && after ? after.life - before.life : null,
          qualification: "Observed after the ordinary synchronous eatFood returned true; its existing save-before-publication transaction owns durability. This observer neither heals nor consumes.",
        });
      } catch (error) {
        // An observer write failure cannot alter a committed ordinary action.
        // The finite recorder must reject a nonempty error file independently.
        errors.push({tick, reason: String(error)});
      }
    }
    return accepted;
  };
  return {
    errors,
    restore() {
      Consumables.prototype.eatFood = original;
    },
  };
}
