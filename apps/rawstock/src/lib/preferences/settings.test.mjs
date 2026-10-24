import assert from "node:assert/strict";
import { test } from "node:test";

import { Preferences } from "./settings.ts";

const stock = (id, name) => ({ id, name, category: "colorNegative", model: "parametric", revision: 1 });
const stocks = [stock("ilford-hp5-plus", "Ilford HP5 Plus"), stock("neutral", "Neutral"), stock("velvia", "Velvia")];

const withDefault = (defaultStockId) => {
  const settings = Preferences.defaults();
  return { ...settings, newImage: { ...settings.newImage, defaultStockId } };
};

test("new photos open on Neutral unless Preferences names a stock", () => {
  assert.equal(Preferences.startingStock(withDefault(null), stocks)?.id, "neutral");
  assert.equal(Preferences.startingStock(withDefault("velvia"), stocks)?.id, "velvia");
});

test("a missing preferred stock falls back to Neutral, then to the first stock", () => {
  assert.equal(Preferences.startingStock(withDefault("provia"), stocks)?.id, "neutral");
  const noNeutral = stocks.filter((item) => item.id !== "neutral");
  assert.equal(Preferences.startingStock(withDefault(null), noNeutral)?.id, "ilford-hp5-plus");
  assert.equal(Preferences.startingStock(withDefault(null), []), null);
});
