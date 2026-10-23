// Pure helpers for Help > About and Help > Report an issue. No imports with
// runtime effects, so `node --test` can load this file directly.

/** Mirrors `help.rs` `AppInfo`. */
export interface AppInfo {
  version: string;
  commit: string;
  buildDate: string;
  os: string;
  arch: string;
  logPath: string;
}

export const ISSUES_URL = "https://github.com/emperorpickles/rawstock/issues/new";

export class Report {
  /** Plain text for "Copy details". Version, commit, build date, OS and arch only. */
  static details(info: AppInfo): string {
    return [
      `rawstock ${info.version}`,
      `Commit: ${info.commit}`,
      `Built: ${info.buildDate}`,
      `OS: ${Report.osName(info.os)} (${info.arch})`,
    ].join("\n");
  }

  /**
   * The new-issue body. It never carries a path, a file name, EXIF, or an
   * image: only the fields `details` copies.
   */
  static body(info: AppInfo): string {
    return [
      "### What happened",
      "",
      "",
      "### What you expected",
      "",
      "",
      "### Steps to reproduce",
      "",
      "",
      "### Build",
      "",
      "```",
      Report.details(info),
      "```",
      "",
      "Please attach `rawstock.log` (Help > Show Log File). Do not attach personal photos.",
      "If a RAW file is needed to reproduce the problem, say so and a maintainer will ask",
      "for one you are happy to share.",
    ].join("\n");
  }

  static issueUrl(info: AppInfo): string {
    const url = new URL(ISSUES_URL);
    url.searchParams.set("title", `Problem in rawstock ${info.version}`);
    url.searchParams.set("body", Report.body(info));
    return url.toString();
  }

  static osName(os: string): string {
    switch (os) {
      case "macos":
        return "macOS";
      case "windows":
        return "Windows";
      case "linux":
        return "Linux";
      default:
        return os;
    }
  }
}
