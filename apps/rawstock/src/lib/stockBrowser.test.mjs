import assert from "node:assert/strict";
import { test } from "node:test";

import { defaultSettings } from "./contract.ts";
import { StockBrowser, ThumbnailRequests } from "./stockBrowser.ts";

const stock = (id, name, category, model = "parametric", revision = 1) => ({
  id,
  name,
  category,
  model,
  revision,
});

const catalog = [
  stock("velvia", "Velvia", "fujiSim"),
  stock("portra400", "Kodak Portra 400", "colorNegative", "spectral", 5),
  stock("neutral", "Neutral", "colorNegative"),
  stock("ilford-hp5-plus", "Ilford HP5 Plus", "bw", "spectral", 4),
  stock("provia", "Provia", "fujiSim"),
];

test("the shipped app hides the development stock", () => {
  assert.deepEqual(
    StockBrowser.visible(catalog, false).map((item) => item.id),
    ["velvia", "portra400", "ilford-hp5-plus", "provia"],
  );
  assert.equal(StockBrowser.visible(catalog, true).length, catalog.length);
});

test("groups keep a fixed order, sort names, and drop empty categories", () => {
  const groups = StockBrowser.groups(StockBrowser.visible(catalog, false));
  assert.deepEqual(
    groups.map((group) => group.label),
    ["Color negative", "Black and white", "Fuji simulation"],
  );
  assert.deepEqual(
    groups[2].stocks.map((item) => item.id),
    ["provia", "velvia"],
  );
});

test("labels name the stock and its category, details add model and revision", () => {
  assert.equal(StockBrowser.label(catalog[1]), "Kodak Portra 400, Color negative");
  assert.equal(StockBrowser.detail(catalog[1]), "Kodak Portra 400 · spectral · revision 5");
});

test("only settings a thumbnail shows change its key", () => {
  const base = defaultSettings();
  const key = StockBrowser.thumbnailKey(base);
  assert.equal(
    StockBrowser.thumbnailKey({ ...base, stock: { id: "velvia", revision: 3 } }),
    key,
  );
  assert.equal(StockBrowser.thumbnailKey({ ...base, grain: { amount: 2, size: 1 } }), key);
  assert.notEqual(StockBrowser.thumbnailKey({ ...base, exposureEv: 1 }), key);
  assert.notEqual(
    StockBrowser.thumbnailKey({ ...base, geometry: { quarterTurns: 1, straightenDeg: 0 } }),
    key,
  );
  assert.notEqual(StockBrowser.thumbnailKey({ ...base, lens: { caCorrection: false } }), key);
});

test("arrow keys move across and between rows and groups", () => {
  // Color negative: [0 1] [2]; Black and white: [3]; Fuji: [4 5]
  const groups = StockBrowser.groups([
    stock("a", "A", "colorNegative"),
    stock("b", "B", "colorNegative"),
    stock("c", "C", "colorNegative"),
    stock("d", "D", "bw"),
    stock("e", "E", "fujiSim"),
    stock("f", "F", "fujiSim"),
  ]);
  assert.equal(StockBrowser.move(groups, 0, "ArrowRight"), 1);
  assert.equal(StockBrowser.move(groups, 5, "ArrowRight"), 5);
  assert.equal(StockBrowser.move(groups, 0, "ArrowLeft"), 0);
  assert.equal(StockBrowser.move(groups, 1, "ArrowDown"), 2);
  assert.equal(StockBrowser.move(groups, 2, "ArrowDown"), 3);
  assert.equal(StockBrowser.move(groups, 3, "ArrowDown"), 4);
  assert.equal(StockBrowser.move(groups, 5, "ArrowUp"), 3);
  assert.equal(StockBrowser.move(groups, 0, "ArrowUp"), 0);
  assert.equal(StockBrowser.move(groups, 2, "End"), 5);
  assert.equal(StockBrowser.move(groups, 4, "Home"), 0);
  assert.equal(StockBrowser.move(groups, 0, "a"), null);
  assert.equal(StockBrowser.move([], 0, "ArrowDown"), null);
});

test("frames from an earlier image or job are ignored", () => {
  const requests = new ThumbnailRequests();
  assert.equal(requests.needs("", "k"), false);
  assert.equal(requests.needs("a", "k"), true);
  requests.begin("a", "k");
  assert.equal(requests.needs("a", "k"), false);
  assert.equal(requests.started("a", "k", 7), true);
  assert.equal(requests.accepts({ imageId: "a", jobId: 7 }), true);
  assert.equal(requests.accepts({ imageId: "a", jobId: 6 }), false);

  requests.begin("b", "k");
  assert.equal(requests.accepts({ imageId: "a", jobId: 7 }), false);
  // A reply for the old request that lands after the switch is refused.
  assert.equal(requests.started("a", "k", 8), false);
  assert.equal(requests.started("b", "k", 9), true);
  assert.equal(requests.accepts({ imageId: "b", jobId: 9 }), true);

  requests.clear();
  assert.equal(requests.accepts({ imageId: "b", jobId: 9 }), false);
});
