// Writes src/generated/licenses.json for Help > Licenses: every crate in the
// rawstock-app dependency graph (from `cargo metadata`) and every npm package
// that ships in the bundle, each with its license text. Exits non-zero when a
// license is not GPL-3.0 compatible, so `npm run build` and CI catch it.
//
// Runs from `npm run build` and `npm run dev`. Needs only node and cargo.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Spdx } from "./spdx.mjs";

const APP = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = resolve(APP, "../..");
const OUT = join(APP, "src/generated/licenses.json");
const LICENSE_FILE = /^(licen[cs]e|copying|notice|unlicense)([-._].*)?$/i;
// The Svelte runtime is compiled into the bundle even though svelte is a
// dev dependency.
const BUNDLED_DEV = new Set(["svelte"]);

class LicenseBundle {
  constructor() {
    this.texts = new Map();
    this.packages = [];
    this.violations = [];
    // A standard text per SPDX id, borrowed from a package that ships one,
    // for packages that publish no license file.
    this.reference = new Map();
  }

  text(body) {
    const id = createHash("sha256").update(body).digest("hex").slice(0, 16);
    this.texts.set(id, body);
    return id;
  }

  filesIn(dir) {
    if (!dir || !existsSync(dir)) {
      return [];
    }
    return readdirSync(dir)
      .filter((name) => LICENSE_FILE.test(name))
      .filter((name) => statSync(join(dir, name)).isFile())
      .sort()
      .map((name) => ({
        name,
        id: this.text(readFileSync(join(dir, name), "utf8").replace(/\r\n/g, "\n").trim()),
      }));
  }

  remember(license, files) {
    const single = /^[A-Za-z0-9.+-]+$/.test(license ?? "") ? license : null;
    for (const file of files) {
      const named = /apache/i.test(file.name) ? "Apache-2.0" : /mit/i.test(file.name) ? "MIT" : null;
      const id = named ?? (files.length === 1 ? single : null);
      if (id && !this.reference.has(id)) {
        this.reference.set(id, file.id);
      }
    }
  }

  /** Fill in a standard text for packages that publish no license file. */
  borrow() {
    for (const pkg of this.packages) {
      if (pkg.texts.length) {
        continue;
      }
      // Prefer texts without a copyright holder, then strip the borrowed
      // holder from the rest so nobody is credited for another package.
      const preferred = ["Apache-2.0", "MPL-2.0", "LGPL-2.1", "MIT"];
      const ids = Spdx.tokens(pkg.license ?? "").filter(
        (token) => this.reference.has(token) && Spdx.allowed(token),
      );
      const id = preferred.find((token) => ids.includes(token)) ?? ids[0];
      if (!id) {
        continue;
      }
      const body = this.texts
        .get(this.reference.get(id))
        .replace(/^\s*(copyright\b|\(c\)).*$/gim, "")
        .replace(/\n{3,}/g, "\n\n")
        .trim();
      pkg.texts = [this.text(body)];
      pkg.note =
        `${pkg.name} publishes no license file. It is used here under ${id}, and the ` +
        `standard ${id} text is shown. Copyright belongs to the ${pkg.name} authors.`;
    }
  }

  add(entry, dir) {
    let ok = false;
    try {
      ok = Spdx.allowed(entry.license);
    } catch (error) {
      this.violations.push(`${entry.name} ${entry.version}: ${error.message}`);
      return;
    }
    if (!ok) {
      this.violations.push(
        `${entry.name} ${entry.version} (${entry.ecosystem}): "${entry.license ?? "no license field"}" is not on the GPL-3.0-compatible allowlist`,
      );
    }
    const files = this.filesIn(dir);
    this.remember(entry.license, files);
    this.packages.push({ ...entry, texts: files.map((file) => file.id) });
  }

