import assert from "node:assert/strict";
import { test } from "node:test";

import { defaultSettings } from "../contract.ts";
import { Clipping, Divider, ReviewController, Viewport } from "./review.ts";

function packed(width, height, value = 128) {
  const bytes = new Uint8Array(8 + width * height * 4);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, width, true);
  view.setUint32(4, height, true);
  bytes.fill(value, 8);
  return bytes;
}

const settled = () => new Promise((resolve) => setImmediate(resolve));

async function flush() {
  for (let index = 0; index < 6; index += 1) {
    await settled();
  }
}

/** Records commands. Frame sizes come from `sizes`, keyed by job kind. */
class FakeHost {
  constructor() {
    this.calls = [];
    this.next = 0;
    this.jobs = new Map();
    this.region = () => packed(4, 4);
  }

  async developPreview(imageId, settings, longEdge, compare) {
    this.next += 1;
    this.calls.push({ command: "develop_preview", jobId: this.next, imageId, longEdge, compare, settings });
    this.jobs.set(this.next, { kind: "preview", settings });
    return { jobId: this.next };
  }

  async developFull(imageId, settings) {
    this.next += 1;
    this.calls.push({ command: "develop_full", jobId: this.next, imageId, settings });
    this.jobs.set(this.next, { kind: "full", settings });
    return { jobId: this.next };
  }

  async takeFrame(jobId) {
    this.calls.push({ command: "take_frame", jobId });
    const job = this.jobs.get(jobId);
    return job.kind === "full" ? packed(48, 32) : packed(24, 16);
  }

  async getRegion(args) {
    this.calls.push({ command: "get_region", ...args });
    return this.region(args);
  }

  count(command) {
    return this.calls.filter((call) => call.command === command).length;
  }

  last(command) {
    return this.calls.filter((call) => call.command === command).at(-1);
  }
}

function harness() {
  const host = new FakeHost();
  const views = [];
  const errors = [];
  const review = new ReviewController(host, {
    onview: (view) => views.push(view),
    onerror: (error) => errors.push(error),
  });
  return { host, review, views, errors, view: () => review.snapshot() };
}

function settings(exposureEv = 0) {
  return { ...defaultSettings(), stock: { id: "portra400", revision: 1 }, exposureEv };
}

function done(review, call, extra = {}) {
  review.done({
    jobId: call.jobId,
    imageId: call.imageId,
    kind: call.command === "develop_full" ? "full" : "preview",
    settingsHash: `hash-${call.settings.exposureEv}`,
    width: 48,
    height: 32,
    exact: call.command === "develop_full",
    ...extra,
  });
}

function ready(review, call, width = 48, height = 32) {
  review.cacheReady({ imageId: call.imageId, settingsHash: `hash-${call.settings.exposureEv}`, width, height });
}

test("fit leaves an 8 px gutter and keeps the aspect", () => {
  const scale = Viewport.fitScale(6000, 4000, 1216, 816, 8);
  assert.equal(scale, 0.2);
  assert.equal(Viewport.scale(2, 6000, 4000, 1216, 816, 8), 2);
});

test("a small image stays centred and a large one stops at the gutter", () => {
  assert.deepEqual(Viewport.clampCenter({ x: 0, y: 1 }, 100, 50, 400, 300, 1, 8), { x: 0.5, y: 0.5 });
  const center = Viewport.clampCenter({ x: 0, y: 1 }, 1000, 1000, 400, 300, 1, 8);
  assert.equal(center.x, (200 - 8) / 1000);
  assert.equal(center.y, 1 - (150 - 8) / 1000);
});

test("double-click keeps the clicked pixel under the pointer", () => {
  const fitCenter = { x: 0.5, y: 0.5 };
  const fit = Viewport.fitScale(6000, 4000, 1216, 816, 8);
  const pointer = { x: 300, y: 200 };
  const pixel = Viewport.imageAt(pointer, fitCenter, 6000, 4000, 1216, 816, fit);
  const center = Viewport.centerKeeping(pixel, pointer, 6000, 4000, 1216, 816, 1);
  const again = Viewport.imageAt(pointer, center, 6000, 4000, 1216, 816, 1);
  assert.ok(Math.abs(again.x - pixel.x) < 1e-9);
  assert.ok(Math.abs(again.y - pixel.y) < 1e-9);
});

