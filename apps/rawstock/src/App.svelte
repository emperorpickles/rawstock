<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { onMount } from "svelte";

  import { appError, defaultSettings } from "./lib/contract";
  import type { EditSettings, ImageInfo, StockSummary } from "./lib/contract";
  import { Delivery, JobTracker } from "./lib/delivery";
  import Filmstrip from "./lib/Filmstrip.svelte";
  import { FrameBuffer } from "./lib/frame";
  import type { PackedFrame } from "./lib/frame";
  import { Host } from "./lib/host";
  import { Shortcuts } from "./lib/shortcuts";
  import SidePanel from "./lib/SidePanel.svelte";
  import Viewer from "./lib/Viewer.svelte";

  const host = Host.available() ? new Host() : null;
  const jobs = new JobTracker();
  const platform = Shortcuts.platform();
  const modLabel = platform === "mac" ? "Meta" : "Control";

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
  let quality = $state(90);
  let frame = $state<PackedFrame | null>(null);
  let frameMode = $state<"texture" | "region" | "empty">("empty");
  let memoryPressure = $state(false);
  let fullJob = $state<number | null>(null);
  let cacheHash = $state("");
  let cacheWidth = $state(0);
  let cacheHeight = $state(0);
  let exportDialog = $state<HTMLDialogElement | null>(null);
  let aboutDialog = $state<HTMLDialogElement | null>(null);
  let licensesDialog = $state<HTMLDialogElement | null>(null);

  const selectedImage = $derived(images.find((image) => image.imageId === selected) ?? null);

  $effect(() => {
    syncDialog(exportDialog, exportOpen);
    syncDialog(aboutDialog, aboutOpen);
    syncDialog(licensesDialog, licensesOpen);
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
        stocks = await host.listStocks();
      } catch (error) {
        errors = [appError(error).message];
      }
      const startup = await host.takeStartupOpens();
      if (startup.length) {
        await ingest(startup);
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
      if (outcome.rejected.length) {
        errors = outcome.rejected.map((item) => item.error.message);
      }
      if (outcome.images.length === 0) {
        return;
      }
      const known = new Map(images.map((image) => [image.imageId, image]));
      for (const image of outcome.images) {
        known.set(image.imageId, image);
      }
      images = [...known.values()];
      await activate(outcome.images[0].imageId);
    } catch (error) {
      errors = [appError(error).message];
    }
  }

  async function activate(imageId: string) {
    if (!host) {
      return;
    }
    try {
      const info = await host.selectImage(imageId);
      selected = info.imageId;
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
        selected = "";
        frame = null;
        frameMode = "empty";
        notice = "Ready";
        if (Host.available()) {
          await getCurrentWindow().setTitle("rawstock");
        }
        break;
      case "undo":
        notice = "Nothing to undo.";
        break;
      case "redo":
        notice = "Nothing to redo.";
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
      case "about":
        aboutOpen = true;
        break;
      case "licenses":
        licensesOpen = true;
        break;
      case "report-issue":
        if (host) {
          await openUrl("https://github.com/emperorpickles/rawstock/issues/new");
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
    settings = {
      ...settings,
      stock: { id: stock.id, revision: stock.revision },
    };
    notice = `${stock.name}, revision ${stock.revision}`;
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

  <div class="workspace">
    <Filmstrip images={images} selected={selected} onselect={(id) => void activate(id)} />
    <Viewer
      {frame}
      mode={selected && frame ? frameMode : "empty"}
      {zoom}
      fileName={selectedImage?.fileName ?? ""}
      {dropActive}
      imageWidth={cacheWidth}
      imageHeight={cacheHeight}
      onopen={() => void perform("open")}
      onview={viewChanged}
    />
    <SidePanel {stocks} stockId={settings.stock.id} onstock={chooseStock} />
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

<dialog bind:this={aboutDialog} aria-labelledby="about-title" onclose={() => (aboutOpen = false)}>
  <h2 id="about-title">About rawstock</h2>
  <p>rawstock 0.0.1 develops RAW photos through film stocks. This window is the app shell.</p>
  <p>Licensed under GPL-3.0-or-later.</p>
  <div class="dialog-actions">
    <button type="button" onclick={() => (aboutOpen = false)}>Close</button>
  </div>
</dialog>

<dialog bind:this={licensesDialog} aria-labelledby="licenses-title" onclose={() => (licensesOpen = false)}>
  <h2 id="licenses-title">Licenses</h2>
  <p>rawstock is free software under the GNU General Public License, version 3 or any later version. You can redistribute it and modify it under those terms. The full text ships with the source as LICENSE.</p>
  <ul>
    <li>rawler is LGPL-2.1-only.</li>
    <li>Tauri, serde, toml, and the Svelte toolchain are MIT or Apache-2.0.</li>
  </ul>
  <div class="dialog-actions">
    <button type="button" onclick={() => (licensesOpen = false)}>Close</button>
  </div>
</dialog>
