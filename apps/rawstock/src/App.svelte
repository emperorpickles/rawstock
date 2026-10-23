<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { onMount } from "svelte";

  import { appError, defaultSettings } from "./lib/contract";
  import type { EditSettings, ImageInfo, StockSummary } from "./lib/contract";
  import { Delivery, JobTracker } from "./lib/delivery";
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
  import Onboarding from "./lib/onboarding/Onboarding.svelte";
  import Preferences from "./lib/preferences/Preferences.svelte";
  import { Preferences as Prefs } from "./lib/preferences/settings";
  import type { AppSettings } from "./lib/preferences/settings";
  import { Shortcuts } from "./lib/shortcuts";
  import SidePanel from "./lib/SidePanel.svelte";
  import Viewer from "./lib/Viewer.svelte";

  const host = Host.available() ? new Host() : null;
  const jobs = new JobTracker();
  const history = new EditHistory();
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
  let zoom = $state<"fit" | 1 | 2>("fit");
  let compare = $state(false);
  let clipping = $state(false);
  let dropActive = $state(false);
  let exportOpen = $state(false);
  let aboutOpen = $state(false);
  let licensesOpen = $state(false);
  let preferencesOpen = $state(false);
  let onboardingOpen = $state(false);
  let onboardingMode = $state<"welcome" | "cheat-sheet">("welcome");
  let samplePath = $state<string | null>(null);
  let appSettings = $state<AppSettings>(Prefs.defaults());
  let appInfo = $state<AppInfo | null>(null);
  let quality = $state(90);
  let frame = $state<PackedFrame | null>(null);
  let frameMode = $state<"texture" | "region" | "empty">("empty");
  let memoryPressure = $state(false);
  let fullJob = $state<number | null>(null);
  let cacheHash = $state("");
  let cacheWidth = $state(0);
  let cacheHeight = $state(0);
  let exportDialog = $state<HTMLDialogElement | null>(null);
  let resetting = $state(false);
  let closing = false;

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

  $effect(() => {
    syncDialog(exportDialog, exportOpen);
  });

  function syncDialog(dialog: HTMLDialogElement | null, openDialog: boolean) {
    if (!dialog) {
      return;
    }
    if (openDialog && !dialog.open) {
      dialog.showModal();
    }
    if (!openDialog && dialog.open) {
      dialog.close();
    }
  }

  onMount(() => {
    const cleanups: Array<() => void> = [];
    const onKey = (event: KeyboardEvent) => {
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
          appSettings = next;
        }),
        host.onOpen((paths) => void ingest(paths)),
        host.onProgress((event) => {
          if (jobs.current(event.imageId, event.kind, event.jobId)) {
            progress = `${event.stage} ${event.done + 1}/${event.total}`;
          }
        }),
        host.onDone((event) => {
          if (!jobs.current(event.imageId, event.kind, event.jobId)) {
            return;
          }
          progress = "";
          if (event.kind === "full") {
            fullJob = event.jobId;
          }
          if (event.kind === "export") {
            notice = "Export is not written in this build.";
          }
        }),
        host.onCancelled((event) => {
          if (jobs.current(event.imageId, event.kind, event.jobId)) {
            progress = "";
          }
        }),
        host.onFailed((event) => {
          if (!jobs.current(event.imageId, event.kind, event.jobId)) {
            return;
          }
          progress = "";
          errors = [event.error.message];
        }),
        host.onCacheReady((event) => void adoptCache(event.imageId, event.settingsHash, event.width, event.height)),
        host.onCacheEvicted((event) => {
          if (event.reason === "settingsChanged") {
            return;
          }
          if (event.imageId === selected) {
            frame = null;
            frameMode = selected ? "empty" : "empty";
          }
          if (event.reason === "memoryBudget") {
            memoryPressure = true;
            void developFull();
          }
        }),
        host.onThumbnail(() => {
          notice = "Stock thumbnails are not drawn in this build.";
        }),
      ]);
      cleanups.push(...unlisten);
      try {
        appSettings = await host.getAppSettings();
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
      frame = null;
      frameMode = "empty";
      fullJob = null;
      notice = info.fileName;
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
    settings = next;
    history.preview(selected, next);
  }

  function applyCommit(next: EditSettings) {
    if (!selected || selectedImage?.error) {
      return;
    }
    settings = next;
    if (history.commit(selected, next)) {
      autosave.schedule(selected, next);
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
    autosave.schedule(selected, next);
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
    autosave.schedule(selected, next);
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
        exportOpen = true;
        break;
      case "close":
        try {
          await autosave.flush();
        } catch (error) {
          errors = [appError(error).message];
        }
        selected = "";
        frame = null;
        frameMode = "empty";
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

  async function changeAppSettings(next: AppSettings) {
    if (!host) {
      appSettings = next;
      return;
    }
    try {
      appSettings = await host.setAppSettings(next);
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
      appSettings = await host.resetAppSettings(clearRecent);
      notice = clearRecent ? "Settings and recent files were reset." : "Settings were reset.";
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  async function developFull() {
    if (!host || !selected) {
      return;
    }
    try {
      const started = await host.developFull(selected, settings);
      jobs.note(selected, "full", started.jobId);
      fullJob = started.jobId;
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  async function adoptCache(imageId: string, settingsHash: string, width: number, height: number) {
    if (!host || imageId !== selected || fullJob === null) {
      return;
    }
    cacheHash = settingsHash;
    cacheWidth = width;
    cacheHeight = height;
    const choice = Delivery.choose(width, height, memoryPressure);
    try {
      if (choice === "texture") {
        const bytes = await host.takeFrame(fullJob);
        frame = FrameBuffer.decode(bytes);
        frameMode = "texture";
        return;
      }
      frameMode = "region";
      await loadRegion({ x: 0, y: 0, width: Math.min(width, 48), height: Math.min(height, 32) });
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  async function loadRegion(rect: { x: number; y: number; width: number; height: number }) {
    if (!host || !selected || !cacheHash || frameMode !== "region") {
      return;
    }
    const bytes = await host.getRegion({
      imageId: selected,
      settingsHash: cacheHash,
      x: rect.x,
      y: rect.y,
      width: rect.width,
      height: rect.height,
    });
    frame = FrameBuffer.decode(bytes);
  }

  function viewChanged(rect: { x: number; y: number; width: number; height: number }) {
    void loadRegion(rect).catch((error: unknown) => {
      errors = [appError(error).message];
    });
  }
</script>

<div class="app">
  <h1>rawstock</h1>
  <header class="toolbar" role="toolbar" aria-label="Develop">
    <button type="button" onclick={() => void perform("open")} aria-keyshortcuts="{modLabel}+O">Open…</button>
    <button type="button" onclick={() => void perform("export")} aria-keyshortcuts="{modLabel}+E">Export…</button>
    <span class="spacer"></span>
    <button type="button" aria-pressed={zoom === "fit"} aria-keyshortcuts="{modLabel}+0" onclick={() => (zoom = "fit")}>Fit</button>
    <button type="button" aria-pressed={zoom === 1} aria-keyshortcuts="{modLabel}+1" onclick={() => (zoom = 1)}>100%</button>
    <button type="button" aria-pressed={zoom === 2} aria-keyshortcuts="{modLabel}+2" onclick={() => (zoom = 2)}>200%</button>
    <button type="button" aria-pressed={compare} onclick={() => (compare = !compare)}>Before/After</button>
    <button type="button" aria-pressed={clipping} onclick={() => (clipping = !clipping)}>Clipping</button>
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
    <Filmstrip images={images} selected={selected} onselect={(id) => void activate(id)} />
    <Viewer
      {frame}
      mode={selected && frame && !openProblem ? frameMode : "empty"}
      {zoom}
      fileName={selectedImage?.fileName ?? ""}
      problem={openProblem}
      {dropActive}
      imageWidth={cacheWidth}
      imageHeight={cacheHeight}
      onopen={() => void perform("open")}
      onview={viewChanged}
    />
    <SidePanel
      {stocks}
      {settings}
      stockId={settings.stock.id}
      disabled={adjustmentsDisabled}
      onstock={chooseStock}
      onpreview={applyPreview}
      oncommit={applyCommit}
    />
  </div>

  <footer class="status" aria-live="polite">
    <span>{progress || notice}</span>
    {#if compare}<span>Before</span>{/if}
    {#if clipping}<span>Clipping</span>{/if}
  </footer>
</div>

<dialog bind:this={exportDialog} aria-labelledby="export-title" onclose={() => (exportOpen = false)}>
  <h2 id="export-title">Export</h2>
  <p>JPEG export is not hooked up in this build. The photo will not be written.</p>
  <label class="field">
    JPEG quality
    <input
      type="range"
      min="1"
      max="100"
      step="1"
      bind:value={quality}
      aria-valuetext={`${quality} percent`}
    />
  </label>
  <div class="dialog-actions">
    <button type="button" onclick={() => (exportOpen = false)}>Close</button>
  </div>
</dialog>

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
