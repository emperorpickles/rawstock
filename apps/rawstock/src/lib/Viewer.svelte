<script lang="ts">
  import type { Snippet } from "svelte";
  import CropOverlay from "./CropOverlay.svelte";
  import type { CropRect, GeometryEdit } from "./contract";
  import type { PackedFrame } from "./frame";
  import { CropMath, Framing, GeometryView } from "./geometry";
  import type { Affine, FrameRect } from "./geometry";
  import { Clipping, Divider, Viewport } from "./review/review";
  import type { Layer, Rect, ReviewView, Side, Zoom } from "./review/review";

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
    edit = null,
    horizon = false,
    guides = false,
    cropping = false,
    onopen,
    onzoom,
    onregion,
    oncrop,
    onstraighten,
    onsize,
    beforeTag,
    afterTag,
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
    edit?: GeometryEdit | null;
    horizon?: boolean;
    /** Show the straighten grid, while the straighten slider moves. */
    guides?: boolean;
    /**
     * Crop mode: the whole turned, straightened frame with the crop handles.
     * Otherwise the viewer shows only the cropped photo, as it exports.
     */
    cropping?: boolean;
    onopen: () => void;
    onzoom: (zoom: Zoom) => void;
    /** Visible image rectangle at 100% or 200%, in image pixels. */
    onregion: (rect: Rect) => void;
    oncrop?: (crop: CropRect, commit: boolean) => void;
    onstraighten?: (degrees: number, commit: boolean) => void;
    /** The viewer's long edge in device pixels, for the proxy size. */
    onsize: (longEdge: number) => void;
    /** Drawn in place of the Before badge, such as the A stock picker. */
    beforeTag?: Snippet;
    /** Drawn in place of the After badge, such as the B stock picker. */
    afterTag?: Snippet;
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
  /** The part of the frame `center` was last measured in. */
  let centerIn: FrameRect | null = null;

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
  let line = $state<{ x: number; y: number; cx: number; cy: number; base: number } | null>(null);
  /** The framed photo on screen, in CSS pixels. The crop overlay sits on it. */
  let frameBox = $state<{ left: number; top: number; width: number; height: number } | null>(null);

  const turns = $derived((((edit?.quarterTurns ?? 0) % 4) + 4) % 4);
  const straighten = $derived(edit?.straightenDeg ?? 0);
  /** The turned, straightened frame: the export's pixels before the crop. */
  const oriented = $derived(
    view && view.width > 0 ? GeometryView.orientedSize(view.width, view.height, turns) : { width: 0, height: 0 },
  );
  const toFrame = $derived<Affine | null>(
    view && view.width > 0 ? Framing.imageToFrame(view.width, view.height, turns, straighten) : null,
  );
  const shownCrop = $derived(edit?.crop ?? { x: 0, y: 0, w: 1, h: 1 });
  /** The frame pixels on screen: the export's own crop, or all of it in crop mode. */
  const viewed = $derived<FrameRect>(
    Framing.cropPixels(cropping ? undefined : edit?.crop, oriented.width, oriented.height),
  );
  const lockRatio = $derived(
    CropMath.pixelRatio(CropMath.infer(edit?.crop, oriented.width, oriented.height), oriented.width, oriented.height),
  );

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
    void [view, zoom, split, showBefore, clipping, divider, viewWidth, viewHeight, canvas, edit, toFrame, viewed];
    if (view?.imageId !== shownImage) {
      shownImage = view?.imageId ?? "";
      center = { x: 0.5, y: 0.5 };
      centerIn = null;
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

  /**
   * Zoom and pan work in the framed photo, so 100% is one export pixel per
   * device pixel and the pan follows the turned, straightened frame. Outside
   * crop mode that frame is the crop.
   */
  function geometry() {
    const ratio = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.floor(viewWidth * ratio));
    const height = Math.max(1, Math.floor(viewHeight * ratio));
    const frameWidth = viewed.width;
    const frameHeight = viewed.height;
    const gutter = GUTTER * ratio;
    const scale = Viewport.scale(zoom, frameWidth, frameHeight, width, height, gutter);
    return { ratio, width, height, frameWidth, frameHeight, gutter, scale };
  }

  function draw() {
    const node = canvas;
    if (!node || viewWidth === 0 || viewHeight === 0) {
      return;
    }
    const { ratio, width, height, frameWidth, frameHeight, gutter, scale } = geometry();
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
    if (!view || !toFrame || frameWidth === 0 || frameHeight === 0) {
      placeFrame(null);
      return;
    }
    if (centerIn && (centerIn.x !== viewed.x || centerIn.y !== viewed.y || centerIn.width !== frameWidth || centerIn.height !== frameHeight)) {
      // Entering or leaving crop mode keeps the same spot in the middle.
      center = Framing.recenter(center, centerIn, viewed);
    }
    centerIn = viewed;
    center = Viewport.clampCenter(center, frameWidth, frameHeight, width, height, scale, gutter);
    const origin = Viewport.origin(center, frameWidth, frameHeight, width, height, scale);
    const shownFrame = Viewport.visibleRect(center, frameWidth, frameHeight, width, height, scale);
    const toViewed = Framing.cropped(toFrame, viewed);
    // The camera-frame pixels under the visible part of the frame.
    const visible = Framing.imageBounds(toViewed, shownFrame, view.width, view.height);
    if (zoom !== "fit") {
      const key = `${view.imageId}:${visible.x}:${visible.y}:${visible.width}:${visible.height}`;
      if (key !== reported) {
        reported = key;
        onregion(visible);
      }
    }
    placeFrame({
      left: origin.x / ratio,
      top: origin.y / ratio,
      width: (frameWidth * scale) / ratio,
      height: (frameHeight * scale) / ratio,
    });

    const splitAt = Math.round(divider * width);
    const panes: Array<{ side: Side; from: number; to: number }> = showBefore
      ? [{ side: "before", from: 0, to: width }]
      : split
        ? [
            { side: "before", from: 0, to: splitAt },
            { side: "after", from: splitAt, to: width },
          ]
        : [{ side: "after", from: 0, to: width }];

    const toCanvas = Framing.then(toViewed, { a: scale, b: 0, c: 0, d: scale, e: origin.x, f: origin.y });
    // Whole pixels stay sharp only when the straighten angle is level.
    const level = Math.abs(straighten) < 1e-6;
    for (const pane of panes) {
      const layer = current(pane.side);
      if (!layer || pane.to <= pane.from) {
        continue;
      }
      const left = Math.max(pane.from, origin.x);
      const right = Math.min(pane.to, origin.x + frameWidth * scale);
      if (right <= left) {
        continue;
      }
      context.save();
      context.beginPath();
      // The frame's edge is the export's edge; a straightened photo's
      // overhang and, outside crop mode, the cropped-off edges are not drawn.
      context.rect(left, origin.y, right - left, frameHeight * scale);
      context.clip();
      context.setTransform(toCanvas.a, toCanvas.b, toCanvas.c, toCanvas.d, toCanvas.e, toCanvas.f);
      paint(context, bitmaps.get(layer.frame), layer, visible, level && layer.exact && scale >= 1);
      if (clipping) {
        paint(context, masks.get(layer.frame), layer, visible, true);
      }
      context.restore();
    }
  }

  function placeFrame(next: { left: number; top: number; width: number; height: number } | null) {
    const same =
      next === frameBox ||
      (next !== null &&
        frameBox !== null &&
        Math.abs(next.left - frameBox.left) < 0.01 &&
        Math.abs(next.top - frameBox.top) < 0.01 &&
        Math.abs(next.width - frameBox.width) < 0.01 &&
        Math.abs(next.height - frameBox.height) < 0.01);
    if (!same) {
      frameBox = next;
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

  /** The part of a layer under `visible`, drawn in camera-frame pixels. */
  function paint(
    context: CanvasRenderingContext2D,
    bitmap: ImageBitmap | undefined,
    layer: Layer,
    visible: Rect,
    nearest: boolean,
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
    context.imageSmoothingEnabled = !nearest;
    context.imageSmoothingQuality = "high";
    context.drawImage(
      bitmap,
      (area.x - layer.rect.x) * fx,
      (area.y - layer.rect.y) * fy,
      area.width * fx,
      area.height * fy,
      area.x,
      area.y,
      area.width,
      area.height,
    );
  }

  function panBy(dx: number, dy: number) {
    const { ratio, frameWidth, frameHeight, scale } = geometry();
    if (frameWidth === 0 || frameHeight === 0) {
      return;
    }
    center = {
      x: center.x - (dx * ratio) / (frameWidth * scale),
      y: center.y - (dy * ratio) / (frameHeight * scale),
    };
    redraw();
  }

  function pointerDown(event: PointerEvent) {
    if (horizon && frameBox && event.button === 0) {
      // The photo on screen is already straightened by `base`.
      line = { x: event.clientX, y: event.clientY, cx: event.clientX, cy: event.clientY, base: straighten };
      (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
      return;
    }
    if (zoom === "fit" || !hasLayer || event.button !== 0) {
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
        onstraighten?.(CropMath.levelled(line.base, degrees), false);
      }
      return;
    }
    if (!dragging || zoom === "fit") {
      return;
    }
    panBy(event.clientX - lastX, event.clientY - lastY);
    lastX = event.clientX;
    lastY = event.clientY;
  }

  function pointerUp(event: PointerEvent) {
    if (line) {
      const degrees = CropMath.horizon(line.x, line.y, event.clientX, event.clientY);
      const base = line.base;
      line = null;
      if (degrees != null) {
        onstraighten?.(CropMath.levelled(base, degrees), true);
      } else if (base !== straighten) {
        // A line too short to read puts the angle back.
        onstraighten?.(base, false);
      }
      return;
    }
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
    const { ratio, width, height, frameWidth, frameHeight, scale } = geometry();
    const bounds = canvas.getBoundingClientRect();
    const pointer = { x: (event.clientX - bounds.left) * ratio, y: (event.clientY - bounds.top) * ratio };
    const fitCenter = Viewport.clampCenter(center, frameWidth, frameHeight, width, height, scale, 0);
    const pixel = Viewport.imageAt(pointer, fitCenter, frameWidth, frameHeight, width, height, scale);
    center = Viewport.centerKeeping(pixel, pointer, frameWidth, frameHeight, width, height, 1);
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
    {#if frameBox && (line || guides)}
      <div
        class="guides"
        aria-hidden="true"
        style:left="{frameBox.left}px"
        style:top="{frameBox.top}px"
        style:width="{frameBox.width}px"
        style:height="{frameBox.height}px"
      ></div>
    {/if}
    {#if frameBox && hasLayer && oncrop && cropping && !horizon}
      <div
        class="edit-frame"
        style:left="{frameBox.left}px"
        style:top="{frameBox.top}px"
        style:width="{frameBox.width}px"
        style:height="{frameBox.height}px"
      >
        <CropOverlay
          crop={shownCrop}
          width={oriented.width}
          height={oriented.height}
          {lockRatio}
          disabled={!active}
          movable={zoom === "fit"}
          onpreview={(crop) => oncrop(crop, false)}
          oncommit={(crop) => oncrop(crop, true)}
        />
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

    {#if !hasLayer}
      <p class="developing" role="status">Developing {fileName}…</p>
    {:else}
      <div class="badges top-left">
        {#if showBefore || split}
          {#if beforeTag}{@render beforeTag()}{:else}<span class="badge strong">Before</span>{/if}
        {/if}
        <span class="badge mono">{zoomLabel}</span>
        {#if statusLabel}<span class="badge" role="status">{statusLabel}</span>{/if}
        {#if beforeNote}<span class="badge">{beforeNote}</span>{/if}
      </div>
      {#if cropping}
        <div class="badges bottom-center" role="status">
          <span class="badge strong">Crop</span>
          <span class="badge">Drag the handles. Enter or R keeps it, Esc puts it back.</span>
        </div>
      {/if}
      {#if split && !showBefore}
        <div class="badges top-right">
          {#if afterTag}{@render afterTag()}{:else}<span class="badge strong">After</span>{/if}
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

  .edit-frame {
    position: absolute;
    pointer-events: none;
  }

  /* A level grid while straightening, a fixed size on screen at any zoom. */
  .guides {
    position: absolute;
    pointer-events: none;
    background-image:
      linear-gradient(to right, rgba(242, 242, 242, 0.35) 1px, transparent 1px),
      linear-gradient(to bottom, rgba(242, 242, 242, 0.35) 1px, transparent 1px);
    background-size: 48px 48px;
    background-position: center;
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

  .bottom-center {
    bottom: 0.75rem;
    left: 50%;
    transform: translateX(-50%);
    justify-content: center;
    max-width: calc(100% - 1.5rem);
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
