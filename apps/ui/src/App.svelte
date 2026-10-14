<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";

  import Controls from "./lib/Controls.svelte";
  import { CropRect } from "./lib/frame";
  import Preview from "./lib/Preview.svelte";
  import { PreviewLoop } from "./lib/previewLoop";
  import { CommandError, StockFileName } from "./lib/ranges";
  import SaveDialog from "./lib/SaveDialog.svelte";
  import { TauriHost } from "./lib/tauriHost";
  import type { ControlEdit, Frame, Progress, SessionState, StockState, StockSummary } from "./lib/types";

  const host = TauriHost.isAvailable() ? new TauriHost() : null;

  let stocks = $state<StockSummary[]>([]);
  let session = $state<SessionState | null>(null);
  let frame = $state<Frame | null>(null);
  let fidelity = $state("preview");
  let compare = $state(false);
  let mode = $state<"fit" | "pixel">("fit");
  let viewport = $state({ width: 0, height: 0 });
  let pan = $state({ x: 0, y: 0 });
  let fault = $state("");
  let notice = $state("");
  let progress = $state<Progress | null>(null);
  let booting = $state(true);
  let saving = $state(false);
  let working = $state("");

  const loop = host
    ? new PreviewLoop(
        host,
        () => ({
          ready: Boolean(session?.stock) && viewport.width > 0 && (session?.displayWidth ?? 0) > 0,
          compare,
          mode,
          displayWidth: session?.displayWidth ?? 0,
          displayHeight: session?.displayHeight ?? 0,
          viewportWidth: viewport.width,
          viewportHeight: viewport.height,
          panX: pan.x,
          panY: pan.y,
        }),
        (next) => {
          frame = next;
          fidelity = next.fidelity;
          progress = null;
          working = "";
        },
        (message) => {
          fault = message;
          void pull();
        },
      )
    : null;

  let resizeTimer: ReturnType<typeof setTimeout> | null = null;

  function remember(stock: StockState | null) {
    if (!stock || stocks.some((item) => item.path === stock.path)) {
      return;
    }
    stocks = [
      ...stocks,
      { path: stock.path, name: stock.name, model: stock.model, revision: stock.revision },
    ];
  }

  async function retitle() {
    const file = session?.fileName;
    const stock = session?.stock?.name;
    const title = file ? `Rawstock — ${file}${stock ? ` · ${stock}` : ""}` : "Rawstock";
    try {
      await getCurrentWindow().setTitle(title);
    } catch {
      // The vite page outside the shell has no window handle.
    }
  }

  async function pull() {
    if (!host) {
      return;
    }
    const next = await host.session();
    session = next;
    remember(next?.stock ?? null);
    await retitle();
    if (next?.stock) {
      loop?.now();
    }
  }

  async function publish(next: SessionState) {
    session = next;
    frame = null;
    await retitle();
    if (!next.stock && host) {
      const choice = stocks.find((item) => item.name.includes("Portra")) ?? stocks[0];
      if (choice) {
        session = await host.loadStock(choice.path);
        remember(session.stock);
      }
    } else {
      remember(next.stock);
    }
    await retitle();
    loop?.now();
  }

  function reportViewport(size: { width: number; height: number }) {
    if (size.width === viewport.width && size.height === viewport.height) {
      return;
    }
    viewport = size;
    if (resizeTimer) {
      clearTimeout(resizeTimer);
    }
    resizeTimer = setTimeout(() => loop?.settle(), 150);
  }

  function reportPan(x: number, y: number, phase: "drag" | "settle") {
    if (!session) {
      return;
    }
    const crop = CropRect.visible(
      session.displayWidth,
      session.displayHeight,
      viewport.width,
      viewport.height,
      x,
      y,
    );
    pan = { x: crop.x, y: crop.y };
    if (phase === "drag") {
      loop?.drag();
    } else {
      loop?.settle();
    }
  }

  function setMode(next: "fit" | "pixel") {
    mode = next;
    if (next === "pixel" && session) {
      pan = CropRect.centered(
        session.displayWidth,
        session.displayHeight,
        viewport.width,
        viewport.height,
      );
    }
    loop?.now();
  }

  function toggleCompare() {
    compare = !compare;
    loop?.now();
  }

  function onEdit(edit: ControlEdit) {
    if (!session?.stock) {
      return;
    }
    const stock = session.stock;
    if (edit.control === "exposure") stock.exposureEv = edit.value;
    if (edit.control === "printExposure") stock.printExposure = edit.value;
    if (edit.control === "dyeScales") stock.dyeScales = edit.value;
    if (edit.control === "grainAmount") stock.grainAmount = edit.value;
    if (edit.control === "halationEnabled") stock.halationEnabled = edit.value;
    if (edit.control === "halationThreshold") stock.halationThreshold = edit.value;
    if (edit.control === "halationRadius") stock.halationRadius = edit.value;
    if (edit.control === "halationRed") {
      stock.halationWeight = [edit.value, stock.halationWeight[1], stock.halationWeight[2]];
    }
    if (compare) {
      compare = false;
    }
    fault = "";
    notice = "";
    loop?.drag(edit);
  }

  async function openFile() {
    if (!host) return;
    const path = await TauriHost.pickRaw();
    if (!path) return;
    fault = "";
    notice = "";
    working = "Opening…";
    try {
      await publish(await host.openRaw(path));
    } catch (error) {
      fault = CommandError.message(error);
    } finally {
      working = "";
    }
  }

  async function openStock() {
    if (!host || !session) return;
    const path = await TauriHost.pickStock();
    if (!path) return;
    fault = "";
    try {
      session = await host.loadStock(path);
      remember(session.stock);
      frame = null;
      await retitle();
      loop?.now();
    } catch (error) {
      fault = CommandError.message(error);
    }
  }

  async function choose(path: string) {
    if (!host || !session) return;
    fault = "";
    notice = "";
    try {
      session = await host.loadStock(path);
      remember(session.stock);
      frame = null;
      await retitle();
      loop?.now();
    } catch (error) {
      fault = CommandError.message(error);
    }
  }

  async function exportJpeg() {
    if (!host || !session?.stock) return;
    const stem = session.fileName.replace(/\.[^.]+$/, "");
    const path = await TauriHost.pickJpeg(`${stem}.jpg`);
    if (!path) return;
    fault = "";
    notice = "";
    working = "Exporting the full frame…";
    try {
      await host.exportJpeg(path);
      notice = "Exported the exact full frame.";
    } catch (error) {
      if (!CommandError.isCancelled(error)) {
        fault = CommandError.message(error);
      }
    } finally {
      working = "";
    }
  }

  async function commitSave(draft: { name: string; source: string; license: string; revision: number }) {
    if (!host || !session?.stock) return;
    const path = await TauriHost.pickSave(StockFileName.suggest(draft.name, draft.revision));
    if (!path) return;
    fault = "";
    try {
      const stock = await host.saveStock({ ...draft, path });
      if (session) {
        session = { ...session, stock };
      }
      remember(stock);
      saving = false;
      notice = `Saved ${stock.name}. Dye scales reset to 1.`;
      loop?.now();
    } catch (error) {
      fault = CommandError.message(error);
    }
  }

  let status = $derived.by(() => {
    if (fault) return fault;
    if (working) return working;
    if (progress && progress.rows > 0) {
      const percent = Math.round((100 * progress.doneRows) / progress.rows);
      return `${progress.stage} ${percent}%`;
    }
    if (progress) return progress.stage;
    if (booting) return "Starting…";
    return notice;
  });

  let empty = $derived(
    session
      ? "Pick a stock to develop this file."
      : "Open a Fuji RAF or an iPhone ProRAW DNG.",
  );

  onMount(() => {
    if (!host || !loop) {
      booting = false;
      return;
    }
    const stops: Array<() => void> = [];
    let gone = false;
    void (async () => {
      stops.push(
        await host.onProgress((event) => {
          progress = event;
        }),
        await host.onSession(() => {
          void pull();
        }),
        await host.onSessionError((message) => {
          fault = message;
          booting = false;
        }),
      );
      if (gone) {
        for (const stop of stops) stop();
        return;
      }
      try {
        stocks = await host.listStocks();
        await pull();
      } catch (error) {
        fault = CommandError.message(error);
      } finally {
        booting = false;
      }
    })();
    return () => {
      gone = true;
      for (const stop of stops) stop();
      loop.dispose();
      if (resizeTimer) clearTimeout(resizeTimer);
    };
  });
