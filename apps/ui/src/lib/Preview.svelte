<script lang="ts">
  import { untrack } from "svelte";

  import { ViewportCanvas, type DrawnPicture } from "./frame";

  let {
    mode,
    generation,
    picture,
    showing,
    fidelity,
    compare,
    fileName,
    empty,
    pending,
    panX,
    panY,
    viewWidth,
    viewHeight,
    displayWidth,
    displayHeight,
    onviewport,
    onpan,
  }: {
    mode: "fit" | "pixel";
    /** Bumps when the cached bitmap or the pan changes, so the canvas repaints. */
    generation: number;
    picture: () => DrawnPicture | null;
    showing: boolean;
    fidelity: string;
    compare: boolean;
    fileName: string;
    empty: string;
    pending: string;
    panX: number;
    panY: number;
    viewWidth: number;
    viewHeight: number;
    displayWidth: number;
    displayHeight: number;
    onviewport: (size: { width: number; height: number }) => void;
    onpan: (x: number, y: number) => void;
  } = $props();

  let stage: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  let drag: { x: number; y: number; panX: number; panY: number } | null = null;

  $effect(() => {
    const element = stage;
    if (!element) return;
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
    generation;
    mode;
    panX;
    panY;
    viewWidth;
    viewHeight;
    displayWidth;
    displayHeight;
    const target = canvas;
    if (!target || !showing) return;
    let cancelled = false;
    const handle = requestAnimationFrame(() => {
      if (cancelled) return;
      const latest = picture();
      if (!latest) return;
      ViewportCanvas.draw(
        target,
        latest,
        mode,
        panX,
        panY,
        viewWidth,
        viewHeight,
        displayWidth,
        displayHeight,
      );
    });
    return () => {
      cancelled = true;
      cancelAnimationFrame(handle);
    };
  });

  function pointerDown(event: PointerEvent) {
    if (mode !== "pixel") return;
    drag = { x: event.clientX, y: event.clientY, panX, panY };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function pointerMove(event: PointerEvent) {
    if (!drag) return;
    const dpr = window.devicePixelRatio || 1;
    onpan(drag.panX - (event.clientX - drag.x) * dpr, drag.panY - (event.clientY - drag.y) * dpr);
  }

  function pointerUp() {
    drag = null;
  }
</script>

<div class="stage" bind:this={stage}>
  <div class="badge">
    {#if fileName}
      <span>{fileName}</span>
    {/if}
    <span>{compare ? "Before" : "After"}</span>
    {#if showing}
      <span data-fidelity={fidelity}>{fidelity}</span>
    {/if}
  </div>
  {#if showing}
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
  {#if mode === "fit" && showing}
    <p class="note">
      Fit view is for tone and color. Grain size here is not the 48 µm cell. Judge grain at 100%.
    </p>
  {:else if mode === "pixel" && showing}
    <p class="note">Drag to pan the cached frame.</p>
  {/if}
</div>
