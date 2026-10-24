<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { onMount } from "svelte";

  import { Adjustments } from "./lib/adjust";
  import { appError, defaultSettings } from "./lib/contract";
  import type {
    AsShotWhiteBalance,
    CancelledEvent,
    CropRect,
    DoneEvent,
    EditSettings,
    FailedEvent,
    ImageInfo,
    StockSummary,
    ThumbnailReadyEvent,
  } from "./lib/contract";
  import ExportDialog from "./lib/ExportDialog.svelte";
  import { ExportPlan } from "./lib/export";
  import type { ExportRequest } from "./lib/export";
  import { JobTracker } from "./lib/delivery";
  import { PreviewInbox, PreviewSchedule } from "./lib/preview";
  import {
    Autosave,
    EditHistory,
    baselineSettings,
    invalidSidecarNotice,
    sidecarFromSave,
    sortImages,
    versionWarning,
  } from "./lib/edits";
  import Filmstrip from "./lib/Filmstrip.svelte";
  import { FrameBuffer } from "./lib/frame";
  import type { PackedFrame } from "./lib/frame";
  import About from "./lib/help/About.svelte";
  import Licenses from "./lib/help/Licenses.svelte";
  import { Report } from "./lib/help/report";
  import type { AppInfo } from "./lib/help/report";
  import { Host } from "./lib/host";
  import { ReviewController } from "./lib/review/review";
  import type { Rect, ReviewView, Zoom } from "./lib/review/review";
  import Onboarding from "./lib/onboarding/Onboarding.svelte";
  import Preferences from "./lib/preferences/Preferences.svelte";
  import { Preferences as Prefs } from "./lib/preferences/settings";
  import type { AppSettings } from "./lib/preferences/settings";
  import { Shortcuts } from "./lib/shortcuts";
  import SidePanel from "./lib/SidePanel.svelte";
  import { StockBrowser, ThumbnailRequests } from "./lib/stockBrowser";
  import Viewer from "./lib/Viewer.svelte";

  type PreviewTerminal =
    | { type: "done"; jobId: number; event: DoneEvent }
    | { type: "failed"; jobId: number; event: FailedEvent }
    | { type: "cancelled"; jobId: number; event: CancelledEvent };

  const host = Host.available() ? new Host() : null;
  const jobs = new JobTracker();
  const inbox = new PreviewInbox<PreviewTerminal>();
  const history = new EditHistory();
  const thumbnailRequests = new ThumbnailRequests();
  const platform = Shortcuts.platform();
  const modLabel = platform === "mac" ? "Meta" : "Control";
  const autosave = new Autosave(
    500,
    async (imageId, next) => {
      if (!host) {
        return;
      }
      try {
        await host.saveSidecar(imageId, next);
      } catch (error) {
        if (appError(error).code === "sidecarInvalid") {
          autosave.block(imageId, true);
        }
        throw error;
      }
      images = images.map((image) =>
        image.imageId === imageId ? { ...image, sidecar: sidecarFromSave(next, stocks) } : image,
      );
    },
    undefined,
    (error) => {
      errors = [appError(error).message];
    },
  );

  let images = $state<ImageInfo[]>([]);
  let selected = $state<string>("");
  let stocks = $state<StockSummary[]>([]);
  let settings = $state<EditSettings>(defaultSettings());
  let errors = $state<string[]>([]);
  let notice = $state("Ready");
  let progress = $state("");
  let zoom = $state<Zoom>("fit");
  let compare = $state(false);
  let split = $state(false);
  let clipping = $state(false);
  let dropActive = $state(false);
  let exportOpen = $state(false);
  let exportRun = $state<{
    jobId: number;
    imageId: string;
    path: string;
    stage: string;
    done: number;
    total: number;
    cancelling: boolean;
  } | null>(null);
  let exportAwaiting: string | null = null;
  let exportPath = "";
  let aboutOpen = $state(false);
  let licensesOpen = $state(false);
  let preferencesOpen = $state(false);
  let onboardingOpen = $state(false);
  let onboardingMode = $state<"welcome" | "cheat-sheet">("welcome");
  let samplePath = $state<string | null>(null);
  let appSettings = $state<AppSettings>(Prefs.defaults());
  let appInfo = $state<AppInfo | null>(null);
  let review = $state<ReviewView | null>(null);
  /** Stock thumbnails for `thumbnailImage`, by stock id. */
  let thumbnails = $state<Record<string, PackedFrame>>({});
  let thumbnailImage = "";
  /** Last committed edit. Thumbnails follow it, never a slider mid-drag. */
  let committed = $state<EditSettings | null>(null);
  let resetting = $state(false);
  let horizon = $state(false);
  /** The straighten slider is moving: the viewer shows its level grid. */
  let straightening = $state(false);
  /**
   * The develop part of the last preview asked for, and of the last settled
   * one. Geometry is not part of it: frames stay in camera orientation and
   * the viewer frames them, so a crop or straighten never develops again.
   */
  let previewKeys = { requested: "", settled: "" };
  /** As-shot light of the photo it was read for. `value` is `null` when the file cannot take a custom balance. */
  let asShot = $state<{ imageId: string; value: AsShotWhiteBalance | null } | null>(null);
  let viewLongEdge = $state(720);
  let closing = false;
  let previewBusy = false;
  let previewPending: {
    settings: EditSettings;
    phase: "drag" | "settle";
    imageId: string;
  } | null = null;

  $effect(() => {
    Prefs.applyAppearance(appSettings);
  });

  const selectedImage = $derived(images.find((image) => image.imageId === selected) ?? null);
  const openProblem = $derived(selectedImage?.error?.message ?? "");
  const versionNotice = $derived(
    selected && !selectedImage?.error ? versionWarning(settings, stocks) : null,
  );
  const invalidNotice = $derived(
    selectedImage?.sidecar.status === "invalid" ? invalidSidecarNotice(selectedImage.fileName) : null,
  );
  const missingStockNotice = $derived.by(() => {
    if (!selected || selectedImage?.error || stocks.length === 0 || !settings.stock.id) {
      return null;
    }
    if (stocks.some((stock) => stock.id === settings.stock.id)) {
      return null;
    }
    const status = selectedImage?.sidecar.status;
    if (status !== "loaded" && status !== "stockVersionMismatch") {
      return null;
    }
    return `The stock "${settings.stock.id}" is not installed. Choose another stock. The photo was not changed.`;
  });
  const adjustmentsDisabled = $derived(!selected || selectedImage?.error != null);
  const selectedAsShot = $derived(
    asShot && asShot.imageId === selected ? asShot.value : selectedImage?.error ? null : undefined,
  );
  const browserStocks = $derived(StockBrowser.visible(stocks, import.meta.env.DEV));

  $effect(() => {
    const imageId = selected;
    const edit = committed;
    const list = browserStocks;
    const broken = selectedImage?.error != null;
    if (!host || !imageId || !edit || broken || list.length === 0) {
      return;
    }
    const key = `${StockBrowser.thumbnailKey(edit)}|${list.map((stock) => stock.id).join(",")}`;
    if (thumbnailRequests.needs(imageId, key)) {
      void requestThumbnails(imageId, key, edit, list);
    }
  });

  // Fit shows the proxy; 100% and 200% pan one full develop per settings state.
  const reviewer = host
    ? new ReviewController(host, {
        onview: (next) => {
          review = next;
        },
        onerror: (error) => {
          errors = [error.message];
        },
        onjob: (imageId, kind, jobId) => jobs.note(imageId, kind, jobId),
      })
    : null;
  reviewer?.adjustOwnsAfterPreview();

  $effect(() => {
    reviewer?.setZoom(zoom);
  });

  $effect(() => {
    reviewer?.setCompare(compare || split);
  });

  function exportEvent(imageId: string, jobId: number): boolean {
    if (jobs.current(imageId, "export", jobId)) {
      return true;
    }
    if (exportAwaiting === imageId) {
      jobs.note(imageId, "export", jobId);
      return true;
    }
    return false;
  }

  onMount(() => {
    const cleanups: Array<() => void> = [];
    const onKey = (event: KeyboardEvent) => {
      if (document.querySelector("dialog[open]")) {
        return;
      }
      const action = Shortcuts.actionFor(event);
      if (!action) {
        return;
      }
      event.preventDefault();
      if (action === "filmstrip-prev" || action === "filmstrip-next") {
        stepFilmstrip(action === "filmstrip-next" ? 1 : -1);
        return;
      }
      if (action === "filmstrip-first" || action === "filmstrip-last") {
        selectEdge(action === "filmstrip-last" ? "last" : "first");
        return;
      }
      if (!host || Shortcuts.dispatchLocally(action, event)) {
        if (host && Shortcuts.dispatchLocally(action, event)) {
          void host.dispatchMenuAction(action);
          return;
        }
        void perform(action);
      }
    };
    const syncFocus = () => {
      const suspended = Shortcuts.isTextEntry(document.activeElement);
      void host?.setShortcutsSuspended(suspended);
    };
    const onFocusOut = () => queueMicrotask(syncFocus);
    window.addEventListener("keydown", onKey);
    window.addEventListener("focusin", syncFocus);
    window.addEventListener("focusout", onFocusOut);
    cleanups.push(() => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("focusin", syncFocus);
      window.removeEventListener("focusout", onFocusOut);
    });

    if (!host) {
      return () => cleanups.forEach((cleanup) => cleanup());
    }

    void (async () => {
      const unlisten = await Promise.all([
        host.onMenu((action) => void perform(action)),
        host.onSettingsChanged((next) => {
          appSettings = Prefs.normalize(next);
        }),
        host.onOpen((paths) => void ingest(paths)),
        host.onProgress((event) => {
          if (event.kind === "export") {
            if (!exportEvent(event.imageId, event.jobId)) {
              return;
            }
            exportRun = {
              jobId: event.jobId,
              imageId: event.imageId,
              path: exportRun?.jobId === event.jobId && exportRun.path ? exportRun.path : exportPath,
              stage: event.stage,
              done: event.done,
              total: event.total,
              cancelling: exportRun?.jobId === event.jobId ? exportRun.cancelling : false,
            };
            return;
          }
          if (jobs.current(event.imageId, event.kind, event.jobId)) {
            progress = `${event.stage} ${event.done + 1}/${event.total}`;
          }
        }),
        host.onDone((event) => {
          if (event.kind === "preview") {
            const verdict = inbox.receive({ type: "done", jobId: event.jobId, event });
            if (verdict === "early") {
              return;
            }
            reviewer?.done(event);
            void loadAsShot(event.imageId);
          } else if (event.kind === "full") {
            reviewer?.done(event);
          }
          if (event.kind === "export") {
            if (!exportEvent(event.imageId, event.jobId)) {
              return;
            }
            const path = event.outputPath || exportRun?.path || exportPath;
            exportRun = null;
            notice = ExportPlan.finishedNotice(path);
            return;
          }
          if (!jobs.current(event.imageId, event.kind, event.jobId)) {
            return;
          }
          progress = "";
        }),
        host.onCancelled((event) => {
          if (event.kind === "preview") {
            const verdict = inbox.receive({ type: "cancelled", jobId: event.jobId, event });
            if (verdict === "early") {
              return;
            }
            reviewer?.cancelled(event);
          } else if (event.kind === "full") {
            reviewer?.cancelled(event);
          }
          if (event.kind === "export") {
            if (!exportEvent(event.imageId, event.jobId)) {
              return;
            }
            exportRun = null;
            notice = ExportPlan.cancelledNotice();
            return;
          }
          if (jobs.current(event.imageId, event.kind, event.jobId)) {
            progress = "";
          }
        }),
        host.onFailed((event) => {
          if (event.kind === "preview") {
            const verdict = inbox.receive({ type: "failed", jobId: event.jobId, event });
            if (verdict === "early") {
              return;
            }
            if (jobs.current(event.imageId, event.kind, event.jobId)) {
              progress = "";
            }
            reviewer?.failed(event);
            return;
          }
          if (event.kind === "full") {
            // The review view reports its own failures.
            if (jobs.current(event.imageId, event.kind, event.jobId)) {
              progress = "";
            }
            reviewer?.failed(event);
            return;
          }
          if (event.kind === "export") {
            if (!exportEvent(event.imageId, event.jobId)) {
              return;
            }
            exportRun = null;
            errors = [event.error.message];
            return;
          }
          if (!jobs.current(event.imageId, event.kind, event.jobId)) {
            return;
          }
          progress = "";
          errors = [event.error.message];
        }),
        host.onCacheReady((event) => reviewer?.cacheReady(event)),
        host.onCacheEvicted((event) => reviewer?.cacheEvicted(event)),
        host.onImageRefined((event) => {
          // The first preview came from the quick develop, without lens
          // corrections. Develop it again from the full frame.
          if (event.imageId === selected) {
            requestPreview(settings, "settle");
            reviewer?.refined(event);
          }
        }),
        host.onThumbnail((event) => void adoptThumbnail(event)),
      ]);
      cleanups.push(...unlisten);
      try {
        appSettings = Prefs.normalize(await host.getAppSettings());
        if (!appSettings.onboarding.completed) {
          showOnboarding("welcome");
        }
      } catch (error) {
        errors = [appError(error).message];
      }
      try {
        samplePath = await host.onboardingSample();
      } catch {
        samplePath = null;
      }
      try {
        appInfo = await host.appInfo();
      } catch (error) {
        errors = [appError(error).message];
      }
      try {
        stocks = await host.listStocks();
        const starting = Prefs.startingStock(appSettings, stocks);
        if (starting && !settings.stock.id) {
          settings = { ...settings, stock: { id: starting.id, revision: starting.revision } };
        }
      } catch (error) {
        errors = [appError(error).message];
      }
      const startup = await host.takeStartupOpens();
      if (startup.length) {
        await ingest(startup);
      }
      try {
        const closed = await getCurrentWindow().onCloseRequested(async (event) => {
          if (closing) {
            return;
          }
          event.preventDefault();
          closing = true;
          try {
            await autosave.flush();
          } catch (error) {
            errors = [appError(error).message];
          }
          try {
            await getCurrentWindow().destroy();
          } catch (error) {
            closing = false;
            errors = [appError(error).message];
          }
        });
        cleanups.push(closed);
      } catch (error) {
        errors = [appError(error).message];
      }
      try {
        const webview = getCurrentWebview();
        const drop = await webview.onDragDropEvent((event) => {
          if (event.payload.type === "enter" || event.payload.type === "over") {
            dropActive = true;
          }
          if (event.payload.type === "leave") {
            dropActive = false;
          }
          if (event.payload.type === "drop") {
            dropActive = false;
            void host.requestOpen(event.payload.paths);
          }
        });
        cleanups.push(drop);
      } catch (error) {
        errors = [appError(error).message];
      }
    })();

    return () => cleanups.forEach((cleanup) => cleanup());
  });

  async function ingest(paths: string[]) {
    if (!host || paths.length === 0) {
      return;
    }
    try {
      const outcome = await host.openPaths(paths);
      errors = outcome.rejected.map((item) => item.error.message);
      if (outcome.images.length === 0) {
        return;
      }
      const known = new Map(images.map((image) => [image.imageId, image]));
      for (const image of outcome.images) {
        known.set(image.imageId, image);
      }
      images = sortImages([...known.values()]);
      const opened = new Set(outcome.images.map((image) => image.imageId));
      const pick =
        images.find((image) => opened.has(image.imageId) && !image.error) ??
        images.find((image) => opened.has(image.imageId));
      if (pick) {
        await activate(pick.imageId, true);
      }
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  async function activate(imageId: string, reload = false) {
    if (!host) {
      return;
    }
    if (!reload && imageId === selected && history.has(imageId)) {
      return;
    }
    if (selected && selected !== imageId) {
      try {
        await autosave.flush();
      } catch (error) {
        errors = [appError(error).message];
      }
    }
    try {
      const info = await host.selectImage(imageId);
      const merged = images.some((image) => image.imageId === info.imageId)
        ? images.map((image) => (image.imageId === info.imageId ? info : image))
        : [...images, info];
      images = sortImages(merged);
      const known = history.has(info.imageId);
      const current = history.open(
        info.imageId,
        baselineSettings(info, Prefs.startingStock(appSettings, stocks)),
      );
      if (!known) {
        autosave.remember(info.imageId, current);
      }
      autosave.block(info.imageId, info.sidecar.status === "invalid");
      selected = info.imageId;
      settings = current;
      committed = current;
      if (!info.error) {
        reviewer?.open(info.imageId, current, { width: info.width, height: info.height });
      } else {
        reviewer?.close();
      }
      notice = info.fileName;
      horizon = false;
      straightening = false;
      previewKeys = { requested: "", settled: "" };
      if (!info.error) {
        requestPreview(current, "settle");
      }
      if (Host.available()) {
        await getCurrentWindow().setTitle(`${info.fileName} - rawstock`);
      }
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  function stepFilmstrip(delta: number) {
    if (images.length === 0) {
      return;
    }
    const index = Math.max(0, images.findIndex((image) => image.imageId === selected));
    const next = images[(index + delta + images.length) % images.length];
    if (next) {
      void activate(next.imageId);
    }
  }

  function selectEdge(edge: "first" | "last") {
    if (images.length === 0) {
      return;
    }
    const next = edge === "first" ? images[0] : images[images.length - 1];
    if (next) {
      void activate(next.imageId);
    }
  }

  function applyPreview(next: EditSettings) {
    if (!selected || selectedImage?.error) {
      return;
    }
    straightening = next.geometry.straightenDeg !== (committed ?? settings).geometry.straightenDeg;
    settings = next;
    history.preview(selected, next);
    requestPreview(next, "drag");
  }

  function applyCommit(next: EditSettings) {
    if (!selected || selectedImage?.error) {
      return;
    }
    straightening = false;
    settings = next;
    committed = next;
    if (history.commit(selected, next)) {
      autosave.schedule(selected, next);
    }
    reviewer?.update(next);
    requestPreview(next, "settle");
  }

  function requestPreview(next: EditSettings, phase: "drag" | "settle") {
    if (!host || !selected || selectedImage?.error) {
      return;
    }
    const key = ReviewController.developKey(next);
    if (key === previewKeys.requested && (phase === "drag" || key === previewKeys.settled)) {
      return;
    }
    previewKeys.requested = key;
    if (phase === "settle") {
      previewKeys.settled = key;
    }
    previewPending = { settings: next, phase, imageId: selected };
    if (!previewBusy) {
      void flushPreview();
    }
  }

  function deliverPreview(notice: PreviewTerminal) {
    if (notice.type === "done") {
      reviewer?.done(notice.event);
    } else if (notice.type === "failed") {
      reviewer?.failed(notice.event);
    } else {
      reviewer?.cancelled(notice.event);
    }
  }

  async function flushPreview() {
    if (!host) {
      return;
    }
    previewBusy = true;
    try {
      while (previewPending && host) {
        const job = previewPending;
        previewPending = null;
        const edge = PreviewSchedule.longEdge(job.phase, viewLongEdge);
        inbox.expect();
        reviewer?.holdPreview(job.settings, edge);
        try {
          const started = await host.developPreview(job.imageId, job.settings, edge, false);
          jobs.note(job.imageId, "preview", started.jobId);
          reviewer?.claimPreview(started.jobId);
          const arrived = inbox.note(started.jobId);
          if (arrived.matched) {
            deliverPreview(arrived.matched);
          }
          for (const old of arrived.stale) {
            deliverPreview(old);
          }
        } catch (error) {
          reviewer?.releasePreview();
          for (const old of inbox.abandon()) {
            deliverPreview(old);
          }
          errors = [appError(error).message];
        }
      }
    } finally {
      previewBusy = false;
      if (previewPending) {
        void flushPreview();
      }
    }
  }

  function editCrop(crop: CropRect, commit: boolean) {
    const next = Adjustments.crop(settings, crop);
    if (commit) {
      applyCommit(next);
    } else {
      applyPreview(next);
    }
  }

  function editStraighten(degrees: number, commit: boolean) {
    const next = Adjustments.straighten(settings, degrees);
    if (commit) {
      applyCommit(next);
    } else {
      applyPreview(next);
    }
  }

  function undoEdit() {
    if (!selected) {
      notice = "Nothing to undo.";
      return;
    }
    const next = history.undo(selected);
    if (!next) {
      notice = "Nothing to undo.";
      return;
    }
    settings = next;
    committed = next;
    autosave.schedule(selected, next);
    reviewer?.update(next);
    requestPreview(next, "settle");
    notice = selectedImage?.fileName ?? "Undo";
  }

  function redoEdit() {
    if (!selected) {
      notice = "Nothing to redo.";
      return;
    }
    const next = history.redo(selected);
    if (!next) {
      notice = "Nothing to redo.";
      return;
    }
    settings = next;
    committed = next;
    autosave.schedule(selected, next);
    reviewer?.update(next);
    requestPreview(next, "settle");
    notice = selectedImage?.fileName ?? "Redo";
  }

  async function resetEdits() {
    if (!host || !selected || resetting) {
      return;
    }
    resetting = true;
    try {
      const info = await host.resetSidecar(selected);
      images = images.map((image) => (image.imageId === info.imageId ? info : image));
      autosave.block(info.imageId, false);
      const live = history.current(info.imageId);
      if (live) {
        history.commit(info.imageId, live);
        if (history.dirty(info.imageId)) {
          autosave.schedule(info.imageId, live);
          await autosave.flush();
        }
      }
      notice = info.fileName;
    } catch (error) {
      errors = [appError(error).message];
    } finally {
      resetting = false;
    }
  }

  async function perform(action: string) {
    switch (action) {
      case "open":
        await pick(false);
        break;
      case "open-folder":
        await pick(true);
        break;
      case "export":
        if (!selected || selectedImage?.error) {
          notice = selected ? "That photo cannot be exported." : "Open a photo before exporting.";
          break;
        }
        if (exportRun) {
          notice = "An export is already running.";
          break;
        }
        exportOpen = true;
        break;
      case "close":
        try {
          await autosave.flush();
        } catch (error) {
          errors = [appError(error).message];
        }
        selected = "";
        reviewer?.close();
        committed = null;
        thumbnails = {};
        thumbnailImage = "";
        thumbnailRequests.clear();
        notice = "Ready";
        if (Host.available()) {
          await getCurrentWindow().setTitle("rawstock");
        }
        break;
      case "undo":
        undoEdit();
        break;
      case "redo":
        redoEdit();
        break;
      case "fit":
        zoom = "fit";
        break;
      case "zoom-100":
        zoom = 1;
        break;
      case "zoom-200":
        zoom = 2;
        break;
      case "split":
        split = !split;
        break;
      case "before-after":
        compare = !compare;
        break;
      case "clipping":
        clipping = !clipping;
        break;
      case "preferences":
        preferencesOpen = true;
        break;
      case "welcome":
        showOnboarding("welcome");
        break;
      case "cheat-sheet":
        showOnboarding("cheat-sheet");
        break;
      case "about":
        aboutOpen = true;
        break;
      case "licenses":
        licensesOpen = true;
        break;
      case "report-issue":
        if (host && appInfo) {
          await openUrl(Report.issueUrl(appInfo));
        }
        break;
      case "show-log":
        if (host && appInfo) {
          try {
            await revealItemInDir(appInfo.logPath);
          } catch {
            errors = [`The log file could not be shown. It is at ${appInfo.logPath}`];
          }
        }
        break;
      default:
        break;
    }
  }

  async function pick(directory: boolean) {
    if (!host) {
      errors = ["Open a photo from the rawstock desktop window."];
      return;
    }
    const picked = await open({
      title: directory ? "Open folder" : "Open RAW",
      directory,
      multiple: !directory,
      filters: directory ? undefined : [{ name: "RAW", extensions: ["raf", "dng", "RAF", "DNG"] }],
    });
    const paths = typeof picked === "string" ? [picked] : Array.isArray(picked) ? picked : [];
    if (paths.length) {
      await host.requestOpen(paths);
    }
  }

  function chooseStock(stock: StockSummary) {
    applyCommit({
      ...settings,
      stock: { id: stock.id, revision: stock.revision },
    });
    notice = `${stock.name}, revision ${stock.revision}`;
  }

  /** Folds the strip at once, then saves it so the next launch matches. */
  function collapseFilmstrip(collapsed: boolean) {
    const next = Prefs.withFilmstrip(appSettings, collapsed);
    appSettings = next;
    void changeAppSettings(next);
  }

  async function changeAppSettings(next: AppSettings) {
    if (!host) {
      appSettings = next;
      return;
    }
    try {
      appSettings = Prefs.normalize(await host.setAppSettings(next));
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  function showOnboarding(mode: "welcome" | "cheat-sheet") {
    if (onboardingOpen) {
      return;
    }
    onboardingMode = mode;
    onboardingOpen = true;
  }

  function closeOnboarding(dontShowAgain: boolean) {
    onboardingOpen = false;
    const completed = onboardingMode === "welcome" ? dontShowAgain : appSettings.onboarding.completed;
    if (completed !== appSettings.onboarding.completed) {
      void changeAppSettings({
        ...appSettings,
        onboarding: { ...appSettings.onboarding, completed },
      });
    }
  }

  async function resetAppSettings(clearRecent: boolean) {
    if (!host) {
      appSettings = Prefs.defaults();
      return;
    }
    try {
      appSettings = Prefs.normalize(await host.resetAppSettings(clearRecent));
      notice = clearRecent ? "Settings and recent files were reset." : "Settings were reset.";
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  /**
   * Read the photo's as-shot light once its first preview is up, so the
   * develop thread has already decoded it and the read is a lookup.
   */
  async function loadAsShot(imageId: string) {
    if (!host || asShot?.imageId === imageId) {
      return;
    }
    try {
      const value = await host.asShotWhiteBalance(imageId);
      asShot = { imageId, value };
    } catch {
      // Left unread: the next preview asks again.
    }
  }

  async function requestThumbnails(
    imageId: string,
    key: string,
    edit: EditSettings,
    list: StockSummary[],
  ) {
    if (!host) {
      return;
    }
    thumbnailRequests.begin(imageId, key);
    if (thumbnailImage !== imageId) {
      // Never show another photo's thumbnails, even for a moment.
      thumbnails = {};
      thumbnailImage = imageId;
    }
    const longEdge = Math.min(768, Math.round(160 * (window.devicePixelRatio || 1)));
    try {
      const started = await host.renderThumbnails(
        imageId,
        edit,
        list.map((stock) => stock.id),
        longEdge,
      );
      if (thumbnailRequests.started(imageId, key, started.jobId)) {
        jobs.note(imageId, "thumbnails", started.jobId);
      }
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  async function adoptThumbnail(event: ThumbnailReadyEvent) {
    if (!host || !thumbnailRequests.accepts(event)) {
      return;
    }
    try {
      const bytes = await host.takeFrame(event.jobId, event.stockId);
      if (!thumbnailRequests.accepts(event)) {
        return;
      }
      thumbnails = { ...thumbnails, [event.stockId]: FrameBuffer.decode(bytes) };
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  async function startExport(request: ExportRequest) {
    if (!host) {
      throw {
        code: "internal",
        message: "Open a photo from the rawstock desktop window.",
      };
    }
    exportPath = request.options.path;
    exportAwaiting = request.imageId;
    try {
      const started = await host.exportImage(request.imageId, request.settings, request.options);
      jobs.note(request.imageId, "export", started.jobId);
      if (!exportRun || exportRun.jobId !== started.jobId) {
        exportRun = {
          jobId: started.jobId,
          imageId: request.imageId,
          path: request.options.path,
          stage: "queued",
          done: 0,
          total: 0,
          cancelling: false,
        };
      }
    } finally {
      exportAwaiting = null;
    }
    exportOpen = false;
    await changeAppSettings(
      ExportPlan.remembered(appSettings, request.options.quality, request.options.path),
    );
  }

  async function cancelExport() {
    if (!host || !exportRun || exportRun.cancelling) {
      return;
    }
    const jobId = exportRun.jobId;
    exportRun = { ...exportRun, cancelling: true };
    try {
      await host.cancel(jobId);
    } catch (error) {
      errors = [appError(error).message];
      if (exportRun?.jobId === jobId) {
        exportRun = { ...exportRun, cancelling: false };
      }
    }
  }

  function viewChanged(rect: Rect) {
    reviewer?.view(rect);
  }
</script>

<div class="app">
  <h1>rawstock</h1>
  <header class="toolbar" role="toolbar" aria-label="Develop">
    <button type="button" onclick={() => void perform("open")} aria-keyshortcuts="{modLabel}+O">Open…</button>
    <button type="button" class="primary" onclick={() => void perform("export")} aria-keyshortcuts="{modLabel}+E">Export…</button>
    <span class="spacer"></span>
    <button type="button" aria-pressed={zoom === "fit"} aria-keyshortcuts="{modLabel}+0" onclick={() => (zoom = "fit")}>Fit</button>
    <button type="button" aria-pressed={zoom === 1} aria-keyshortcuts="{modLabel}+1" onclick={() => (zoom = 1)}>100%</button>
    <button type="button" aria-pressed={zoom === 2} aria-keyshortcuts="{modLabel}+2" onclick={() => (zoom = 2)}>200%</button>
    <button type="button" aria-pressed={split} aria-keyshortcuts="Y" onclick={() => (split = !split)}>Split</button>
    <button type="button" aria-pressed={compare} aria-keyshortcuts={"\\"} onclick={() => (compare = !compare)}>Before/After</button>
    <button type="button" aria-pressed={clipping} aria-keyshortcuts="J" onclick={() => (clipping = !clipping)}>Clipping</button>
  </header>

  {#if errors.length}
    <div class="banner" role="alert">
      <div>
        {#each errors as message, index (index)}
          <p>{message}</p>
        {/each}
      </div>
      <button type="button" aria-label="Dismiss message" onclick={() => (errors = [])}>Dismiss</button>
    </div>
  {/if}

  {#if versionNotice || missingStockNotice}
    <div class="banner" role="status">
      {#if versionNotice}<p>{versionNotice}</p>{/if}
      {#if missingStockNotice}<p>{missingStockNotice}</p>{/if}
    </div>
  {/if}

  {#if invalidNotice}
    <div class="banner" role="alert">
      <p>{invalidNotice}</p>
      <button type="button" onclick={() => void resetEdits()} disabled={resetting}>Reset edits</button>
    </div>
  {/if}

  <div class="workspace">
    <div class="stage">
      <Viewer
        view={review}
        active={selected !== "" && !openProblem}
        {zoom}
        {split}
        showBefore={compare}
        {clipping}
        fileName={selectedImage?.fileName ?? ""}
        problem={openProblem}
        {dropActive}
        onopen={() => void perform("open")}
        edit={settings.geometry}
        {horizon}
        guides={straightening}
        onzoom={(next) => (zoom = next)}
        onregion={viewChanged}
        oncrop={editCrop}
        onstraighten={editStraighten}
        onsize={(edge) => {
          viewLongEdge = Math.max(1, Math.round(edge));
          reviewer?.setLongEdge(viewLongEdge);
        }}
      />
      <Filmstrip
        {images}
        {selected}
        collapsed={appSettings.layout?.filmstripCollapsed === true}
        onselect={(id) => void activate(id)}
        oncollapse={collapseFilmstrip}
      />
    </div>
    <SidePanel
      stocks={browserStocks}
      {thumbnails}
      {settings}
      stockId={settings.stock.id}
      disabled={adjustmentsDisabled}
      frameWidth={review?.width || selectedImage?.width || 0}
      frameHeight={review?.height || selectedImage?.height || 0}
      {horizon}
      asShot={selectedAsShot}
      onstock={chooseStock}
      onpreview={applyPreview}
      oncommit={applyCommit}
      onhorizon={(on) => (horizon = on)}
    />
  </div>

  {#if exportRun}
    <div class="export-run" role="status">
      <span class="count">{ExportPlan.progressText(exportRun.stage, exportRun.done, exportRun.total)}</span>
      <span class="who">{ExportPlan.baseName(exportRun.path)}</span>
      <span class="spacer"></span>
      <button
        type="button"
        onclick={() => void cancelExport()}
        disabled={exportRun.cancelling}
        aria-label={exportRun.cancelling ? "Cancelling export" : "Cancel export"}
      >
        {exportRun.cancelling ? "Cancelling…" : "Cancel"}
      </button>
    </div>
  {/if}

  <footer class="status" aria-live="polite">
    <span>{progress || notice}</span>
    {#if compare}<span>Before</span>{:else if split}<span>Split</span>{/if}
    {#if clipping}<span>Clipping</span>{/if}
  </footer>
</div>

<ExportDialog
  open={exportOpen}
  image={selectedImage}
  {settings}
  quality={appSettings.export?.quality ?? 92}
  lastFolder={appSettings.export?.lastFolder ?? null}
  onclose={() => (exportOpen = false)}
  onexport={startExport}
/>

<Preferences
  open={preferencesOpen}
  settings={appSettings}
  {stocks}
  title={platform === "mac" ? "Settings" : "Preferences"}
  onchange={(next) => void changeAppSettings(next)}
  onreset={(clearRecent) => void resetAppSettings(clearRecent)}
  onclose={() => (preferencesOpen = false)}
/>

<Onboarding
  open={onboardingOpen}
  mode={onboardingMode}
  {platform}
  sampleAvailable={samplePath !== null}
  onopen={() => void perform("open")}
  onopenfolder={() => void perform("open-folder")}
  onsample={() => {
    if (host && samplePath) {
      void host.requestOpen([samplePath]);
    }
  }}
  onclose={closeOnboarding}
/>

<About
  open={aboutOpen}
  info={appInfo}
  onclose={() => (aboutOpen = false)}
  onlicenses={() => {
    aboutOpen = false;
    licensesOpen = true;
  }}
/>

<Licenses open={licensesOpen} onclose={() => (licensesOpen = false)} />

<style>
  .export-run {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.35rem 0.7rem;
    background: var(--paper);
    border-top: 1px solid var(--line);
    color: var(--ink);
  }

  .export-run .count,
  .export-run .who {
    font-family: var(--font-mono);
    font-size: 0.78rem;
    font-variant-numeric: tabular-nums;
  }

  .export-run .who {
    color: var(--muted);
  }

  .export-run button {
    border-radius: 4px;
    font-size: 0.78rem;
    padding: 0.2rem 0.55rem;
  }
</style>
