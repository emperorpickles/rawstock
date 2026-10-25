import assert from "node:assert/strict";
import { test } from "node:test";

import { defaultSettings } from "./contract.ts";
import { StripOrder } from "./strip.ts";

const photo = (id, error) => ({ imageId: id, fileName: `${id}.RAF`, ...(error ? { error } : {}) });
const settings = (image) => ({ ...defaultSettings(), stock: { id: `stock-${image.imageId}`, revision: 1 } });

test("the queue starts at the selected photo and works outward", () => {
  const images = ["a", "b", "c", "d", "e"].map((id) => photo(id));
  const order = StripOrder.requests(images, "c", settings).map((item) => item.imageId);
  assert.deepEqual(order, ["c", "d", "b", "e", "a"]);
  assert.equal(StripOrder.requests(images, "c", settings)[0].settings.stock.id, "stock-c");
});

test("with nothing selected the strip fills from the left, and broken photos are skipped", () => {
  const images = [photo("a"), photo("b", { code: "corruptFile", message: "x" }), photo("c")];
  assert.deepEqual(
    StripOrder.requests(images, "", settings).map((item) => item.imageId),
    ["a", "c"],
  );
  assert.deepEqual(StripOrder.requests([], "", settings), []);
});

test("the key changes only when an edit or the order does", () => {
  const images = ["a", "b"].map((id) => photo(id));
  const first = StripOrder.key(StripOrder.requests(images, "a", settings));
  assert.equal(StripOrder.key(StripOrder.requests(images, "a", settings)), first);
  assert.notEqual(StripOrder.key(StripOrder.requests(images, "b", settings)), first);
});
