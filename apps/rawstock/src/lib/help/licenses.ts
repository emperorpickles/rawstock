// Shape of `src/generated/licenses.json` (written by scripts/licenses.mjs)
// and the grouping and search the Licenses view uses.

export interface LicensePackage {
  ecosystem: "cargo" | "npm";
  name: string;
  version: string;
  license: string | null;
  repository: string | null;
  note?: string;
  /** Keys into `LicenseData.texts`. */
  texts: string[];
}

export interface LicenseData {
  rawstock: { name: string; license: string; text: string };
  packages: LicensePackage[];
  texts: Record<string, string>;
}

export interface LicenseGroup {
  license: string;
  packages: LicensePackage[];
}

export class LicenseIndex {
  /**
   * Groups by license expression, largest group first. rawler always leads,
   * in its own group, so its LGPL note is the first thing after rawstock.
   */
  static groups(packages: LicensePackage[], query = ""): LicenseGroup[] {
    const needle = query.trim().toLowerCase();
    const matches = packages.filter(
      (pkg) =>
        !needle ||
        pkg.name.toLowerCase().includes(needle) ||
        (pkg.license ?? "").toLowerCase().includes(needle),
    );
    const lead = matches.filter((pkg) => pkg.name === "rawler");
    const byLicense = new Map<string, LicensePackage[]>();
    for (const pkg of matches) {
      if (pkg.name === "rawler") {
        continue;
      }
      const key = pkg.license ?? "Unknown";
      byLicense.set(key, [...(byLicense.get(key) ?? []), pkg]);
    }
    const rest = [...byLicense.entries()]
      .map(([license, list]) => ({ license, packages: list }))
      .sort((a, b) => b.packages.length - a.packages.length || a.license.localeCompare(b.license));
    return lead.length ? [{ license: lead[0].license ?? "Unknown", packages: lead }, ...rest] : rest;
  }
}