  cargo() {
    const raw = execFileSync(
      "cargo",
      ["metadata", "--format-version", "1", "--manifest-path", join(APP, "src-tauri/Cargo.toml")],
      { cwd: APP, encoding: "utf8", maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "inherit"] },
    );
    const metadata = JSON.parse(raw);
    const packages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
    const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
    const members = new Set(metadata.workspace_members);
    const root = metadata.packages.find((pkg) => pkg.name === "rawstock-app");
    if (!root) {
      throw new Error("cargo metadata did not list rawstock-app");
    }
    // Normal and build dependencies, on every target, since the app ships
    // on macOS and Windows. Dev dependencies never ship.
    const seen = new Set();
    const stack = [root.id];
    while (stack.length) {
      const id = stack.pop();
      if (seen.has(id)) {
        continue;
      }
      seen.add(id);
      for (const dep of nodes.get(id)?.deps ?? []) {
        if (dep.dep_kinds.some((kind) => kind.kind === null || kind.kind === "build")) {
          stack.push(dep.pkg);
        }
      }
    }
    for (const id of seen) {
      if (members.has(id)) {
        continue;
      }
      const pkg = packages.get(id);
      const entry = {
        ecosystem: "cargo",
        name: pkg.name,
        version: pkg.version,
        license: pkg.license,
        repository: pkg.repository ?? null,
      };
      if (pkg.name === "rawler") {
        entry.note =
          `rawler is linked under ${pkg.license}. Its complete source is at ` +
          `${pkg.repository ?? "https://github.com/dnglab/dnglab"} and on crates.io as ` +
          `https://crates.io/crates/rawler/${pkg.version}. To use a modified rawler, ` +
          "rebuild rawstock from its source at https://github.com/emperorpickles/rawstock.";
      }
      this.add(entry, dirname(pkg.manifest_path));
    }
  }

  npm() {
    const lock = JSON.parse(readFileSync(join(APP, "package-lock.json"), "utf8"));
    for (const [path, pkg] of Object.entries(lock.packages ?? {})) {
      if (!path.startsWith("node_modules/")) {
        continue;
      }
      const name = path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
      if (pkg.dev && !BUNDLED_DEV.has(name)) {
        continue;
      }
      const dir = join(APP, path);
      const manifest = existsSync(join(dir, "package.json"))
        ? JSON.parse(readFileSync(join(dir, "package.json"), "utf8"))
        : {};
      const repository = manifest.repository?.url ?? manifest.repository ?? null;
      this.add(
        {
          ecosystem: "npm",
          name,
          version: pkg.version,
          license: pkg.license ?? manifest.license ?? null,
          repository: typeof repository === "string" ? repository : null,
        },
        dir,
      );
    }
  }

  json() {
    const rank = (pkg) => (pkg.name === "rawler" ? 0 : 1);
    const packages = [...this.packages].sort(
      (a, b) =>
        rank(a) - rank(b) ||
        a.name.localeCompare(b.name) ||
        a.version.localeCompare(b.version, undefined, { numeric: true }),
    );
    const rawstock = {
      name: "rawstock",
      license: "GPL-3.0-or-later",
      text: this.text(readFileSync(join(REPO, "LICENSE"), "utf8").replace(/\r\n/g, "\n").trim()),
    };
    return JSON.stringify({ rawstock, packages, texts: Object.fromEntries(this.texts) });
  }
}

const bundle = new LicenseBundle();
bundle.cargo();
bundle.npm();
bundle.borrow();
if (bundle.violations.length) {
  console.error("Licenses that are not GPL-3.0 compatible:");
  for (const line of bundle.violations) {
    console.error(`  ${line}`);
  }
  process.exit(1);
}
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, bundle.json());
const withoutText = bundle.packages.filter((pkg) => pkg.texts.length === 0).length;
console.log(
  `licenses: ${bundle.packages.length} packages, ${bundle.texts.size} distinct texts, ` +
    `${withoutText} without a license file -> src/generated/licenses.json`,
);
