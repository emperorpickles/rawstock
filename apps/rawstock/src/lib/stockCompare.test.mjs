import assert from "node:assert/strict";
import { test } from "node:test";

import { StockCompare } from "./stockCompare.ts";

const stock = (id, name, revision = 1) => ({ id, name, category: "colorNegative", model: "parametric", revision });
const stocks = [stock("neutral", "Neutral", 2), stock("portra400", "Kodak Portra 400", 5), stock("velvia", "Velvia", 3)];

test("an installed A stock resolves with its revision; anything else is as opened", () => {
  assert.deepEqual(StockCompare.resolve(stocks, "velvia"), { id: "velvia", revision: 3 });
  assert.equal(StockCompare.resolve(stocks, null), null);
  assert.equal(StockCompare.resolve(stocks, "provia"), null);
});

test("swap trades the stocks, and only when A is another installed stock", () => {
  const swapped = StockCompare.swap(stocks, "neutral", "portra400");
  assert.equal(swapped?.before, "portra400");
  assert.equal(swapped?.after.id, "neutral");
  assert.equal(StockCompare.swap(stocks, null, "portra400"), null);
  assert.equal(StockCompare.swap(stocks, "velvia", "velvia"), null);
  assert.equal(StockCompare.swap(stocks, "provia", "velvia"), null);
});
