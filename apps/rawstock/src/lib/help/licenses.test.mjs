import assert from "node:assert/strict";
import { test } from "node:test";

import { LicenseIndex } from "./licenses.ts";

const pkg = (name, license) => ({
  ecosystem: "cargo",
  name,
  version: "1.0.0",
  license,
  repository: null,
  texts: [],
});

const packages = [
  pkg("serde", "MIT OR Apache-2.0"),
  pkg("rawler", "LGPL-2.1"),
  pkg("toml", "MIT OR Apache-2.0"),
  pkg("selectors", "MPL-2.0"),
];

test("rawler leads and the rest group by license, largest first", () => {
  const groups = LicenseIndex.groups(packages);
  assert.deepEqual(
    groups.map((group) => [group.license, group.packages.map((item) => item.name)]),
    [
      ["LGPL-2.1", ["rawler"]],
      ["MIT OR Apache-2.0", ["serde", "toml"]],
      ["MPL-2.0", ["selectors"]],
    ],
  );
});

test("search matches the package name or the license", () => {
  assert.deepEqual(
    LicenseIndex.groups(packages, "TOML").map((group) => group.packages.map((item) => item.name)),
    [["toml"]],
  );
  assert.deepEqual(
    LicenseIndex.groups(packages, "mpl").map((group) => group.license),
    ["MPL-2.0"],
  );
  assert.deepEqual(LicenseIndex.groups(packages, "nothing"), []);
});
