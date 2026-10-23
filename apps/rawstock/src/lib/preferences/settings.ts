import type { StockSummary } from "../contract";

export type Surround = "dark" | "mid";
export type ReduceMotion = "system" | "on" | "off";

/**
 * Mirrors `settings.rs`. Unknown keys from a newer build ride along in the
 * index signatures so a save never drops them.
 */
export interface AppSettings {
  version: number;
  appearance: { surround: Surround; reduceMotion: ReduceMotion; [key: string]: unknown };
  newImage: { defaultStockId: string | null; [key: string]: unknown };
  recent: { limit: number; [key: string]: unknown };
  onboarding: { completed: boolean; [key: string]: unknown };
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
      appearance: { surround: "dark", reduceMotion: "system" },
      newImage: { defaultStockId: null },
      recent: { limit: 10 },
      onboarding: { completed: false },
    };
  }

  static surroundColors(surround: Surround): { background: string; ink: string } {
    return SURROUND_COLORS[surround] ?? SURROUND_COLORS.dark;
  }

  /** Push the appearance keys onto the document root, where the CSS reads them. */
  static applyAppearance(settings: AppSettings, root: HTMLElement = document.documentElement) {
    const colors = Preferences.surroundColors(settings.appearance.surround);
    root.style.setProperty("--surround", colors.background);
    root.style.setProperty("--surround-ink", colors.ink);
    root.dataset.reduceMotion = settings.appearance.reduceMotion;
  }

  /** The stock a newly opened image starts on. */
  static startingStock(settings: AppSettings, stocks: StockSummary[]): StockSummary | null {
    const wanted = settings.newImage.defaultStockId;
    return stocks.find((stock) => stock.id === wanted) ?? stocks[0] ?? null;
  }
}
