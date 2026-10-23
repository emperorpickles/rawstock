import assert from "node:assert/strict";
import { test } from "node:test";

import { PreviewSchedule } from "./preview.ts";
import { Preferences } from "./preferences/settings.ts";

test("theme defaults to warm and survives a json round trip", () => {
  const defaults = Preferences.defaults();
  assert.equal(defaults.appearance.theme, "warm");
  const saved = JSON.parse(
    JSON.stringify({
      ...defaults,
      appearance: { ...defaults.appearance, theme: "neutral", surround: "mid" },
    }),
  );
  const loaded = Preferences.normalize(saved);
  assert.equal(loaded.appearance.theme, "neutral");
  assert.equal(loaded.appearance.surround, "mid");
  const again = JSON.parse(JSON.stringify(loaded));
  assert.equal(again.appearance.theme, "neutral");
  assert.equal(Preferences.normalize({ ...defaults, appearance: { ...defaults.appearance, theme: "sepia" } }).appearance.theme, "warm");
});

test("applying a theme sets data-theme and leaves the surround neutral", () => {
  const root = {
    dataset: {},
    style: {
      props: {},
      setProperty(name, value) {
        this.props[name] = value;
      },
    },
  };
  Preferences.applyAppearance(Preferences.defaults(), root);
  assert.equal(root.dataset.theme, "warm");
  assert.equal(root.style.props["--surround"], "#2e2e2e");
  const neutral = Preferences.normalize({
    ...Preferences.defaults(),
    appearance: { ...Preferences.defaults().appearance, theme: "neutral", surround: "mid" },
  });
  Preferences.applyAppearance(neutral, root);
  assert.equal(root.dataset.theme, "neutral");
  assert.equal(root.style.props["--surround"], "#7a7a7a");
  assert.equal(root.style.props["--surround-ink"], "#141414");
});

test("a drag asks for 720 and a release asks for the screen, with no queue", () => {
  assert.equal(PreviewSchedule.longEdge("drag", 1800), 720);
  assert.equal(PreviewSchedule.longEdge("settle", 1800.4), 1800);
  assert.equal(PreviewSchedule.longEdge("settle", 0), 720);
});
