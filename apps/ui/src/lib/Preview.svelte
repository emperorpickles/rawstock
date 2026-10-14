<script lang="ts">
  import { untrack } from "svelte";

  import { PreviewCanvas } from "./frame";
  import type { Frame } from "./types";

  let {
    mode,
    frame,
    fidelity,
    compare,
    fileName,
    empty,
    pending,
    panX,
    panY,
    onviewport,
    onpan,
  }: {
    mode: "fit" | "pixel";
    frame: Frame | null;
    fidelity: string;
    compare: boolean;
    fileName: string;
    empty: string;
    pending: string;
    panX: number;
    panY: number;
    onviewport: (size: { width: number; height: number }) => void;
    onpan: (x: number, y: number, phase: "drag" | "settle") => void;
  } = $props();

  let stage: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  let drag: { x: number; y: number; panX: number; panY: number } | null = null;
  let drawId = 0;

  $effect(() => {
    const element = stage;
    if (!element) {
      return;
    }
    const report = () => {
      const rect = element.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      untrack(() =>
        onviewport({
          width: Math.max(1, Math.round(rect.width * dpr)),
          height: Math.max(1, Math.round(rect.height * dpr)),
        }),
      );
    };
    report();
    const observer = new ResizeObserver(report);
    observer.observe(element);
    return () => observer.disconnect();
  });

  $effect(() => {
    const target = canvas;
    const current = frame;
    const shown = mode;
    if (!target || !current) {
      return;
    }
    const id = ++drawId;
    void PreviewCanvas.draw(target, current, shown, () => id === drawId);
  });

  function pointerDown(event: PointerEvent) {
    if (mode !== "pixel") {
      return;
    }
    drag = { x: event.clientX, y: event.clientY, panX, panY };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function pointerMove(event: PointerEvent) {
    if (!drag) {
      return;
    }
    const dpr = window.devicePixelRatio || 1;
    onpan(
      drag.panX - (event.clientX - drag.x) * dpr,
      drag.panY - (event.clientY - drag.y) * dpr,
      "drag",
    );
  }

  function pointerUp() {
    if (!drag) {
      return;
    }
    drag = null;
    onpan(panX, panY, "settle");
  }
</script>

<div class="stage" bind:this={stage}>
  <div class="badge">
    {#if fileName}
      <span>{fileName}</span>
    {/if}
    <span>{compare ? "Before" : "After"}</span>
    {#if frame}
      <span>{fidelity}</span>
    {/if}
  </div>
  {#if frame}
    <canvas
      bind:this={canvas}
      class={mode}
      onpointerdown={pointerDown}
      onpointermove={pointerMove}
      onpointerup={pointerUp}
      onpointercancel={pointerUp}
    ></canvas>
  {:else}
    <p class="empty">{empty}</p>
  {/if}
  {#if pending}
    <p class="pending">{pending}</p>
  {/if}
  {#if mode === "fit" && frame}
    <p class="note">
      Fit view is for tone and color. Grain size here is not the 48 µm cell. Judge grain at 100%.
    </p>
  {:else if mode === "pixel" && frame}
    <p class="note">Drag to pan. Grain uses the full-frame cell.</p>
  {/if}
</div>
