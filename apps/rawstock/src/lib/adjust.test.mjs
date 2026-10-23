import assert from "node:assert/strict";
import { test } from "node:test";

import { defaultSettings } from "./contract.ts";
import { Adjustments } from "./adjust.ts";
import { CropMath, GeometryView } from "./geometry.ts";

const stock = (controls) => ({
  id: "sample",
  name: "Sample",
  category: "colorNegative",
  model: "spectral",
  revision: 1,
  controls,
});

test("dye, contrast, and grain stay hidden unless the stock or the sidecar has them", () => {
  const settings = defaultSettings();
  const parametric = stock({ dye: false, contrast: false, grain: false });
  assert.equal(Adjustments.showDye(parametric, settings), false);
  assert.equal(Adjustments.showContrast(parametric, settings), false);
  assert.equal(Adjustments.showGrain(parametric, settings), false);
  const spectral = stock({ dye: true, contrast: false, grain: true });
  assert.equal(Adjustments.showDye(spectral, settings), true);
  assert.equal(Adjustments.showContrast(spectral, settings), false);
  assert.equal(Adjustments.showGrain(spectral, settings), true);
  const saved = {
    ...settings,
    dye: { scales: [1.2, 1, 1] },
    contrast: 1.4,
    grain: { amount: 1.2, size: 1 },
  };
  assert.equal(Adjustments.showDye(parametric, saved), true);
  assert.equal(Adjustments.showContrast(parametric, saved), true);
  assert.equal(Adjustments.showGrain(parametric, saved), true);
});

test("editing exposure keeps a custom white balance for the sidecar", () => {
  const custom = {
    ...defaultSettings(),
    whiteBalance: { mode: "custom", temperatureK: 5200, tint: -3 },
  };
  const next = Adjustments.exposure(custom, 0.4);
  assert.equal(next.exposureEv, 0.4);
  assert.deepEqual(next.whiteBalance, custom.whiteBalance);
  assert.equal(next.lens.caCorrection, true);
});

test("dye and contrast omit the stock default, and grain size rejects zero", () => {
  const settings = defaultSettings();
  const dyed = Adjustments.dyeChannel(settings, 0, 1.2);
  assert.deepEqual(dyed.dye?.scales, [1.2, 1, 1]);
  assert.equal(Adjustments.dyeChannel(dyed, 0, 1).dye, undefined);
  assert.equal(Adjustments.contrast(settings, 1.2).contrast, 1.2);
  assert.equal(Adjustments.contrast(settings, 1).contrast, undefined);
  assert.equal(Adjustments.grainSize(settings, 0), settings);
  assert.equal(Adjustments.grainSize(settings, 1.5).grain.size, 1.5);
});

test("an unknown scan look stays selectable and can return to the stock default", () => {
  const options = Adjustments.scanOptions("lab-42");
  assert.equal(options.at(-1)?.id, "lab-42");
  assert.equal(Adjustments.scanOptions("default").some((item) => item.id === "lab-42"), false);
  const next = Adjustments.scan(defaultSettings(), "flat");
  assert.equal(next.scan.look, "flat");
});

test("rotate turns the crop with the photo", () => {
  const settings = {
    ...defaultSettings(),
    geometry: { quarterTurns: 0, straightenDeg: 2, crop: { x: 0, y: 0, w: 0.5, h: 1 } },
  };
  const turned = Adjustments.rotate(settings, 1);
  assert.equal(turned.geometry.quarterTurns, 1);
  assert.deepEqual(turned.geometry.crop, { x: 0, y: 0.5, w: 1, h: 0.5 });
  assert.equal(turned.geometry.straightenDeg, 2);
  const back = Adjustments.rotate(turned, -1);
  assert.equal(back.geometry.quarterTurns, 0);
  assert.deepEqual(back.geometry.crop, settings.geometry.crop);
});

test("crop aspects, a free start, and a drawn horizon", () => {
  const square = CropMath.maximum(1, 3, 2);
  assert.ok(Math.abs(square.w * 3 - square.h * 2) < 1e-9);
  assert.equal(CropMath.infer(undefined, 3, 2), "original");
  assert.equal(CropMath.infer(square, 3, 2), "1:1");
  assert.equal(CropMath.infer(CropMath.freeStart(), 3, 2), "free");
  const framed = Adjustments.aspect(defaultSettings(), "1:1", 3, 2);
  assert.equal(CropMath.infer(framed.geometry.crop, 3, 2), "1:1");
  assert.equal(Adjustments.aspect(framed, "original", 3, 2).geometry.crop, undefined);
  assert.equal(CropMath.horizon(0, 0, 40, 0), 0);
  assert.ok(Math.abs((CropMath.horizon(0, 0, 100, 17.6) ?? 0) + 10) < 0.2);
  assert.equal(CropMath.horizon(0, 0, 2, 0), null);
  assert.equal(CropMath.horizon(0, 0, 10, 100), -45);
  const moved = CropMath.drag(square, "move", 0.1, 0, null, 3, 2);
  assert.ok(moved.x > square.x);
  assert.equal(moved.w, square.w);
  const fitted = GeometryView.fit(300, 200, 300, 300);
  assert.equal(fitted.width, 300);
  assert.equal(fitted.height, 200);
  assert.deepEqual(GeometryView.orientedSize(300, 200, 1), { width: 200, height: 300 });
});