test("the visible rectangle is whole pixels inside the image", () => {
  const rect = Viewport.visibleRect({ x: 0.5, y: 0.5 }, 6000, 4000, 1200, 800, 2);
  assert.deepEqual(rect, { x: 2700, y: 1800, width: 600, height: 400 });
  assert.deepEqual(Viewport.padded(rect, 6000, 4000), { x: 2400, y: 1600, width: 1200, height: 800 });
});

test("clipping marks a synthetic gradient's ends in distinct colours", () => {
  const width = 256;
  const rgba = new Uint8ClampedArray(width * 4);
  for (let x = 0; x < width; x += 1) {
    rgba.set([x, x, x, 255], x * 4);
  }
  const mask = Clipping.mask(rgba, width, 1);
  assert.deepEqual([...mask.subarray(0, 4)], [...Clipping.shadow, 255]);
  assert.deepEqual([...mask.subarray(255 * 4, 256 * 4)], [...Clipping.highlight, 255]);
  for (let x = 1; x < 255; x += 1) {
    assert.equal(mask[x * 4 + 3], 0, `pixel ${x} is not clipped`);
  }
  assert.deepEqual(Clipping.counts(rgba), { highlights: 1, shadows: 1 });
  assert.notDeepEqual(Clipping.highlight, Clipping.shadow);
});

test("clipping counts any single channel, and a highlight wins", () => {
  const rgba = new Uint8ClampedArray([
    200, 255, 100, 255,
    0, 128, 128, 255,
    0, 255, 10, 255,
    1, 254, 128, 255,
  ]);
  const mask = Clipping.mask(rgba, 4, 1);
  assert.deepEqual([...mask.subarray(0, 4)], [...Clipping.highlight, 255]);
  assert.deepEqual([...mask.subarray(4, 8)], [...Clipping.shadow, 255]);
  assert.deepEqual([...mask.subarray(8, 12)], [...Clipping.highlight, 255]);
  assert.equal(mask[15], 0);
});

test("the divider moves with arrows, Shift, Home and End", () => {
  assert.equal(Divider.key(0.5, "ArrowRight", false), 0.52);
  assert.equal(Divider.key(0.5, "ArrowLeft", true), 0.4);
  assert.equal(Divider.key(0.99, "ArrowRight", false), 1);
  assert.equal(Divider.key(0.3, "Home", false), 0);
  assert.equal(Divider.key(0.3, "End", false), 1);
  assert.equal(Divider.key(0.3, "a", false), null);
});

test("fit develops the proxy only", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  await flush();
  assert.equal(host.count("develop_preview"), 1);
  assert.equal(host.count("develop_full"), 0);
  done(review, host.last("develop_preview"));
  await flush();
  assert.equal(view().after.frame.width, 24);
  assert.equal(view().proxy, true);
  assert.equal(host.count("develop_preview"), 1);
});

test("a refined image develops its fit preview again, once", async () => {
  const { host, review } = harness();
  review.open("img", settings());
  await flush();
  done(review, host.last("develop_preview"));
  await flush();
  assert.equal(host.count("develop_preview"), 1);
  review.refined({ imageId: "other" });
  await flush();
  assert.equal(host.count("develop_preview"), 1, "another image is ignored");
  review.refined({ imageId: "img" });
  await flush();
  assert.equal(host.count("develop_preview"), 2);
  done(review, host.last("develop_preview"));
  await flush();
  assert.equal(host.count("develop_preview"), 2);
  assert.equal(host.count("develop_full"), 0);
});

test("100% develops once, then panning sends no develop", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  await flush();
  done(review, host.last("develop_preview"));
  review.setZoom(1);
  await flush();
  assert.equal(host.count("develop_full"), 1);
  const full = host.last("develop_full");
  done(review, full);
  ready(review, full);
  await flush();
  assert.equal(view().after.exact, true);
  assert.equal(view().width, 48);
  const before = host.calls.length;
  for (let x = 0; x < 40; x += 1) {
    review.view({ x, y: 0, width: 8, height: 8 });
  }
  review.setZoom(2);
  review.setZoom(1);
  await flush();
  assert.equal(host.calls.length, before, "no command while panning a texture");
});

