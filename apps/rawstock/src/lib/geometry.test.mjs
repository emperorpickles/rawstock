import assert from "node:assert/strict";
import { test } from "node:test";

import { CropMath, Framing } from "./geometry.ts";

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