</script>

{#if !host}
  <div class="shell-message">
    <div>
      <p class="word">Rawstock</p>
      <p>This window is the desktop shell. Start it with <code>npm run dev</code> in <code>apps/desktop</code>.</p>
    </div>
  </div>
{:else}
  <div class="shell">
    <header class="toolbar">
      <span class="word">Rawstock</span>
      <button type="button" class="primary" onclick={openFile}>Open RAW</button>
      <button type="button" disabled={!session?.stock} onclick={exportJpeg}>Export JPEG</button>
      <button type="button" disabled={!session?.stock} onclick={() => (saving = true)}>Save as</button>
      <span class="spacer"></span>
      <button type="button" class:on={mode === "fit"} onclick={() => setMode("fit")}>Fit</button>
      <button type="button" class:on={mode === "pixel"} onclick={() => setMode("pixel")}>100%</button>
      <button type="button" class:on={compare} disabled={!session?.stock} onclick={toggleCompare}>
        A/B
      </button>
    </header>
    <Preview
      {mode}
      {frame}
      {fidelity}
      {compare}
      fileName={session?.fileName ?? ""}
      {empty}
      pending={working}
      panX={pan.x}
      panY={pan.y}
      onviewport={reportViewport}
      onpan={reportPan}
    />
    <Controls
      {stocks}
      selected={session?.stock?.path ?? ""}
      stock={session?.stock ?? null}
      fileOpen={session !== null}
      onchoose={choose}
      onopenstock={openStock}
      onedit={onEdit}
    />
    <footer class="status" aria-live="polite">
      <span class:fault>{status}</span>
      {#if session}
        <span class="spacer"></span>
        <span>{session.displayWidth}×{session.displayHeight} · {session.orientation}</span>
      {/if}
    </footer>
  </div>
  {#if saving && session?.stock}
    <SaveDialog stock={session.stock} oncancel={() => (saving = false)} onsave={commitSave} />
  {/if}
{/if}
