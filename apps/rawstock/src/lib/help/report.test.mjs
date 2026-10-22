import assert from "node:assert/strict";
import { test } from "node:test";

import { Report } from "./report.ts";

const info = {
  version: "0.0.1",
  commit: "d9b7b401b42d",
  buildDate: "2026-10-22",
  os: "windows",
  arch: "x86_64",
  logPath: "C:\\Users\\caleb\\AppData\\Roaming\\dev.rawstock.app\\logs\\rawstock.log",
};

test("the issue body carries build details and nothing private", () => {
  const body = Report.body(info);
  assert.match(body, /rawstock 0\.0\.1/);
  assert.match(body, /Commit: d9b7b401b42d/);
  assert.match(body, /Built: 2026-10-22/);
  assert.match(body, /OS: Windows \(x86_64\)/);
  assert.match(body, /Do not attach personal photos/);
  assert.ok(!body.includes(info.logPath));
  assert.ok(!body.includes("caleb"));
  assert.ok(!/[A-Za-z]:\\|\/Users\/|\/home\//.test(body));
  assert.ok(!/\.(raf|dng|jpe?g|tiff?)\b/i.test(body));
});

test("the issue URL prefills the title and body on the rawstock repo", () => {
  const url = new URL(Report.issueUrl(info));
  assert.equal(url.origin + url.pathname, "https://github.com/emperorpickles/rawstock/issues/new");
  assert.equal(url.searchParams.get("title"), "Problem in rawstock 0.0.1");
  assert.equal(url.searchParams.get("body"), Report.body(info));
});

test("copied details are four plain lines", () => {
  assert.equal(
    Report.details({ ...info, os: "macos", arch: "aarch64" }),
    "rawstock 0.0.1\nCommit: d9b7b401b42d\nBuilt: 2026-10-22\nOS: macOS (aarch64)",
  );
});
