/**
 * Which two stocks split and before/after compare. B is the edit itself.
 * A is either the settings the photo opened with, or the edit developed
 * through another stock, so the two sides differ only by stock.
 */

import type { StockRef, StockSummary } from "./contract.ts";

export class StockCompare {
  /** The A option that keeps the settings the photo opened with. */
  static readonly openedLabel = "As opened";

  /** The installed stock for an A choice, or `null` for "as opened". */
  static resolve(stocks: StockSummary[], id: string | null): StockRef | null {
    const stock = id ? stocks.find((item) => item.id === id) : undefined;
    return stock ? { id: stock.id, revision: stock.revision } : null;
  }

  /**
   * Swap the sides: A takes the edit's stock and the edit takes A's. Only a
   * stock A can swap. Returns `null` when there is nothing to swap.
   */
  static swap(stocks: StockSummary[], beforeId: string | null, afterId: string): { before: string; after: StockSummary } | null {
    const before = beforeId ? stocks.find((item) => item.id === beforeId) : undefined;
    if (!before || !stocks.some((item) => item.id === afterId) || before.id === afterId) {
      return null;
    }
    return { before: afterId, after: before };
  }
}