test("a settings change at 100% sends exactly one develop_full and keeps the old frame up", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  await flush();
  done(review, host.last("develop_preview"));
  review.setZoom(1);
  await flush();
  const first = host.last("develop_full");
  done(review, first);
  ready(review, first);
  await flush();
  const shown = view().after;

  review.update(settings(0.5));
  await flush();
  assert.equal(host.count("develop_full"), 2);
  assert.equal(view().after, shown, "the old frame stays up");
  assert.equal(view().updating, true);

  const second = host.last("develop_full");
  review.cacheEvicted({ imageId: "img", settingsHash: "hash-0", reason: "settingsChanged" });
  done(review, second);
  ready(review, second);
  await flush();
  assert.equal(host.count("develop_full"), 2);
  assert.notEqual(view().after, shown);
  assert.equal(view().updating, false);
});

test("cache:ready may arrive before job:done", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  review.setZoom(1);
  await flush();
  const full = host.last("develop_full");
  ready(review, full);
  done(review, full);
  await flush();
  assert.equal(view().after.exact, true);
  assert.equal(host.count("develop_full"), 1);
});

test("the before side waits for the after side and is developed with compare", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  await flush();
  done(review, host.last("develop_preview"));
  await flush();
  review.update(settings(1));
  review.setCompare(true);
  await flush();
  const afterCall = host.last("develop_preview");
  assert.equal(afterCall.compare, false);
  assert.equal(afterCall.settings.exposureEv, 1);
  done(review, afterCall);
  await flush();
  const beforeCall = host.last("develop_preview");
  assert.equal(beforeCall.compare, true);
  assert.equal(beforeCall.settings.exposureEv, 0, "before is the state at open");
  done(review, beforeCall);
  await flush();
  assert.notEqual(view().before, view().after);

  review.setZoom(1);
  await flush();
  const afterFull = host.last("develop_full");
  assert.equal(afterFull.settings.exposureEv, 1);
  done(review, afterFull);
  ready(review, afterFull);
  await flush();
  const beforeFull = host.last("develop_full");
  assert.equal(beforeFull.settings.exposureEv, 0);
  done(review, beforeFull);
  ready(review, beforeFull);
  await flush();
  assert.equal(view().before.exact, true);
  assert.equal(view().after.exact, true);
  assert.equal(host.count("develop_full"), 2);
});

test("with no edits, before reuses the after frames", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  review.setCompare(true);
  await flush();
  done(review, host.last("develop_preview"));
  await flush();
  assert.equal(host.count("develop_preview"), 1);
  const shown = view();
  assert.equal(shown.before, shown.after);
});

test("a frame over the budget pans through get_region, and a miss develops once", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  review.setZoom(1);
  await flush();
  const full = host.last("develop_full");
  done(review, full);
  ready(review, full, 8000, 6000);
  review.view({ x: 100, y: 100, width: 40, height: 20 });
  await flush();
  assert.equal(host.count("take_frame"), 0);
  assert.ok(host.count("get_region") >= 1);
  assert.equal(view().delivery, "region");
  assert.equal(view().after.exact, true);
  assert.deepEqual(host.last("get_region").x, 80);

  host.region = () => {
    throw { code: "cacheMiss", message: "gone" };
  };
  review.view({ x: 200, y: 100, width: 40, height: 20 });
  review.view({ x: 210, y: 100, width: 40, height: 20 });
  await flush();
  assert.equal(host.count("develop_full"), 2);
});

test("switching images starts over and before is per image", async () => {
  const { host, review, view } = harness();
  review.open("a", settings(0));
  await flush();
  review.update(settings(2));
  review.open("b", settings(1));
  await flush();
  assert.equal(view().imageId, "b");
  assert.equal(view().after, null);
  review.open("a", settings(2));
  review.setCompare(true);
  await flush();
  const last = host.last("develop_preview");
  assert.equal(last.imageId, "a");
  assert.equal(last.settings.exposureEv, 2);
});

test("a memory eviction drops the texture and develops for region delivery", async () => {
  const { host, review, view } = harness();
  review.open("img", settings());
  review.setZoom(1);
  await flush();
  const full = host.last("develop_full");
  done(review, full);
  ready(review, full);
  await flush();
  review.cacheEvicted({ imageId: "img", settingsHash: "hash-0", reason: "memoryBudget" });
  await flush();
  assert.equal(view().delivery, "region");
  assert.equal(host.count("develop_full"), 2);
});

