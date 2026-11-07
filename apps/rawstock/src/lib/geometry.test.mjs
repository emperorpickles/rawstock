import assert from "node:assert/strict";
import { test } from "node:test";

import { CropMath, CropMode, Framing } from "./geometry.ts";

const close = (actual, expected, tolerance = 1e-6) =>
  assert.ok(Math.abs(actual - expected) <= tolerance, `${actual} is not ${expected}`);

test("cover scale matches rawstock-core's framing", () => {
  assert.equal(Framing.coverScale(0, 6000, 4000), 1);
  // The same value is checked in crates/rawstock-core/src/geometry.rs.
  close(Framing.coverScale(10, 6000, 4000), 1.24528, 1e-5);
  assert.equal(Framing.coverScale(-10, 6000, 4000), Framing.coverScale(10, 6000, 4000));
});

test("a clockwise quarter turn puts the bottom-left corner top-left", () => {
  const map = Framing.imageToFrame(40, 20, 1, 0);
  const corner = Framing.apply(map, { x: 0, y: 20 });
  close(corner.x, 0);
  close(corner.y, 0);
  const far = Framing.apply(map, { x: 40, y: 0 });
  close(far.x, 20);
  close(far.y, 40);
});

test("a straightened frame reads only inside the image, and inverts", () => {
  for (const turns of [0, 1, 2, 3]) {
    const map = Framing.imageToFrame(600, 400, turns, -7.5);
    const back = Framing.invert(map);
    const frame = turns % 2 ? { width: 400, height: 600 } : { width: 600, height: 400 };
    for (const corner of [
      { x: 0, y: 0 },
      { x: frame.width, y: 0 },
      { x: 0, y: frame.height },
      { x: frame.width, y: frame.height },
    ]) {
      const source = Framing.apply(back, corner);
      assert.ok(source.x >= -1e-9 && source.x <= 600 + 1e-9, `x ${source.x}`);
      assert.ok(source.y >= -1e-9 && source.y <= 400 + 1e-9, `y ${source.y}`);
      const again = Framing.apply(map, source);
      close(again.x, corner.x);
      close(again.y, corner.y);
    }
  }
});

test("a positive angle turns the photo clockwise about the frame centre", () => {
  const map = Framing.imageToFrame(200, 100, 0, 90);
  // Covering at 90 degrees scales the photo; the right edge's middle moves down.
  const point = Framing.apply(map, { x: 150, y: 50 });
  close(point.x, 100);
  assert.ok(point.y > 50);
});

test("the region for a 100% view covers every pixel the frame shows", () => {
  const map = Framing.imageToFrame(6000, 4000, 1, 3);
  const bounds = Framing.imageBounds(map, { x: 1000, y: 2000, width: 800, height: 600 }, 6000, 4000);
  const back = Framing.invert(map);
  for (const [x, y] of [
    [1000, 2000],
    [1800, 2000],
    [1000, 2600],
    [1800, 2600],
    [1400, 2300],
  ]) {
    const source = Framing.apply(back, { x, y });
    assert.ok(source.x >= bounds.x && source.x <= bounds.x + bounds.width);
    assert.ok(source.y >= bounds.y && source.y <= bounds.y + bounds.height);
  }
  const whole = Framing.imageBounds(Framing.imageToFrame(60, 40, 0, 0), { x: 0, y: 0, width: 60, height: 40 }, 60, 40);
  assert.deepEqual(whole, { x: 0, y: 0, width: 60, height: 40 });
});

test("a horizon drawn on a straightened photo adds to its angle", () => {
  assert.equal(CropMath.levelled(2, -1.5), 0.5);
  assert.equal(CropMath.levelled(40, 10), 45);
  assert.equal(CropMath.levelled(-1, 1), 0);
});

test("the cropped view rounds the crop outward like rawstock-core", () => {
  // The same cases as crop_rounds_outward_in_the_turned_frame.
  assert.deepEqual(Framing.cropPixels({ x: 0.5, y: 0.5, w: 0.5, h: 0.5 }, 4, 2), { x: 2, y: 1, width: 2, height: 1 });
  assert.deepEqual(Framing.cropPixels({ x: 0, y: 0, w: 0.5, h: 0.5 }, 6, 8), { x: 0, y: 0, width: 3, height: 4 });
  assert.deepEqual(Framing.cropPixels(undefined, 6, 8), { x: 0, y: 0, width: 6, height: 8 });
  assert.deepEqual(Framing.cropPixels({ x: 0.999, y: 0, w: 0.001, h: 1 }, 10, 10), { x: 9, y: 0, width: 1, height: 10 });
});

test("the cropped map puts the crop's corner at the origin", () => {
  const map = Framing.imageToFrame(600, 400, 1, 4);
  const crop = Framing.cropPixels({ x: 0.25, y: 0.1, w: 0.5, h: 0.5 }, 400, 600);
  const inner = Framing.cropped(map, crop);
  const source = Framing.apply(Framing.invert(map), { x: crop.x, y: crop.y });
  const corner = Framing.apply(inner, source);
  close(corner.x, 0);
  close(corner.y, 0);
});

test("recentering keeps the same point of the photo in the middle", () => {
  const full = { x: 0, y: 0, width: 600, height: 400 };
  const crop = { x: 150, y: 100, width: 300, height: 200 };
  const inCrop = Framing.recenter({ x: 0.5, y: 0.5 }, full, crop);
  close(inCrop.x, 0.5);
  close(inCrop.y, 0.5);
  const back = Framing.recenter({ x: 0, y: 1 }, crop, full);
  close(back.x, 0.25);
  close(back.y, 0.75);
});

test("crop mode closes on Enter and Esc, but not from fields or buttons", () => {
  const key = (name, extra = {}) => ({ key: name, metaKey: false, ctrlKey: false, altKey: false, ...extra });
  assert.equal(CropMode.isExitKey(key("Enter"), "BODY"), true);
  assert.equal(CropMode.isExitKey(key("Escape"), "SPAN"), true);
  assert.equal(CropMode.isExitKey(key("Enter"), "BUTTON"), false);
  assert.equal(CropMode.isExitKey(key("Escape"), "BUTTON"), true);
  assert.equal(CropMode.isExitKey(key("Escape"), "INPUT"), false);
  assert.equal(CropMode.isExitKey(key("Enter", { metaKey: true }), "BODY"), false);
  assert.equal(CropMode.isExitKey(key("r"), "BODY"), false);
});

test("crop mode compares geometry by value", () => {
  const a = { quarterTurns: 1, straightenDeg: 2, crop: { x: 0.1, y: 0.2, w: 0.5, h: 0.5 } };
  assert.equal(CropMode.sameGeometry(a, { ...a, crop: { ...a.crop } }), true);
  assert.equal(CropMode.sameGeometry(a, { ...a, crop: undefined }), false);
  assert.equal(CropMode.sameGeometry(a, { ...a, straightenDeg: 0 }), false);
});
