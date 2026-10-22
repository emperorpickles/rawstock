<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";

  import Controls from "./lib/Controls.svelte";
  import { CropRect, FrameBitmap, ViewTurn, type DrawnPicture, type HeldPicture } from "./lib/frame";
  import Preview from "./lib/Preview.svelte";
  import { PreviewLoop, type PaintedFrame } from "./lib/previewLoop";
  import { CommandError, StockFileName } from "./lib/ranges";
  import { StockList } from "./lib/stockList";
  import SaveDialog from "./lib/SaveDialog.svelte";
  import { TauriHost } from "./lib/tauriHost";
  import type { ControlEdit, Progress, SessionState, StockState, StockSummary } from "./lib/types";
  import { DevelopPlan, PictureBook, type PictureSide } from "./lib/viewport";

  const host = TauriHost.isAvailable() ? new TauriHost() : null;

  let stocks = $state<StockSummary[]>([]);
  let session = $state<SessionState | null>(null);
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
  let scene = $state(0);
  let edit = $state(0);
  let paintGen = $state(0);
  let develops = $state(0);

  const book = new PictureBook<HeldPicture>();
  const slotTicket = new Map<string, number>();

  function release(image: HeldPicture) {
    image.bitmap.close();
  }

  function side(): PictureSide {
    return compare ? "before" : "after";
  }

  function retarget() {
    scene += 1;
    edit = 0;
    slotTicket.clear();
    book.invalidateScene(release);
    paintGen += 1;
  }

  const loop = host
    ? new PreviewLoop(
        host,
        () => ({
          ready: Boolean(session?.stock) && viewport.width > 0 && (session?.displayWidth ?? 0) > 0,
          compare,
          scene,
          edit,
        }),
        (painted) => {
          void keep(painted);
        },
        (message) => {
          fault = message;
          void pull();
        },
        () => {
          develops += 1;
        },
      )
    : null;

  function slotKey(painted: PaintedFrame): string {
    const editKey = painted.compare ? 0 : painted.edit;
    return `${painted.kind}:${painted.compare ? "before" : "after"}:${painted.scene}:${editKey}`;
  }

  async function keep(painted: PaintedFrame) {
    const key = slotKey(painted);
    slotTicket.set(key, Math.max(slotTicket.get(key) ?? 0, painted.frame.ticket));
    const held = await FrameBitmap.create(painted.frame, () => slotTicket.get(key) === painted.frame.ticket);
    if (!held) return;
    if (slotTicket.get(key) !== painted.frame.ticket || painted.scene !== scene) {
      held.bitmap.close();
      return;
    }
    if (!painted.compare && painted.edit !== edit) {
      held.bitmap.close();
      return;
    }
    const target: PictureSide = painted.compare ? "before" : "after";
    if (painted.kind === "full") book.putFull(target, painted.scene, painted.edit, held, release);
    else book.putFit(target, painted.scene, painted.edit, held, release);
    progress = null;
    working = "";
    paintGen += 1;
  }

  function drawn(): DrawnPicture | null {
    const picked = book.pick(mode, side(), scene, edit);
    if (!picked) return null;
    return { bitmap: picked.image.bitmap, full: picked.full };
  }

  function remember(stock: StockState | null) {
    stocks = StockList.include(stocks, stock);
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
    retarget();
    session = next;
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
    const first = viewport.width === 0;
    viewport = size;
    if (session && mode === "pixel") {
      const crop = CropRect.visible(
        session.displayWidth,
        session.displayHeight,
        size.width,
        size.height,
        pan.x,
        pan.y,
      );
      pan = { x: crop.x, y: crop.y };
    }
    paintGen += 1;
    if (first) loop?.now();
    else loop?.viewport("resize");
  }

  function reportPan(x: number, y: number) {
    if (!session) return;
    const crop = CropRect.visible(
      session.displayWidth,
      session.displayHeight,
      viewport.width,
      viewport.height,
      x,
      y,
    );
    pan = { x: crop.x, y: crop.y };
    loop?.viewport("pan");
    paintGen += 1;
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
    loop?.viewport("zoom");
    paintGen += 1;
  }

  async function rotate(direction: "clockwise" | "counterClockwise") {
    if (!host || !session) return;
    const previous = session;
    const previousPan = pan;
    fault = "";
    notice = "";
    try {
      const next = await host.rotate(direction);
      retarget();
      session = next;
      if (mode === "pixel") {
        pan = ViewTurn.panAfter(
          direction,
          previousPan.x,
          previousPan.y,
          viewport.width,
          viewport.height,
          previous.displayWidth,
          previous.displayHeight,
        );
      }
      loop?.now();
    } catch (error) {
      fault = CommandError.message(error);
      void pull();
    }
  }

  function onKey(event: KeyboardEvent) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target;
    if (
      target instanceof HTMLElement &&
      (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)
    ) {
      return;
    }
    if (event.key === "[") {
      event.preventDefault();
      void rotate("counterClockwise");
    } else if (event.key === "]") {
      event.preventDefault();
      void rotate("clockwise");
    }
  }

  function toggleCompare() {
    compare = !compare;
    const work = DevelopPlan.compare(book.canShow(side(), scene, edit));
    if (DevelopPlan.runs(work)) loop?.now();
    paintGen += 1;
  }

  function onEdit(control: ControlEdit) {
    if (!session?.stock) {
      return;
    }
    const stock = session.stock;
    if (control.control === "exposure") stock.exposureEv = control.value;
    if (control.control === "printExposure") stock.printExposure = control.value;
    if (control.control === "dyeScales") stock.dyeScales = control.value;
    if (control.control === "grainAmount") stock.grainAmount = control.value;
    if (control.control === "halationEnabled") stock.halationEnabled = control.value;
    if (control.control === "halationThreshold") stock.halationThreshold = control.value;
    if (control.control === "halationRadius") stock.halationRadius = control.value;
    if (control.control === "halationRed") {
      stock.halationWeight = [control.value, stock.halationWeight[1], stock.halationWeight[2]];
    }
    if (compare) {
      compare = false;
    }
    edit += 1;
    book.invalidateEdit(release);
    fault = "";
    notice = "";
    paintGen += 1;
    loop?.drag(control);
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
      const next = await host.loadStock(path);
      retarget();
      session = next;
      remember(session.stock);
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
      const next = await host.loadStock(path);
      retarget();
      session = next;
      remember(session.stock);
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
      retarget();
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

  let fidelity = $derived.by(() => {
    paintGen;
    return book.pick(mode, side(), scene, edit)?.image.fidelity ?? "preview";
  });

  let showing = $derived.by(() => {
    paintGen;
    return book.pick(mode, side(), scene, edit) !== null;
  });

  onMount(() => {
    window.addEventListener("keydown", onKey);
    if (!host || !loop) {
      booting = false;
      return () => window.removeEventListener("keydown", onKey);
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
        stocks = StockList.dedupe(await host.listStocks());
        await pull();
      } catch (error) {
        fault = CommandError.message(error);
      } finally {
        booting = false;
      }
    })();
    return () => {
      gone = true;
      window.removeEventListener("keydown", onKey);
      for (const stop of stops) stop();
      loop.dispose();
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
      <button
        type="button"
        class="icon"
        title="Rotate left ([)"
        aria-label="Rotate left"
        disabled={!session}
        onclick={() => rotate("counterClockwise")}>↺</button
      >
      <button
        type="button"
        class="icon"
        title="Rotate right (])"
        aria-label="Rotate right"
        disabled={!session}
        onclick={() => rotate("clockwise")}>↻</button
      >
      <button type="button" class:on={compare} disabled={!session?.stock} onclick={toggleCompare}>
        A/B
      </button>
    </header>
    <Preview
      {mode}
      generation={paintGen}
      picture={drawn}
      {showing}
      {fidelity}
      {compare}
      fileName={session?.fileName ?? ""}
      {empty}
      pending={working}
      panX={pan.x}
      panY={pan.y}
      viewWidth={viewport.width}
      viewHeight={viewport.height}
      displayWidth={session?.displayWidth ?? 0}
      displayHeight={session?.displayHeight ?? 0}
      onviewport={reportViewport}
      onpan={reportPan}
    />
    <Controls
      {stocks}
      selected={session?.stock?.id ?? ""}
      stock={session?.stock ?? null}
      fileOpen={session !== null}
      onchoose={choose}
      onopenstock={openStock}
      onedit={onEdit}
    />
    <footer class="status" aria-live="polite">
      <span class:fault>{status}</span>
      <span class="spacer"></span>
      <span class="develops" data-develop-count={develops}>{develops} develops</span>
      {#if session}
        <span>
          {session.displayWidth}×{session.displayHeight} · {session.orientation}{#if session.quarterTurns > 0}
            · +{session.quarterTurns}×90°{/if}
        </span>
      {/if}
    </footer>
  </div>
  {#if saving && session?.stock}
    <SaveDialog stock={session.stock} oncancel={() => (saving = false)} onsave={commitSave} />
  {/if}
{/if}
