<script lang="ts">
  import CropOverlay from "./CropOverlay.svelte";
  import type { CropRect, GeometryEdit } from "./contract";
  import { Delivery } from "./delivery";
  import type { PackedFrame } from "./frame";
  import { CropMath, GeometryView } from "./geometry";

  let {
    frame = null,
    mode = "empty",
    zoom = "fit",
    fileName = "",
    problem = "",
    dropActive = false,
    imageWidth = 0,
    imageHeight = 0,
    geometry = null,
    horizon = false,
    onopen,
    onview,
    oncrop,
    onstraighten,
    onsize,
  }: {
    frame?: PackedFrame | null;
    mode?: "texture" | "region" | "empty";
    zoom?: "fit" | 1 | 2;
    fileName?: string;
    problem?: string;
    dropActive?: boolean;
    imageWidth?: number;
    imageHeight?: number;
    geometry?: GeometryEdit | null;
    horizon?: boolean;
    onopen: () => void;
    onview: (rect: { x: number; y: number; width: number; height: number }) => void;
    oncrop?: (crop: CropRect, commit: boolean) => void;
    onstraighten?: (degrees: number, commit: boolean) => void;
    onsize?: (longEdge: number) => void;
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
  let line = $state<{ x: number; y: number; cx: number; cy: number } | null>(null);

  const turns = $derived(geometry?.quarterTurns ?? 0);
  const straighten = $derived(geometry?.straightenDeg ?? 0);
  const oriented = $derived(
    bitmap ? GeometryView.orientedSize(bitmap.width, bitmap.height, turns) : { width: 0, height: 0 },
  );
  const placement = $derived(
    zoom === "fit" ? GeometryView.fit(oriented.width, oriented.height, viewWidth, viewHeight) : null,
  );
  const shownCrop = $derived(geometry?.crop ?? { x: 0, y: 0, w: 1, h: 1 });
  const lockRatio = $derived(
    CropMath.pixelRatio(CropMath.infer(geometry?.crop, oriented.width, oriented.height), oriented.width, oriented.height),
  );

  $effect(() => {
    const edge = Math.max(viewWidth, viewHeight) * (window.devicePixelRatio || 1);
    if (edge > 0) {
      onsize?.(edge);
    }
  });

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
    const quarter = turns;
    const tilt = straighten;
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
    const plain = quarter === 0 && tilt === 0;
    if (plain && (mode === "region" || scale === 0)) {
      const fit = Math.min(width / source.width, height / source.height);
      const drawnW = source.width * (scale === 0 ? fit : scale);
      const drawnH = source.height * (scale === 0 ? fit : scale);
      context.imageSmoothingEnabled = scale !== 1;
      context.drawImage(source, (width - drawnW) / 2, (height - drawnH) / 2, drawnW, drawnH);
      return;
    }
    if (plain) {
      context.imageSmoothingEnabled = scale !== 1;
      context.drawImage(
        source,
        -originX * scale,
        -originY * scale,
        source.width * scale,
        source.height * scale,
      );
      return;
    }
    const orientedSize = GeometryView.orientedSize(source.width, source.height, quarter);
    const dest =
      scale === 0 || mode === "region"
        ? GeometryView.fit(orientedSize.width, orientedSize.height, width, height)
        : {
            x: -originX * scale,
            y: -originY * scale,
            width: orientedSize.width * scale,
            height: orientedSize.height * scale,
          };
    context.imageSmoothingEnabled = scale !== 1;
    context.save();
    context.translate(dest.x + dest.width / 2, dest.y + dest.height / 2);
    context.rotate((tilt * Math.PI) / 180);
    context.rotate((quarter * Math.PI) / 2);
    const pixel = orientedSize.width === 0 ? 1 : dest.width / orientedSize.width;
    context.scale(pixel, pixel);
    context.drawImage(source, -source.width / 2, -source.height / 2);
    context.restore();
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
    if (horizon && zoom === "fit" && placement) {
      line = { x: event.clientX, y: event.clientY, cx: event.clientX, cy: event.clientY };
      (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
      return;
    }
    if (zoom === "fit" || mode === "empty") {
      return;
    }
    dragging = true;
    lastX = event.clientX;
    lastY = event.clientY;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function pointerMove(event: PointerEvent) {
    if (line) {
      line = { ...line, cx: event.clientX, cy: event.clientY };
      const degrees = CropMath.horizon(line.x, line.y, event.clientX, event.clientY);
      if (degrees != null) {
        onstraighten?.(degrees, false);
      }
      return;
    }
    if (!dragging || zoom === "fit") {
      return;
    }
    const scale = zoom;
    panX = Math.max(0, panX - (event.clientX - lastX) / scale);
    panY = Math.max(0, panY - (event.clientY - lastY) / scale);
    lastX = event.clientX;
    lastY = event.clientY;
  }

  function pointerUp(event: PointerEvent) {
    if (line) {
      const degrees = CropMath.horizon(line.x, line.y, event.clientX, event.clientY);
      line = null;
      if (degrees != null) {
        onstraighten?.(degrees, true);
      }
      return;
    }
    dragging = false;
  }
</script>

<div class="surround" class:drop={dropActive}>
  <div
    class="stage"
    role="group"
    aria-label="Viewer"
    bind:this={stage}
    bind:clientWidth={viewWidth}
    bind:clientHeight={viewHeight}
    onpointerdown={pointerDown}
    onpointermove={pointerMove}
    onpointerup={pointerUp}
    onpointercancel={pointerUp}
  >
    {#if problem}
      <div class="empty">
        <p class="empty-title">{fileName}</p>
        <p>{problem}</p>
      </div>
    {:else if mode === "empty"}
      <div class="empty">
        <p class="empty-title">{fileName || "No photo open"}</p>
        <p>Drop a Fuji RAF or an iPhone ProRAW DNG here, or open one.</p>
        <button type="button" class="primary" onclick={onopen}>Open…</button>
      </div>
    {:else}
      <canvas
        bind:this={canvas}
        aria-label={fileName ? `Photograph, ${fileName}` : "Photograph"}
      ></canvas>
      {#if placement && placement.width > 0 && oncrop}
        <div
          class="frame"
          style:left="{placement.x}px"
          style:top="{placement.y}px"
          style:width="{placement.width}px"
          style:height="{placement.height}px"
        >
          {#if !horizon}
            <CropOverlay
              crop={shownCrop}
              width={oriented.width}
              height={oriented.height}
              {lockRatio}
              disabled={false}
              onpreview={(crop) => oncrop(crop, false)}
              oncommit={(crop) => oncrop(crop, true)}
            />
          {/if}
        </div>
      {/if}
      {#if line && stage}
        <svg class="horizon" aria-hidden="true">
          <line
            x1={line.x - stage.getBoundingClientRect().left}
            y1={line.y - stage.getBoundingClientRect().top}
            x2={line.cx - stage.getBoundingClientRect().left}
            y2={line.cy - stage.getBoundingClientRect().top}
          />
        </svg>
      {/if}
    {/if}
  </div>
</div>

<style>
  .surround {
    background: var(--surround, #2e2e2e);
    color: var(--surround-ink, #f2f2f2);
    min-width: 0;
    min-height: 0;
    padding: var(--gutter, 8px);
    display: grid;
  }

  .stage {
    position: relative;
    min-width: 0;
    min-height: 0;
  }

  .surround.drop {
    outline: 3px solid var(--accent, #f2f2f2);
    outline-offset: -6px;
  }

  .empty {
    display: grid;
    justify-items: center;
    align-content: center;
    gap: 0.75rem;
    max-width: 28rem;
    text-align: center;
    padding: 1.5rem;
    height: 100%;
    margin: 0 auto;
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
  }

  .frame {
    position: absolute;
    pointer-events: none;
  }

  .frame :global(.crop) {
    pointer-events: auto;
  }

  .horizon {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }

  .horizon line {
    stroke: #f2f2f2;
    stroke-width: 2;
  }
</style>
