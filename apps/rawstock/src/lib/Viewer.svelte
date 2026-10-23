<script lang="ts">
  import { Clipping, Divider, Viewport } from "./review/review";
  import type { Layer, Rect, ReviewView, Side, Zoom } from "./review/review";
  import type { PackedFrame } from "./frame";

  let {
    view = null,
    active = false,
    zoom = "fit",
    split = false,
    showBefore = false,
    clipping = false,
    fileName = "",
    problem = "",
    dropActive = false,
    onopen,
    onzoom,
    onregion,
    onsize,
  }: {
    view?: ReviewView | null;
    /** An image is selected and can be developed. */
    active?: boolean;
    zoom?: Zoom;
    split?: boolean;
    showBefore?: boolean;
    clipping?: boolean;
    fileName?: string;
    problem?: string;
    dropActive?: boolean;
    onopen: () => void;
    onzoom: (zoom: Zoom) => void;
    /** Visible image rectangle at 100% or 200%, in image pixels. */
    onregion: (rect: Rect) => void;
    /** The viewer's long edge in device pixels, for the proxy size. */
    onsize: (longEdge: number) => void;
  } = $props();

  /** The surround's neutral gutter around the photo, in CSS pixels. */
  const GUTTER = 8;

  let canvas = $state<HTMLCanvasElement | null>(null);
  let stage = $state<HTMLDivElement | null>(null);
  let viewWidth = $state(0);
  let viewHeight = $state(0);
  let divider = $state(0.5);
  let dragging = false;
  let movingDivider = false;
  let lastX = 0;
  let lastY = 0;
  let frameRequest = 0;
  let reported = "";
  let shownImage = "";

  // Pan is a normalised image centre, so it survives the proxy giving way
  // to the full frame. It is not reactive: the canvas redraws on demand.
  let center = { x: 0.5, y: 0.5 };

  // Decoded bitmaps per frame. A side keeps drawing its last ready layer
  // until the next one is decoded, so a swap is never partial.
  const bitmaps = new Map<PackedFrame, ImageBitmap>();
  const masks = new Map<PackedFrame, ImageBitmap>();
  const decoding = new Set<PackedFrame>();
  const masking = new Set<PackedFrame>();
  let shown: Record<Side, Layer | null> = { after: null, before: null };

  const hasLayer = $derived(view !== null && (view.after !== null || view.before !== null));
  const zoomLabel = $derived(zoom === "fit" ? "Fit" : `${zoom * 100}%`);
  const statusLabel = $derived.by(() => {
    if (!view || !hasLayer) {
      return "";
    }
    if (zoom === "fit") {
      return "Proxy, not for judging grain";
    }
    if (view.proxy) {
      return "Proxy, developing full resolution…";
    }
    if (view.updating) {
      return "Updating…";
    }
    return "";
  });
  const beforeNote = $derived(
    view?.beforeProxy && (split || showBefore) ? "Before is the proxy at this zoom" : "",
  );

  $effect(() => {
    const ratio = window.devicePixelRatio || 1;
    if (viewWidth > 0 && viewHeight > 0) {
      onsize(Math.max(viewWidth, viewHeight) * ratio);
    }
  });

  $effect(() => {
    // Redraw whenever anything drawn changes.
    void [view, zoom, split, showBefore, clipping, divider, viewWidth, viewHeight, canvas];
    if (view?.imageId !== shownImage) {
      shownImage = view?.imageId ?? "";
      center = { x: 0.5, y: 0.5 };
      shown = { after: null, before: null };
      reported = "";
    }
    sync();
    redraw();
  });

  $effect(() => {
    const node = stage;
    if (!node) {
      return;
    }
    const onWheel = (event: WheelEvent) => wheel(event);
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  });

  $effect(() => {
    return () => {
      cancelAnimationFrame(frameRequest);
      for (const bitmap of [...bitmaps.values(), ...masks.values()]) {
        bitmap.close();
      }
      bitmaps.clear();
      masks.clear();
    };
  });

  /** Decode what the view needs and close what nothing draws. */
  function sync() {
    const wanted = new Set<PackedFrame>();
    for (const layer of [view?.after, view?.before, shown.after, shown.before]) {
      if (layer) {
        wanted.add(layer.frame);
      }
    }
    for (const layer of [view?.after, view?.before]) {
      if (layer) {
        decode(layer.frame);
        if (clipping) {
          mask(layer.frame);
        }
      }
    }
    for (const [frame, bitmap] of bitmaps) {
      if (!wanted.has(frame)) {
        bitmap.close();
        bitmaps.delete(frame);
      }
    }
    for (const [frame, bitmap] of masks) {
      if (!wanted.has(frame) || !clipping) {
        bitmap.close();
        masks.delete(frame);
      }
    }
  }

  function decode(frame: PackedFrame) {
    if (bitmaps.has(frame) || decoding.has(frame)) {
      return;
    }
    decoding.add(frame);
    void createImageBitmap(new ImageData(frame.rgba, frame.width, frame.height))
      .then((bitmap) => {
        bitmaps.set(frame, bitmap);
        sync();
        redraw();
      })
      .finally(() => decoding.delete(frame));
  }

  function mask(frame: PackedFrame) {
    if (masks.has(frame) || masking.has(frame)) {
      return;
    }
    masking.add(frame);
    const overlay = Clipping.mask(frame.rgba, frame.width, frame.height);
    void createImageBitmap(new ImageData(overlay, frame.width, frame.height))
      .then((bitmap) => {
        masks.set(frame, bitmap);
        sync();
        redraw();
      })
      .finally(() => masking.delete(frame));
  }

  function redraw() {
    cancelAnimationFrame(frameRequest);
    frameRequest = requestAnimationFrame(draw);
  }

  function geometry() {
    const ratio = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.floor(viewWidth * ratio));
    const height = Math.max(1, Math.floor(viewHeight * ratio));
    const imageWidth = view?.width ?? 0;
    const imageHeight = view?.height ?? 0;
    const gutter = GUTTER * ratio;
    const scale = Viewport.scale(zoom, imageWidth, imageHeight, width, height, gutter);
    return { ratio, width, height, imageWidth, imageHeight, gutter, scale };
  }

  function draw() {
    const node = canvas;
    if (!node || viewWidth === 0 || viewHeight === 0) {
      return;
    }
    const { width, height, imageWidth, imageHeight, gutter, scale } = geometry();
    if (node.width !== width || node.height !== height) {
      node.width = width;
      node.height = height;
    }
    const context = node.getContext("2d");
    if (!context) {
      return;
    }
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, width, height);
    if (!view || imageWidth === 0 || imageHeight === 0) {
      return;
    }
    center = Viewport.clampCenter(center, imageWidth, imageHeight, width, height, scale, gutter);
    const origin = Viewport.origin(center, imageWidth, imageHeight, width, height, scale);
    const visible = Viewport.visibleRect(center, imageWidth, imageHeight, width, height, scale);
    if (zoom !== "fit") {
      const key = `${view.imageId}:${visible.x}:${visible.y}:${visible.width}:${visible.height}`;
      if (key !== reported) {
        reported = key;
        onregion(visible);
      }
    }

    const splitAt = Math.round(divider * width);
    const panes: Array<{ side: Side; from: number; to: number }> = showBefore
      ? [{ side: "before", from: 0, to: width }]
      : split
        ? [
            { side: "before", from: 0, to: splitAt },
            { side: "after", from: splitAt, to: width },
          ]
        : [{ side: "after", from: 0, to: width }];

    for (const pane of panes) {
      const layer = current(pane.side);
      if (!layer || pane.to <= pane.from) {
        continue;
      }
      context.save();
      context.beginPath();
      context.rect(pane.from, 0, pane.to - pane.from, height);
      context.clip();
      paint(context, bitmaps.get(layer.frame), layer, visible, origin, scale);
      if (clipping) {
        paint(context, masks.get(layer.frame), layer, visible, origin, scale, true);
      }
      context.restore();
    }
  }

  /** The newest decoded layer for a side, else the one already on screen. */
  function current(side: Side): Layer | null {
    const wanted = view?.[side] ?? null;
    if (wanted && bitmaps.has(wanted.frame)) {
      shown[side] = wanted;
    }
    const layer = shown[side];
    return layer && bitmaps.has(layer.frame) ? layer : null;
  }

  function paint(
    context: CanvasRenderingContext2D,
    bitmap: ImageBitmap | undefined,
    layer: Layer,
    visible: Rect,
    origin: { x: number; y: number },
    scale: number,
    nearest = false,
  ) {
    if (!bitmap) {
      return;
    }
    const area = Viewport.intersect(visible, layer.rect);
    if (!area) {
      return;
    }
    const fx = layer.frame.width / layer.rect.width;
    const fy = layer.frame.height / layer.rect.height;
    // Full-resolution pixels are never smoothed at 100% and 200%.
    context.imageSmoothingEnabled = !nearest && !(layer.exact && scale >= 1);
    context.imageSmoothingQuality = "high";
    context.drawImage(
      bitmap,
      (area.x - layer.rect.x) * fx,
      (area.y - layer.rect.y) * fy,
      area.width * fx,
      area.height * fy,
      origin.x + area.x * scale,
      origin.y + area.y * scale,
      area.width * scale,
      area.height * scale,
    );
  }

  function panBy(dx: number, dy: number) {
    const { ratio, imageWidth, imageHeight, scale } = geometry();
    if (imageWidth === 0 || imageHeight === 0) {
      return;
    }
    center = {
      x: center.x - (dx * ratio) / (imageWidth * scale),
      y: center.y - (dy * ratio) / (imageHeight * scale),
    };
    redraw();
  }

  function pointerDown(event: PointerEvent) {
    if (zoom === "fit" || !hasLayer || event.button !== 0) {
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
    panBy(event.clientX - lastX, event.clientY - lastY);
    lastX = event.clientX;
    lastY = event.clientY;
  }

  function pointerUp() {
    dragging = false;
  }

  function wheel(event: WheelEvent) {
    if (zoom === "fit" || !hasLayer || event.ctrlKey) {
      return;
    }
    event.preventDefault();
    panBy(-event.deltaX, -event.deltaY);
  }

  /** Fit and 100% swap, keeping the clicked pixel under the pointer. */
  function doubleClick(event: MouseEvent) {
    if (!hasLayer || !canvas) {
      return;
    }
    if (zoom !== "fit") {
      onzoom("fit");
      return;
    }
    const { ratio, width, height, imageWidth, imageHeight, scale } = geometry();
    const bounds = canvas.getBoundingClientRect();
    const pointer = { x: (event.clientX - bounds.left) * ratio, y: (event.clientY - bounds.top) * ratio };
    const fitCenter = Viewport.clampCenter(center, imageWidth, imageHeight, width, height, scale, 0);
    const pixel = Viewport.imageAt(pointer, fitCenter, imageWidth, imageHeight, width, height, scale);
    center = Viewport.centerKeeping(pixel, pointer, imageWidth, imageHeight, width, height, 1);
    onzoom(1);
  }

  function dividerDown(event: PointerEvent) {
    movingDivider = true;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    event.stopPropagation();
  }

  function dividerMove(event: PointerEvent) {
    if (!movingDivider || !stage) {
      return;
    }
    const bounds = stage.getBoundingClientRect();
    divider = Divider.clamp((event.clientX - bounds.left) / Math.max(1, bounds.width));
  }

  function dividerUp() {
    movingDivider = false;
  }

  function dividerKey(event: KeyboardEvent) {
    const next = Divider.key(divider, event.key, event.shiftKey);
    if (next === null) {
      return;
    }
    // Arrow keys also step the filmstrip; the focused divider takes them.
    event.preventDefault();
    event.stopPropagation();
    divider = next;
  }
</script>

<div
  class="surround"
  class:drop={dropActive}
  bind:this={stage}
  bind:clientWidth={viewWidth}
  bind:clientHeight={viewHeight}
>
  {#if problem}
    <div class="empty">
      <p class="empty-title">{fileName}</p>
      <p>{problem}</p>
    </div>
  {:else if !active}
    <div class="empty">
      <p class="empty-title">{fileName || "No photo open"}</p>
      <p>Drop a Fuji RAF or an iPhone ProRAW DNG here, or open one.</p>
      <button type="button" class="primary" onclick={onopen}>Open…</button>
    </div>
  {:else}
    <canvas
      bind:this={canvas}
      class:zoomed={zoom !== "fit"}
      aria-label={fileName ? `Photograph, ${fileName}, ${zoomLabel}` : "Photograph"}
      onpointerdown={pointerDown}
      onpointermove={pointerMove}
      onpointerup={pointerUp}
      onpointercancel={pointerUp}
      ondblclick={doubleClick}
    ></canvas>

    {#if !hasLayer}
      <p class="developing" role="status">Developing {fileName}…</p>
    {:else}
      <div class="badges top-left">
        {#if showBefore}
          <span class="badge strong">Before</span>
        {:else if split}
          <span class="badge strong">Before</span>
        {/if}
        <span class="badge mono">{zoomLabel}</span>
        {#if statusLabel}<span class="badge" role="status">{statusLabel}</span>{/if}
        {#if beforeNote}<span class="badge">{beforeNote}</span>{/if}
      </div>
      {#if split && !showBefore}
        <div class="badges top-right">
          <span class="badge strong">After</span>
        </div>
        <div
          class="divider"
          style:left="{divider * 100}%"
          role="slider"
          tabindex="0"
          aria-label="Before and after divider"
          aria-orientation="horizontal"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={Math.round(divider * 100)}
          aria-valuetext="{Math.round(divider * 100)} percent before"
          onpointerdown={dividerDown}
          onpointermove={dividerMove}
          onpointerup={dividerUp}
          onpointercancel={dividerUp}
          onkeydown={dividerKey}
        >
          <span class="handle" aria-hidden="true"></span>
        </div>
      {/if}
      {#if clipping}
        <div class="badges bottom-left" aria-label="Clipping overlay legend">
          <span class="badge"><span class="swatch highlight" aria-hidden="true"></span>Highlights clipped</span>
          <span class="badge"><span class="swatch shadow" aria-hidden="true"></span>Shadows clipped</span>
        </div>
      {/if}
    {/if}
  {/if}
</div>

<style>
  /* The surround is never themed: neutral grey from Preferences. */
  .surround {
    background: var(--surround, #2e2e2e);
    color: var(--surround-ink, #f2f2f2);
    min-width: 0;
    min-height: 0;
    position: relative;
    display: grid;
    place-items: center;
    overflow: hidden;
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
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
    touch-action: none;
  }

  canvas.zoomed {
    cursor: grab;
  }

  canvas.zoomed:active {
    cursor: grabbing;
  }

  .developing {
    position: relative;
    margin: 0;
  }

  .badges {
    position: absolute;
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    pointer-events: none;
    max-width: calc(50% - 1.5rem);
  }

  .top-left {
    top: 0.75rem;
    left: 0.75rem;
  }

  .top-right {
    top: 0.75rem;
    right: 0.75rem;
    justify-content: flex-end;
  }

  .bottom-left {
    bottom: 0.75rem;
    left: 0.75rem;
  }

  /* Neutral chips over the photo, so nothing tints the judgement. */
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.15rem 0.5rem;
    border-radius: 4px;
    background: rgba(20, 20, 20, 0.78);
    color: #f2f2f2;
    font-size: 0.8rem;
    line-height: 1.4;
    white-space: nowrap;
  }

  .badge.strong {
    font-weight: 600;
  }

  .mono {
    font-family: "JetBrains Mono", ui-monospace, "SF Mono", Consolas, monospace;
    font-variant-numeric: tabular-nums;
  }

  .swatch {
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 2px;
  }

  .swatch.highlight {
    background: rgb(255, 48, 48);
  }

  .swatch.shadow {
    background: rgb(40, 120, 255);
  }

  .divider {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1.5rem;
    margin-left: -0.75rem;
    cursor: ew-resize;
    touch-action: none;
    display: grid;
    place-items: center;
  }

  .divider::before {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    left: calc(50% - 1px);
    width: 2px;
    background: #f2f2f2;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.35);
  }

  .handle {
    position: relative;
    width: 1.1rem;
    height: 2.2rem;
    border-radius: 999px;
    background: #f2f2f2;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.35);
  }

  .divider:focus-visible {
    outline: none;
  }

  .divider:focus-visible .handle {
    outline: 2px solid #f7f7f7;
    outline-offset: 2px;
  }
</style>
