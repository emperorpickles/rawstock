import type { StockSummary } from "../contract.ts";
import { ExportPlan } from "../export.ts";
import { ThemeSetting } from "../tokens.ts";
import type { Theme } from "../tokens.ts";

export type Surround = "dark" | "mid";
export type ReduceMotion = "system" | "on" | "off";
export type { Theme };

/**
 * Mirrors `settings.rs`. Unknown keys from a newer build ride along in the
 * index signatures so a save never drops them.
 */
export interface ExportPrefs {
  quality: number;
  lastFolder: string | null;
  [key: string]: unknown;
}

export interface AppSettings {
  version: number;
  appearance: {
    surround: Surround;
    reduceMotion: ReduceMotion;
    theme: Theme;
    [key: string]: unknown;
  };
  newImage: { defaultStockId: string | null; [key: string]: unknown };
  recent: { limit: number; [key: string]: unknown };
  onboarding: { completed: boolean; [key: string]: unknown };
  export: ExportPrefs;
  [namespace: string]: unknown;
}

export interface SettingsChanged {
  settings: AppSettings;
}

export const RECENT_MIN = 5;
export const RECENT_MAX = 20;

/** Both greys are neutral (R = G = B) so the surround never tints the photo. */
const SURROUND_COLORS: Record<Surround, { background: string; ink: string }> = {
  dark: { background: "#2e2e2e", ink: "#f2f2f2" },
  mid: { background: "#7a7a7a", ink: "#141414" },
};

export class Preferences {
  static defaults(): AppSettings {
    return {
      version: 1,
      appearance: { surround: "dark", reduceMotion: "system", theme: "warm" },
      newImage: { defaultStockId: null },
      recent: { limit: 10 },
      onboarding: { completed: false },
      export: { quality: 92, lastFolder: null },
    };
  }

  static surroundColors(surround: Surround): { background: string; ink: string } {
    return SURROUND_COLORS[surround] ?? SURROUND_COLORS.dark;
  }

  /**
   * Fill a theme a file omitted, and turn anything else into warm.
   * Surround and the other keys are left as they were saved.
   */
  static normalize(settings: AppSettings): AppSettings {
    const saved = settings.export ?? { quality: 92, lastFolder: null };
    const folder = typeof saved.lastFolder === "string" ? saved.lastFolder.trim() : "";
    return {
      ...settings,
      appearance: {
        ...settings.appearance,
        theme: ThemeSetting.parse(settings.appearance?.theme),
      },
      export: {
        ...saved,
        quality: ExportPlan.savedQuality(saved.quality),
        lastFolder: folder || null,
      },
    };
  }

  /** Push the appearance keys onto the document root, where the CSS reads them. */
  static applyAppearance(settings: AppSettings, root: HTMLElement = document.documentElement) {
    const colors = Preferences.surroundColors(settings.appearance.surround);
    root.style.setProperty("--surround", colors.background);
    root.style.setProperty("--surround-ink", colors.ink);
    root.dataset.reduceMotion = settings.appearance.reduceMotion;
    root.dataset.theme = ThemeSetting.parse(settings.appearance.theme);
  }

  /** The stock a newly opened image starts on. */
  static startingStock(settings: AppSettings, stocks: StockSummary[]): StockSummary | null {
    const wanted = settings.newImage.defaultStockId;
    return stocks.find((stock) => stock.id === wanted) ?? stocks[0] ?? null;
  }
}
