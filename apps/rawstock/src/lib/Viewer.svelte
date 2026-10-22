<script lang="ts">
  import { Delivery } from "./delivery";
  import type { PackedFrame } from "./frame";

  let {
    frame = null,
    mode = "empty",
    zoom = "fit",
    fileName = "",
    dropActive = false,
    imageWidth = 0,
    imageHeight = 0,
    onopen,
    onview,
  }: {
    frame?: PackedFrame | null;
    mode?: "texture" | "region" | "empty";
    zoom?: "fit" | 1 | 2;
    fileName?: string;
    dropActive?: boolean;
    imageWidth?: number;
    imageHeight?: number;
    onopen: () => void;
    onview: (rect: { x: number; y: number; width: number; height: number }) => void;
  } = $props();

  let canvas = $state<HTMLCanvasElement | null>(null);
  let stage = $state<HTMLDivElement | null>(null);
  let viewWidth = $state(0);
  let viewHeight = $state(0);
  let panX = $state(0);
  let panY = $state(0);
  let dragging = $state(false);
  let lastX = 0;
  let lastY = 0;
  let bitmap = $state<ImageBitmap | null>(null);

  $effect(() => {
    const current = frame;
    let cancelled = false;
    if (!current) {
      bitmap = null;
      return;
    }
    const image = new ImageData(current.rgba, current.width, current.height);
    void createImageBitmap(image).then((next) => {
      if (cancelled) {
        next.close();
        return;
      }
      bitmap?.close();
      bitmap = next;
    });
    return () => {
      cancelled = true;
    };
  });

  $effect(() => {
    const node = canvas;
    const source = bitmap;
    const width = viewWidth;
    const height = viewHeight;
    const scale = zoom === "fit" ? 0 : zoom;
    const originX = panX;
    const originY = panY;
    if (!node || width === 0 || height === 0) {
      return;
    }
    const ratio = window.devicePixelRatio || 1;
    node.width = Math.max(1, Math.floor(width * ratio));
    node.height = Math.max(1, Math.floor(height * ratio));
    const context = node.getContext("2d");
    if (!context) {
      return;
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    if (!source) {
      return;
    }
    if (mode === "region" || scale === 0) {
      const fit = Math.min(width / source.width, height / source.height);
      const drawnW = source.width * (scale === 0 ? fit : scale);
      const drawnH = source.height * (scale === 0 ? fit : scale);
      context.imageSmoothingEnabled = scale !== 1;
      context.drawImage(source, (width - drawnW) / 2, (height - drawnH) / 2, drawnW, drawnH);
      return;
    }
    context.imageSmoothingEnabled = scale !== 1;
    context.drawImage(source, -originX * scale, -originY * scale, source.width * scale, source.height * scale);
  });

  let reported = "";

  $effect(() => {
    if (mode !== "region" || imageWidth === 0 || imageHeight === 0 || viewWidth === 0) {
      return;
    }
    const scale = zoom === "fit" ? 1 : zoom;
    const rect = Delivery.visibleRect(imageWidth, imageHeight, viewWidth, viewHeight, scale, panX, panY);
    const key = `${rect.x}:${rect.y}:${rect.width}:${rect.height}`;
    if (key === reported) {
      return;
    }
    reported = key;
    onview(rect);
  });

  function pointerDown(event: PointerEvent) {
    if (zoom === "fit" || mode === "empty") {
      return;
    }
    dragging = true;
    lastX = event.clientX;
    lastY = event.clientY;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function pointerMove(event: PointerEvent) {
    if (!dragging || zoom === "fit") {
      return;
    }
    const scale = zoom;
    panX = Math.max(0, panX - (event.clientX - lastX) / scale);
    panY = Math.max(0, panY - (event.clientY - lastY) / scale);
    lastX = event.clientX;
    lastY = event.clientY;
  }

  function pointerUp() {
    dragging = false;
  }
</script>

<div
  class="surround"
  class:drop={dropActive}
  bind:this={stage}
  bind:clientWidth={viewWidth}
  bind:clientHeight={viewHeight}
>
  {#if mode === "empty"}
    <div class="empty">
      <p class="empty-title">{fileName || "No photo open"}</p>
      <p>Drop a Fuji RAF or an iPhone ProRAW DNG here, or open one.</p>
      <button type="button" class="primary" onclick={onopen}>Open…</button>
    </div>
  {:else}
    <canvas
      bind:this={canvas}
      aria-label={fileName ? `Photograph, ${fileName}` : "Photograph"}
      onpointerdown={pointerDown}
      onpointermove={pointerMove}
      onpointerup={pointerUp}
      onpointercancel={pointerUp}
    ></canvas>
  {/if}
</div>

<style>
  .surround {
    background: #7a7a7a;
    color: #141414;
    min-width: 0;
    min-height: 0;
    position: relative;
    display: grid;
    place-items: center;
  }

  .surround.drop {
    outline: 3px solid #f2f2f2;
    outline-offset: -6px;
  }

  .empty {
    display: grid;
    justify-items: center;
    gap: 0.75rem;
    max-width: 28rem;
    text-align: center;
    padding: 1.5rem;
  }

  .empty-title {
    margin: 0;
    font-size: 1.25rem;
  }

  .empty p {
    margin: 0;
  }

  canvas {
    width: 100%;
    height: 100%;
    display: block;
    cursor: grab;
    touch-action: none;
  }
</style>
