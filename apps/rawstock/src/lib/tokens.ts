/** Appearance theme. Warm is the default; neutral is the grey Darkroom palette. */
export type Theme = "warm" | "neutral";

export class ThemeSetting {
  static readonly fallback: Theme = "warm";

  /** Unknown or missing values fall back to warm. */
  static parse(value: unknown): Theme {
    return value === "neutral" ? "neutral" : ThemeSetting.fallback;
  }
}
