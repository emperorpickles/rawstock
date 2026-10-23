import assert from "node:assert/strict";
import { test } from "node:test";

import { defaultSettings } from "./contract.ts";
import { EditHistory } from "./edits.ts";
import { SliderGesture, SliderMath } from "./slider.ts";
import { Adjustments } from "./adjust.ts";

const exposure = Adjustments.exposureScale;

test("an out-of-range file value widens the track and still formats", () => {
  const bounds = SliderMath.bounds(exposure, 4.5);
  assert.deepEqual(bounds, { min: -3, max: 4.5 });
  assert.equal(SliderMath.format(4.5, exposure), "+4.50 EV");
  assert.equal(SliderMath.format(-0.5, exposure), "-0.50 EV");
  assert.equal(SliderMath.ariaText(0.3, exposure), "+0.30 EV");
});

test("arrow keys step, shift is coarse, alt is fine, and reset restores the default", () => {
  assert.equal(SliderMath.nudge(0, exposure, 1, {}), 0.1);
  assert.equal(SliderMath.nudge(0, exposure, -1, {}), -0.1);
  assert.equal(SliderMath.nudge(0, exposure, 1, { shift: true }), 1);
  assert.equal(SliderMath.nudge(0, exposure, 1, { alt: true }), 0.01);
  assert.equal(SliderMath.nudge(0, exposure, 1, { shift: true, alt: true }), 0.01);
  assert.equal(SliderMath.nudge(3, exposure, 1, {}), 3);
  assert.equal(SliderMath.nudge(4.5, exposure, 1, {}), 4.5);
  assert.equal(SliderMath.nudge(4.5, exposure, -1, {}), 4.4);
  assert.equal(SliderMath.reset(exposure), 0);
});

test("a step that would pass the detent lands on it, and a drag snaps", () => {
  assert.equal(SliderMath.nudge(-0.05, exposure, 1, {}), 0);
  assert.equal(SliderMath.nudge(0.05, exposure, -1, {}), 0);
  assert.equal(SliderMath.nudge(-0.5, exposure, 1, { shift: true }), 0);
  const mid = SliderMath.fromFraction(0.5, exposure, 0);
  assert.equal(mid, 0);
  const near = SliderMath.fromFraction(0.5 + 0.02 / 6, exposure, 0);
  assert.equal(near, 0);
  const away = SliderMath.fromFraction(0.75, exposure, 0);
  assert.equal(away, 1.5);
});

test("typed readouts accept a sign and a unit", () => {
  assert.equal(SliderMath.parse("+1.25 EV"), 1.25);
  assert.equal(SliderMath.parse("-0.50 EV"), -0.5);
  assert.equal(SliderMath.parse("1.50×"), 1.5);
  assert.equal(SliderMath.parse("nope"), null);
  assert.equal(SliderMath.parse(""), null);
});

test("major ticks land on whole stops and minor ticks skip them", () => {
  assert.deepEqual(SliderMath.ticks(-3, 3, 1), [-3, -2, -1, 0, 1, 2, 3]);
  const minor = SliderMath.ticks(-3, 3, 0.5).filter(
    (tick) => !SliderMath.ticks(-3, 3, 1).some((major) => Math.abs(major - tick) < 1e-6),
  );
  assert.deepEqual(minor, [-2.5, -1.5, -0.5, 0.5, 1.5, 2.5]);
});

test("a drag previews many values and commits one undo step", () => {
  const history = new EditHistory();
  const initial = defaultSettings();
  history.open("a", initial);
  const gesture = new SliderGesture(initial.exposureEv);
  for (const value of [0.1, 0.2, 0.4]) {
    const next = gesture.preview(SliderMath.quantize(value, exposure.fine));
    history.preview("a", { ...initial, exposureEv: next });
  }
  const end = gesture.end();
  assert.equal(end, 0.4);
  assert.equal(history.commit("a", { ...initial, exposureEv: end }), true);
  const undone = history.undo("a");
  assert.equal(undone?.exposureEv, 0);
  assert.equal(history.undo("a"), null);
});

test("a drag that returns to the start does not commit", () => {
  const gesture = new SliderGesture(0);
  gesture.preview(0.2);
  gesture.preview(0);
  assert.equal(gesture.end(), null);
});

test("bipolar fill grows from the detent", () => {
  const negative = SliderMath.fill(-1, exposure);
  assert.ok(Math.abs(negative.end - 0.5) < 1e-9);
  assert.ok(negative.start < negative.end);
  const positive = SliderMath.fill(1.5, exposure);
  assert.ok(Math.abs(positive.start - 0.5) < 1e-9);
  assert.ok(positive.end > positive.start);
});
