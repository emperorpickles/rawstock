import assert from "node:assert/strict";
import { test } from "node:test";

import { defaultSettings } from "./contract.ts";
import {
  Autosave,
  EditHistory,
  baselineSettings,
  filmstripBadges,
  filmstripLabel,
  invalidSidecarNotice,
  sortImages,
  versionWarning,
} from "./edits.ts";

const image = (fileName, extra = {}) => ({
  imageId: fileName,
  path: `/photos/${fileName}`,
  fileName,
  camera: { make: "Fujifilm", model: "X-T2" },
  support: "decodes",
  exif: {},
  width: 0,
  height: 0,
  sidecar: { status: "none" },
  ...extra,
});

test("photos sort by capture time, then file name", () => {
  const sorted = sortImages([
    image("b.dng"),
    image("a.RAF", { exif: { capturedAt: "2021-01-01T00:00:00" } }),
    image("c.dng", { exif: { capturedAt: "2020-05-01T00:00:00" } }),
    image("d.RAF", { exif: { capturedAt: "2020-05-01T00:00:00" } }),
  ]);
  assert.deepEqual(
    sorted.map((item) => item.fileName),
    ["c.dng", "d.RAF", "a.RAF", "b.dng"],
  );
});

test("filmstrip badges name edits, uncalibrated cameras, and failures", () => {
  assert.deepEqual(filmstripBadges(image("a.RAF")), ["Uncalibrated"]);
  assert.deepEqual(
    filmstripBadges(image("a.RAF", { sidecar: { status: "loaded", settings: defaultSettings() } })),
    ["Edited", "Uncalibrated"],
  );
  const unsupported = image("canon.raf", {
    support: "unsupported",
    error: {
      code: "unsupportedCamera",
      message: "canon.raf is from a Canon EOS R5, which rawstock cannot develop.",
    },
  });
  assert.deepEqual(filmstripBadges(unsupported), ["Unsupported", "Error"]);
  assert.match(filmstripLabel(unsupported), /EOS R5/);
  const corrupt = image("broken.dng", {
    support: "unsupported",
    error: { code: "corruptFile", message: "broken.dng is damaged or incomplete." },
  });
  assert.deepEqual(filmstripBadges(corrupt), ["Error"]);
  assert.deepEqual(
    filmstripBadges(image("a.RAF", { sidecar: { status: "invalid" } })),
    ["Edit unreadable", "Uncalibrated"],
  );
});

test("a stock revision mismatch uses the installed stock's name", () => {
  const settings = { ...defaultSettings(), stock: { id: "velvia", revision: 2 } };
  const stocks = [
    { id: "velvia", name: "Velvia", category: "fujiSim", model: "parametric", revision: 1 },
  ];
  assert.equal(
    versionWarning(settings, stocks),
    "Edited with Velvia rev 2; installed rev 1; the render may differ.",
  );
  assert.equal(versionWarning({ ...settings, stock: { id: "velvia", revision: 1 } }, stocks), null);
  assert.equal(versionWarning(settings, []), null);
});

test("an unreadable sidecar explains that the file was left alone", () => {
  assert.equal(
    invalidSidecarNotice("DSCF0001.RAF"),
    "DSCF0001.RAF has an edit file that could not be read. The photo opened with stock defaults, and that edit file was left unchanged.",
  );
});

test("a loaded sidecar is the baseline and a bad one is not", () => {
  const saved = { ...defaultSettings(), exposureEv: 0.5, stock: { id: "velvia", revision: 1 } };
  const loaded = baselineSettings(
    image("a.RAF", { sidecar: { status: "loaded", settings: saved } }),
    { id: "portra400", revision: 3 },
  );
  assert.equal(loaded.exposureEv, 0.5);
  assert.equal(loaded.stock.id, "velvia");
  const fresh = baselineSettings(image("b.RAF", { sidecar: { status: "invalid" } }), {
    id: "portra400",
    revision: 3,
  });
  assert.equal(fresh.exposureEv, 0);
  assert.equal(fresh.stock.id, "portra400");
});

test("a slider drag is one undo step and each photo keeps its own history", () => {
  const history = new EditHistory();
  const base = defaultSettings();
  history.open("a", base);
  history.open("b", { ...base, exposureEv: 1 });
  history.preview("a", { ...base, exposureEv: 0.2 });
  history.preview("a", { ...base, exposureEv: 0.4 });
  assert.equal(history.commit("a", { ...base, exposureEv: 0.4 }), true);
  assert.equal(history.commit("a", { ...base, exposureEv: 0.4 }), false);
  history.commit("b", { ...base, exposureEv: 2 });
  assert.equal(history.undo("a")?.exposureEv, 0);
  assert.equal(history.undo("a"), null);
  assert.equal(history.current("b")?.exposureEv, 2);
  assert.equal(history.redo("a")?.exposureEv, 0.4);
  history.preview("a", { ...base, exposureEv: 0.9 });
  assert.equal(history.undo("a")?.exposureEv, 0.4);
  assert.equal(history.current("b")?.exposureEv, 2);
});

test("autosave waits for a quiet period, flushes immediately, and skips a blocked file", async () => {
  const writes = [];
  let queued = null;
  const autosave = new Autosave(
    500,
    async (imageId, settings) => {
      writes.push([imageId, settings.exposureEv]);
    },
    (delay, run) => {
      assert.equal(delay, 500);
      queued = run;
      return () => {
        queued = null;
      };
    },
  );
  const base = defaultSettings();
  autosave.remember("a", base);
  autosave.schedule("a", base);
  assert.equal(queued, null);
  autosave.schedule("a", { ...base, exposureEv: 0.25 });
  autosave.schedule("a", { ...base, exposureEv: 0.5 });
  assert.equal(writes.length, 0);
  await autosave.flush();
  assert.deepEqual(writes, [["a", 0.5]]);
  assert.equal(queued, null);

  autosave.schedule("b", { ...base, exposureEv: 1 });
  assert.equal(typeof queued, "function");
  const fire = queued;
  queued = null;
  fire();
  await Promise.resolve();
  assert.deepEqual(writes, [
    ["a", 0.5],
    ["b", 1],
  ]);

  autosave.block("c", true);
  autosave.schedule("c", { ...base, exposureEv: 3 });
  assert.equal(queued, null);
  await autosave.flush();
  assert.equal(writes.length, 2);
});
