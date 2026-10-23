import assert from "node:assert/strict";
import { test } from "node:test";

import { defaultSettings } from "./contract.ts";
import { ExportPlan } from "./export.ts";
import { Preferences } from "./preferences/settings.ts";

const raw = "/photos/DSCF0001.RAF";

test("the default file is the stem as a jpeg, in the last folder or beside the photo", () => {
  assert.equal(ExportPlan.jpegName(raw), "DSCF0001.jpg");
  assert.equal(ExportPlan.jpegName("C:\\roll\\IMG_0001.DNG"), "IMG_0001.jpg");
  assert.equal(ExportPlan.jpegName("/photos/My.Photo.RAF"), "My.Photo.jpg");
  assert.equal(ExportPlan.defaultPath(raw, null), "/photos/DSCF0001.jpg");
  assert.equal(ExportPlan.defaultPath(raw, "/exports"), "/exports/DSCF0001.jpg");
  assert.equal(ExportPlan.defaultPath(raw, "/exports/"), "/exports/DSCF0001.jpg");
  assert.equal(ExportPlan.defaultPath("C:\\roll\\IMG_0001.DNG", "D:\\out"), "D:\\out\\IMG_0001.jpg");
  assert.equal(ExportPlan.defaultPath("C:\\DSCF0001.RAF", null), "C:\\DSCF0001.jpg");
});

test("overwrite needs confirmation, and the raw and sidecar paths are refused", () => {
  const jpeg = "/photos/DSCF0001.jpg";
  const ready = ExportPlan.review(raw, jpeg, "missing");
  assert.equal(ready.action, "export");
  assert.equal(ready.path, jpeg);

  const replace = ExportPlan.review(raw, jpeg, "file");
  assert.equal(replace.action, "confirm");
  assert.equal(
    replace.message,
    "DSCF0001.jpg already exists. Replace it? The RAW photo is not changed.",
  );
  assert.equal(ExportPlan.isReplacePrompt(replace.message), true);

  const ontoRaw = ExportPlan.review(raw, raw, "file");
  assert.equal(ontoRaw.action, "refuse");
  assert.match(ontoRaw.message, /cannot replace DSCF0001\.RAF, the RAW photo/);

  const caseAndSlashes = ExportPlan.review(raw, "\\photos\\dscf0001.raf", "file");
  assert.equal(caseAndSlashes.action, "refuse");

  const dotted = ExportPlan.review(raw, "/photos/roll/../DSCF0001.RAF", "missing");
  assert.equal(dotted.action, "refuse");

  const sidecar = ExportPlan.review(raw, "/photos/DSCF0001.RAF.rawstock.toml", "missing");
  assert.equal(sidecar.action, "refuse");
  assert.match(sidecar.message, /edit saved beside the photo/);

  const otherFolder = ExportPlan.review(raw, "/other/DSCF0001.RAF", "missing");
  assert.equal(otherFolder.action, "export");
  assert.equal(otherFolder.path, "/other/DSCF0001.RAF.jpg");

  const added = ExportPlan.review(raw, "/photos/DSCF0001", "missing");
  assert.equal(added.action, "export");
  assert.equal(added.path, "/photos/DSCF0001.jpg");

  const kept = ExportPlan.review(raw, "/photos/DSCF0001.JPEG", "file");
  assert.equal(kept.action, "confirm");
  assert.equal(kept.path, "/photos/DSCF0001.JPEG");

  assert.equal(ExportPlan.review(raw, "  ", "missing").action, "refuse");
  assert.equal(ExportPlan.review(raw, "/photos/out", "directory").action, "refuse");
  assert.match(ExportPlan.review(raw, "/photos/out", "directory").message, /folder/);
});

test("the export request keeps the whole edit, including crop, straighten, and rotate", () => {
  const settings = {
    ...defaultSettings(),
    exposureEv: 0.3,
    stock: { id: "portra-400", revision: 3 },
    geometry: {
      quarterTurns: 1,
      straightenDeg: -1.25,
      crop: { x: 0.08, y: 0.1, w: 0.84, h: 0.8 },
    },
  };
  const request = ExportPlan.request("img-1", settings, 92.4, "/photos/DSCF0001.jpg", false);
  assert.equal(request.imageId, "img-1");
  assert.deepEqual(request.settings, settings);
  assert.deepEqual(request.settings.geometry, settings.geometry);
  assert.equal(request.settings.exposureEv, 0.3);
  assert.deepEqual(request.options, {
    format: "jpeg",
    quality: 92,
    path: "/photos/DSCF0001.jpg",
    overwrite: false,
  });
  const confirmed = ExportPlan.request("img-1", settings, 200, "/photos/DSCF0001.jpg", true);
  assert.equal(confirmed.options.quality, 100);
  assert.equal(confirmed.options.overwrite, true);
  assert.equal(ExportPlan.quality(Number.NaN), 92);
  settings.geometry.quarterTurns = 0;
  assert.equal(request.settings.geometry.quarterTurns, 1);
});

test("the last folder and quality are what the next export opens with", () => {
  const saved = ExportPlan.remembered(Preferences.defaults(), 80, "/exports/DSCF0001.jpg");
  assert.equal(saved.export.quality, 80);
  assert.equal(saved.export.lastFolder, "/exports");
  assert.equal(saved.appearance.theme, "warm");
  const loaded = Preferences.normalize({
    ...Preferences.defaults(),
    export: { quality: 80, lastFolder: "/exports" },
  });
  assert.equal(loaded.export.quality, 80);
  assert.equal(loaded.export.lastFolder, "/exports");
  const broken = Preferences.normalize({
    ...Preferences.defaults(),
    export: { quality: 0, lastFolder: "   " },
  });
  assert.equal(broken.export.quality, 92);
  assert.equal(broken.export.lastFolder, null);
  assert.equal(ExportPlan.defaultPath(raw, loaded.export.lastFolder), "/exports/DSCF0001.jpg");
});

test("progress names the stage, and a finished job does not claim a file was written", () => {
  assert.equal(ExportPlan.progressText("decode", 0, 6), "Decoding 1/6");
  assert.equal(ExportPlan.progressText("write", 5, 6), "Writing 6/6");
  assert.equal(ExportPlan.progressText("queued", 0, 0), "Queued");
  const notice = ExportPlan.finishedNotice("/photos/DSCF0001.jpg");
  assert.match(notice, /DSCF0001\.jpg/);
  assert.match(notice, /not written/);
  assert.equal(ExportPlan.cancelledNotice(), "Export cancelled. No file was written.");
});
