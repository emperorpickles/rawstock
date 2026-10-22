import assert from "node:assert/strict";
import { test } from "node:test";

import { Spdx } from "./spdx.mjs";

test("single allowed licenses pass and others fail", () => {
  assert.equal(Spdx.allowed("MIT"), true);
  assert.equal(Spdx.allowed("LGPL-2.1"), true);
  assert.equal(Spdx.allowed("GPL-2.0-only"), false);
  assert.equal(Spdx.allowed("SSPL-1.0"), false);
  assert.equal(Spdx.allowed(""), false);
  assert.equal(Spdx.allowed(null), false);
});

test("OR needs one side, AND needs both", () => {
  assert.equal(Spdx.allowed("MIT OR Apache-2.0"), true);
  assert.equal(Spdx.allowed("SSPL-1.0 OR MIT"), true);
  assert.equal(Spdx.allowed("Apache-2.0 AND MIT"), true);
  assert.equal(Spdx.allowed("MIT AND SSPL-1.0"), false);
  assert.equal(Spdx.allowed("(MIT OR Apache-2.0) AND Unicode-3.0"), true);
  assert.equal(Spdx.allowed("(MIT OR Apache-2.0) AND SSPL-1.0"), false);
});

test("WITH is judged on the license and slash means OR", () => {
  assert.equal(Spdx.allowed("Apache-2.0 WITH LLVM-exception"), true);
  assert.equal(Spdx.allowed("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"), true);
  assert.equal(Spdx.allowed("MIT/Apache-2.0"), true);
  assert.equal(Spdx.allowed("Apache-2.0 / MIT"), true);
  assert.equal(Spdx.allowed("Unlicense/MIT"), true);
  assert.equal(Spdx.allowed("Unlicense"), false);
});

test("malformed expressions throw", () => {
  assert.throws(() => Spdx.allowed("(MIT OR Apache-2.0"));
  assert.throws(() => Spdx.allowed("MIT Apache-2.0"));
});
