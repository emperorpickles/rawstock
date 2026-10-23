import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { SHORTCUTS, Shortcuts } from "./shortcuts.ts";

const press = (spec, platform) => ({
  key: spec.key,
  metaKey: spec.mod && platform === "mac",
  ctrlKey: spec.mod && platform !== "mac",
  shiftKey: Boolean(spec.shift) || spec.key === "?",
  altKey: false,
});

test("every table row matches its own action on its platforms", () => {
  for (const platform of ["mac", "windows", "other"]) {
    for (const spec of SHORTCUTS) {
      if (spec.platform && spec.platform !== platform) {
        continue;
      }
      assert.equal(Shortcuts.match(press(spec, platform), platform), spec.action, `${spec.key} on ${platform}`);
    }
  }
});

test("Ctrl+Y is redo on Windows only, and the other modifier never fires", () => {
  const ctrlY = { key: "y", metaKey: false, ctrlKey: true, shiftKey: false, altKey: false };
  assert.equal(Shortcuts.match(ctrlY, "windows"), "redo");
  assert.equal(Shortcuts.match(ctrlY, "other"), null);
  const ctrlO = { key: "o", metaKey: false, ctrlKey: true, shiftKey: false, altKey: false };
  assert.equal(Shortcuts.match(ctrlO, "mac"), null);
  assert.equal(Shortcuts.match({ ...ctrlO, altKey: true }, "windows"), null);
});

test("the cheat sheet shows Cmd on macOS and Ctrl on Windows", () => {
  const mac = Shortcuts.cheatSheet("mac");
  const windows = Shortcuts.cheatSheet("windows");
  const row = (rows, action) => rows.find((item) => item.action === action)?.keys;
  assert.equal(row(mac, "open-folder"), "Cmd+Shift+O");
  assert.equal(row(windows, "open-folder"), "Ctrl+Shift+O");
  assert.equal(row(mac, "redo"), "Cmd+Shift+Z");
  assert.equal(row(windows, "redo"), "Ctrl+Shift+Z or Ctrl+Y");
  assert.equal(row(mac, "clipping"), "J");
  assert.equal(row(mac, "cheat-sheet"), "?");
  assert.equal(row(windows, "filmstrip-next"), "Right Arrow");
  const actions = new Set(SHORTCUTS.map((spec) => spec.action));
  assert.deepEqual(new Set(mac.map((item) => item.action)), actions);
});

test("modifier shortcuts match the menu accelerators in menu.rs", () => {
  const source = readFileSync(new URL("../../src-tauri/src/menu.rs", import.meta.url), "utf8");
  const accelerators = new Map();
  for (const match of source.matchAll(/item\(\s*app,\s*"([^"]+)",\s*"[^"]*",\s*Some\("([^"]+)"\)/g)) {
    accelerators.set(match[1], match[2]);
  }
  assert.ok(accelerators.size > 5, "menu.rs accelerators were found");
  const expected = new Map();
  for (const spec of SHORTCUTS) {
    if (!spec.mod || spec.platform) {
      continue;
    }
    const key = spec.key.toUpperCase();
    expected.set(spec.menuId ?? spec.action, `CmdOrCtrl+${spec.shift ? "Shift+" : ""}${key}`);
  }
  assert.deepEqual(new Map([...accelerators].sort()), new Map([...expected].sort()));
});