test("zoomed drag previews develop the full frame once, on release", async () => {
  for (const zoom of [1, 2]) {
    const { host, review } = harness();
    review.adjustOwnsAfterPreview();
    review.open("img", settings(0), { width: 800, height: 600 });
    review.setZoom(zoom);
    await flush();
    const full = host.last("develop_full");
    done(review, full);
    ready(review, full);
    await flush();
    const during = host.count("develop_full");
    assert.equal(during, 1);

    let jobId = 20;
    for (const exposure of [0.1, 0.2, 0.3]) {
      review.holdPreview(settings(exposure), 720);
      host.jobs.set(jobId, { kind: "preview", settings: settings(exposure) });
      review.claimPreview(jobId);
      review.done({
        jobId,
        imageId: "img",
        kind: "preview",
        settingsHash: `hash-${exposure}`,
        width: 24,
        height: 16,
        exact: false,
      });
      await flush();
      assert.equal(host.count("develop_full"), during, `zoom ${zoom} exposure ${exposure}`);
      jobId += 1;
    }

    review.update(settings(0.3));
    await flush();
    assert.equal(host.count("develop_full"), during + 1, `zoom ${zoom}`);
    assert.equal(host.last("develop_full").settings.exposureEv, 0.3);
  }
});

test("settling a drag still develops the full frame once", async () => {
  const { host, review } = harness();
  review.adjustOwnsAfterPreview();
  review.open("img", settings(0), { width: 800, height: 600 });
  review.setZoom(1);
  await flush();
  assert.equal(host.count("develop_preview"), 0);
  assert.equal(host.count("develop_full"), 1);
  review.holdPreview(settings(0.5), 720);
  review.update(settings(0.5));
  await flush();
  assert.equal(host.count("develop_full"), 2);
  assert.equal(host.last("develop_full").settings.exposureEv, 0.5);
});

test("a preview replaced before it lands is discarded", async () => {
  const { host, review, view } = harness();
  review.adjustOwnsAfterPreview();
  review.open("img", settings(0));
  await flush();
  review.holdPreview(settings(0), 720);
  review.claimPreview(4);
  host.jobs.set(4, { kind: "preview", settings: settings(0) });
  review.holdPreview(settings(1), 1536);
  review.done({
    jobId: 4,
    imageId: "img",
    kind: "preview",
    settingsHash: "hash-0",
    width: 99,
    height: 99,
    exact: false,
  });
  await flush();
  assert.equal(view().after, null);
  assert.equal(host.count("take_frame"), 1);
});

test("split still develops the before image while adjust owns the after preview", async () => {
  const { host, review } = harness();
  review.adjustOwnsAfterPreview();
  review.open("img", settings(0));
  review.update(settings(1));
  review.setCompare(true);
  await flush();
  assert.equal(host.count("develop_preview"), 1);
  assert.equal(host.last("develop_preview").compare, true);
  assert.equal(host.last("develop_preview").settings.exposureEv, 0);
});

test("an adjust preview that finishes before its id is known is still shown", async () => {
  const { host, review, view } = harness();
  review.adjustOwnsAfterPreview();
  review.open("img", settings());
  await flush();
  assert.equal(host.count("develop_preview"), 0);
  review.holdPreview(settings(0.5), 720);
  review.done({
    jobId: 9,
    imageId: "img",
    kind: "preview",
    settingsHash: "hash-0.5",
    width: 24,
    height: 16,
    exact: false,
  });
  assert.equal(view().after, null);
  host.jobs.set(9, { kind: "preview", settings: settings(0.5) });
  review.claimPreview(9);
  await flush();
  assert.equal(view().after.frame.width, 24);
  assert.equal(view().after.frame.height, 16);
});

test("a failed develop is reported once and not retried", async () => {
  const { host, review, errors } = harness();
  review.open("img", settings());
  review.setZoom(1);
  await flush();
  const full = host.last("develop_full");
  review.failed({ jobId: full.jobId, imageId: "img", kind: "full", error: { code: "internal", message: "no" } });
  review.setZoom(2);
  await flush();
  assert.equal(errors.length, 1);
  assert.equal(host.count("develop_full"), 1);
});
