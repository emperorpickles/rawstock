import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { appError } from "./contract";
import type { AppSettings, SettingsChanged } from "./preferences/settings";
import type { AppInfo } from "./help/report";
import type {
  AsShotWhiteBalance,
  CacheEvictedEvent,
  ImageRefinedEvent,
  CacheReadyEvent,
  CancelledEvent,
  DoneEvent,
  EditSettings,
  ExportOptions,
  FailedEvent,
  ImageInfo,
  JobStarted,
  MenuAction,
  OpenOutcome,
  OpenRequested,
  ProgressEvent,
  SaveResult,
  StockSummary,
  ThumbnailReadyEvent,
} from "./contract";

export class Host {
  static available(): boolean {
    return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  }

  async openPaths(paths: string[]): Promise<OpenOutcome> {
    return this.invoke("open_paths", { paths });
  }

  async selectImage(imageId: string): Promise<ImageInfo> {
    return this.invoke("select_image", { imageId });
  }

  async listStocks(): Promise<StockSummary[]> {
    return this.invoke("list_stocks");
  }

  async developPreview(
    imageId: string,
    settings: EditSettings,
    longEdge: number,
    compare: boolean,
  ): Promise<JobStarted> {
    return this.invoke("develop_preview", { imageId, settings, longEdge, compare });
  }

  async asShotWhiteBalance(imageId: string): Promise<AsShotWhiteBalance | null> {
    return this.invoke("as_shot_white_balance", { imageId });
  }

  async developFull(imageId: string, settings: EditSettings): Promise<JobStarted> {
    return this.invoke("develop_full", { imageId, settings });
  }

  async getRegion(args: {
    imageId: string;
    settingsHash: string;
    x: number;
    y: number;
    width: number;
    height: number;
  }): Promise<Uint8Array> {
    const value = await this.invoke<ArrayBuffer | Uint8Array>("get_region", args);
    return value instanceof Uint8Array ? value : new Uint8Array(value);
  }

  async renderThumbnails(
    imageId: string,
    settings: EditSettings,
    stockIds: string[],
    longEdge: number,
  ): Promise<JobStarted> {
    return this.invoke("render_thumbnails", { imageId, settings, stockIds, longEdge });
  }

  async takeFrame(jobId: number, stockId?: string): Promise<Uint8Array> {
    const value = await this.invoke<ArrayBuffer | Uint8Array>("take_frame", { jobId, stockId });
    return value instanceof Uint8Array ? value : new Uint8Array(value);
  }

  async cancel(jobId: number): Promise<boolean> {
    const result = await this.invoke<{ cancelled: boolean }>("cancel", { jobId });
    return result.cancelled;
  }

  async exportImage(
    imageId: string,
    settings: EditSettings,
    options: ExportOptions,
  ): Promise<JobStarted> {
    return this.invoke("export_image", { imageId, settings, options });
  }

  async saveSidecar(imageId: string, settings: EditSettings): Promise<SaveResult> {
    return this.invoke("save_sidecar", { imageId, settings });
  }

  async resetSidecar(imageId: string): Promise<ImageInfo> {
    return this.invoke("reset_sidecar", { imageId });
  }

  async recentFiles(): Promise<string[]> {
    return this.invoke("recent_files");
  }

  async clearRecent(): Promise<void> {
    await this.invoke("clear_recent");
  }

  async getAppSettings(): Promise<AppSettings> {
    return this.invoke("get_app_settings");
  }

  async setAppSettings(settings: AppSettings): Promise<AppSettings> {
    return this.invoke("set_app_settings", { settings });
  }

  async resetAppSettings(clearRecent: boolean): Promise<AppSettings> {
    return this.invoke("reset_app_settings", { clearRecent });
  }

  async onboardingSample(): Promise<string | null> {
    return this.invoke("onboarding_sample");
  }

  async appInfo(): Promise<AppInfo> {
    return this.invoke("app_info");
  }

  async dispatchMenuAction(action: string): Promise<boolean> {
    return this.invoke("dispatch_menu_action", { action });
  }

  async setShortcutsSuspended(suspended: boolean): Promise<void> {
    await this.invoke("set_shortcuts_suspended", { suspended });
  }

  async requestOpen(paths: string[]): Promise<void> {
    await this.invoke("request_open", { paths });
  }

  async takeStartupOpens(): Promise<string[]> {
    return this.invoke("take_startup_opens");
  }

  on<T>(name: string, handler: (payload: T) => void): Promise<UnlistenFn> {
    return listen<T>(name, (event) => handler(event.payload));
  }

  onMenu(handler: (action: string) => void): Promise<UnlistenFn> {
    return this.on<MenuAction>("menu:action", (payload) => handler(payload.action));
  }

  onOpen(handler: (paths: string[]) => void): Promise<UnlistenFn> {
    return this.on<OpenRequested>("open:requested", (payload) => handler(payload.paths));
  }

  onSettingsChanged(handler: (settings: AppSettings) => void): Promise<UnlistenFn> {
    return this.on<SettingsChanged>("settings:changed", (payload) => handler(payload.settings));
  }

  onProgress(handler: (event: ProgressEvent) => void): Promise<UnlistenFn> {
    return this.on("job:progress", handler);
  }

  onDone(handler: (event: DoneEvent) => void): Promise<UnlistenFn> {
    return this.on("job:done", handler);
  }

  onCancelled(handler: (event: CancelledEvent) => void): Promise<UnlistenFn> {
    return this.on("job:cancelled", handler);
  }

  onFailed(handler: (event: FailedEvent) => void): Promise<UnlistenFn> {
    return this.on("job:failed", handler);
  }

  onThumbnail(handler: (event: ThumbnailReadyEvent) => void): Promise<UnlistenFn> {
    return this.on("thumbnail:ready", handler);
  }

  onCacheReady(handler: (event: CacheReadyEvent) => void): Promise<UnlistenFn> {
    return this.on("cache:ready", handler);
  }

  onCacheEvicted(handler: (event: CacheEvictedEvent) => void): Promise<UnlistenFn> {
    return this.on("cache:evicted", handler);
  }

  onImageRefined(handler: (event: ImageRefinedEvent) => void): Promise<UnlistenFn> {
    return this.on("image:refined", handler);
  }

  private async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    try {
      return await invoke<T>(command, args);
    } catch (error) {
      throw appError(error);
    }
  }
}
