import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";

import { FrameBuffer } from "./frame";
import type {
  Host,
  Progress,
  RenderRequest,
  SaveDraft,
  SessionState,
  StockState,
  StockSummary,
} from "./types";

interface DialogFilter {
  name: string;
  extensions: string[];
}

/** Desktop host. The web shell will implement the same [`Host`] later. */
export class TauriHost implements Host {
  static isAvailable(): boolean {
    return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  }

  async listStocks(): Promise<StockSummary[]> {
    return invoke<StockSummary[]>("list_stocks");
  }

  async session(): Promise<SessionState | null> {
    return invoke<SessionState | null>("session");
  }

  async openRaw(path: string): Promise<SessionState> {
    return invoke<SessionState>("open_raw", { path });
  }

  async loadStock(path: string): Promise<SessionState> {
    return invoke<SessionState>("load_stock", { path });
  }

  async rotate(direction: "clockwise" | "counterClockwise"): Promise<SessionState> {
    return invoke<SessionState>("rotate_image", { direction });
  }

  async render(request: RenderRequest) {
    const bytes = await invoke<ArrayBuffer | Uint8Array>("render_frame", { request });
    return FrameBuffer.decode(bytes);
  }

  async exportJpeg(path: string): Promise<void> {
    await invoke("export_jpeg", { path });
  }

  async saveStock(draft: SaveDraft): Promise<StockState> {
    return invoke<StockState>("save_stock", { request: draft });
  }

  async onProgress(handler: (progress: Progress) => void): Promise<() => void> {
    return listen<Progress>("develop-progress", (event) => handler(event.payload));
  }

  async onSession(handler: () => void): Promise<() => void> {
    return listen("session-changed", () => handler());
  }

  async onSessionError(handler: (message: string) => void): Promise<() => void> {
    return listen<string>("session-error", (event) => handler(event.payload));
  }

  static async pickRaw(): Promise<string | null> {
    return TauriHost.pickOne({
      title: "Open RAW",
      filters: [{ name: "RAW", extensions: ["raf", "RAF", "dng", "DNG"] }],
    });
  }

  static async pickStock(): Promise<string | null> {
    return TauriHost.pickOne({
      title: "Open stock",
      filters: [{ name: "Stock", extensions: ["toml"] }],
    });
  }

  static async pickJpeg(defaultPath: string): Promise<string | null> {
    const picked = await save({
      title: "Export JPEG",
      defaultPath,
      filters: [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
    });
    return typeof picked === "string" ? picked : null;
  }

  static async pickSave(defaultPath: string): Promise<string | null> {
    const picked = await save({
      title: "Save stock",
      defaultPath,
      filters: [{ name: "Stock", extensions: ["toml"] }],
    });
    return typeof picked === "string" ? picked : null;
  }

  private static async pickOne(options: {
    title: string;
    filters: DialogFilter[];
  }): Promise<string | null> {
    const picked = await open({
      title: options.title,
      multiple: false,
      directory: false,
      filters: options.filters,
    });
    return typeof picked === "string" ? picked : null;
  }
}
