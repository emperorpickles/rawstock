<script lang="ts">
  import type { CropRect } from "./contract";
  import { CropMath } from "./geometry";
  import type { CropHandle } from "./geometry";

  let {
    crop,
    width,
    height,
    lockRatio = null,
    disabled = false,
    movable = true,
    onpreview,
    oncommit,
  }: {
    crop: CropRect;
    /** The framed image's size in pixels, for the aspect lock. */
    width: number;
    height: number;
    lockRatio?: number | null;
    disabled?: boolean;
    /** Dragging inside the crop moves it. Off when zoomed, where it pans. */
    movable?: boolean;
    onpreview: (crop: CropRect) => void;
    oncommit: (crop: CropRect) => void;
  } = $props();

  const HANDLES: ReadonlyArray<{ id: Exclude<CropHandle, "move">; label: string }> = [
    { id: "nw", label: "Crop top-left corner" },
    { id: "n", label: "Crop top edge" },
    { id: "ne", label: "Crop top-right corner" },
    { id: "e", label: "Crop right edge" },
    { id: "se", label: "Crop bottom-right corner" },
    { id: "s", label: "Crop bottom edge" },
    { id: "sw", label: "Crop bottom-left corner" },
    { id: "w", label: "Crop left edge" },
  ];

  const ARROWS: Record<string, [number, number]> = {
    ArrowLeft: [-1, 0],
    ArrowRight: [1, 0],
    ArrowUp: [0, -1],
    ArrowDown: [0, 1],
  };

  let frame = $state<HTMLElement | null>(null);
  let handle: CropHandle | null = null;
  let start = { x: 0, y: 0 };
  let keyed = { x: 0, y: 0 };
  let origin: CropRect | null = null;
  let live: CropRect | null = null;

  /** The pointer in normalised frame units. The frame moves when the view pans. */
  function at(event: PointerEvent): { x: number; y: number } | null {
    const rect = frame?.getBoundingClientRect();
    if (!rect || rect.width <= 0 || rect.height <= 0) {
      return null;
    }
    return { x: (event.clientX - rect.left) / rect.width, y: (event.clientY - rect.top) / rect.height };
  }

  function pointerDown(event: PointerEvent, next: CropHandle) {
    if (disabled || event.button !== 0) {
      return;
    }
    const point = at(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    handle = next;
    origin = { ...crop };
    start = point;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function pointerMove(event: PointerEvent) {
    if (!handle || !origin) {
      return;
    }
    const point = at(event);
    if (!point) {
      return;
    }
    live = CropMath.drag(origin, handle, point.x - start.x, point.y - start.y, lockRatio, width, height);
    onpreview(live);
  }

  function finish() {
    const done = live;
    handle = null;
    origin = null;
    live = null;
    keyed = { x: 0, y: 0 };
    if (done) {
      oncommit(done);
    }
  }

  /** Arrow keys nudge a focused handle a screen pixel, or ten with Shift. */
  function keyDown(event: KeyboardEvent, next: CropHandle) {
    const step = ARROWS[event.key];
    const rect = frame?.getBoundingClientRect();
    if (disabled || !step || !rect || rect.width <= 0 || rect.height <= 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (handle !== next || !origin) {
      handle = next;
      origin = { ...crop };
      keyed = { x: 0, y: 0 };
    }
    const size = event.shiftKey ? 10 : 1;
    keyed = { x: keyed.x + step[0] * size, y: keyed.y + step[1] * size };
    live = CropMath.drag(origin, next, keyed.x / rect.width, keyed.y / rect.height, lockRatio, width, height);
    onpreview(live);
  }

  function keyUp(event: KeyboardEvent) {
    if (ARROWS[event.key] && handle) {
      finish();
    }
  }
</script>

<div class="frame" bind:this={frame}>
  <div
    class="crop"
    class:full={CropMath.isFull(crop)}
    class:movable={movable && !disabled}
    role="button"
    tabindex={disabled ? -1 : 0}
    aria-label="Crop area. Arrow keys move it."
    style:left={`${crop.x * 100}%`}
    style:top={`${crop.y * 100}%`}
    style:width={`${crop.w * 100}%`}
    style:height={`${crop.h * 100}%`}
    onpointerdown={(event) => movable && pointerDown(event, "move")}
    onpointermove={pointerMove}
    onpointerup={finish}
    onpointercancel={finish}
    onkeydown={(event) => keyDown(event, "move")}
    onkeyup={keyUp}
    onblur={() => handle && finish()}
  >
    {#each HANDLES as item (item.id)}
      <span
        class="handle {item.id}"
        class:disabled
        role="button"
        tabindex={disabled ? -1 : 0}
        aria-label="{item.label}. Arrow keys move it."
        onpointerdown={(event) => pointerDown(event, item.id)}
        onpointermove={pointerMove}
        onpointerup={finish}
        onpointercancel={finish}
        onkeydown={(event) => keyDown(event, item.id)}
        onkeyup={keyUp}
        onblur={() => handle && finish()}
      ></span>
    {/each}
  </div>
</div>

<style>
  .frame {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  .crop {
    position: absolute;
    border: 1px solid #f2f2f2;
    box-shadow: 0 0 0 99999px rgba(0, 0, 0, 0.45);
    pointer-events: none;
    touch-action: none;
  }

  /* Rule of thirds inside the crop, as a guide while framing. */
  .crop::before {
    content: "";
    position: absolute;
    inset: 0;
    pointer-events: none;
    background-image:
      linear-gradient(to right, transparent calc(33.333% - 0.5px), rgba(242, 242, 242, 0.4) calc(33.333% - 0.5px), rgba(242, 242, 242, 0.4) calc(33.333% + 0.5px), transparent calc(33.333% + 0.5px), transparent calc(66.667% - 0.5px), rgba(242, 242, 242, 0.4) calc(66.667% - 0.5px), rgba(242, 242, 242, 0.4) calc(66.667% + 0.5px), transparent calc(66.667% + 0.5px)),
      linear-gradient(to bottom, transparent calc(33.333% - 0.5px), rgba(242, 242, 242, 0.4) calc(33.333% - 0.5px), rgba(242, 242, 242, 0.4) calc(33.333% + 0.5px), transparent calc(33.333% + 0.5px), transparent calc(66.667% - 0.5px), rgba(242, 242, 242, 0.4) calc(66.667% - 0.5px), rgba(242, 242, 242, 0.4) calc(66.667% + 0.5px), transparent calc(66.667% + 0.5px));
  }

  .crop.movable {
    pointer-events: auto;
    cursor: move;
  }

  .crop.full {
    box-shadow: none;
    border-color: rgba(242, 242, 242, 0.45);
  }

  .crop:focus-visible,
  .handle:focus-visible {
    outline: 2px solid #f7f7f7;
    outline-offset: 2px;
  }

  /* Each handle is a generous hit strip with a small visible mark. */
  .handle {
    position: absolute;
    pointer-events: auto;
    touch-action: none;
  }

  .handle.disabled {
    pointer-events: none;
  }

  .handle::after {
    content: "";
    position: absolute;
    left: 50%;
    top: 50%;
    width: 9px;
    height: 9px;
    margin: -5px 0 0 -5px;
    background: #f2f2f2;
    border: 1px solid #141414;
  }

  .n, .s { left: 12px; right: 12px; height: 12px; cursor: ns-resize; }
  .e, .w { top: 12px; bottom: 12px; width: 12px; cursor: ew-resize; }
  .n { top: -6px; }
  .s { bottom: -6px; }
  .e { right: -6px; }
  .w { left: -6px; }
  .n::after, .s::after { width: 18px; height: 4px; margin: -3px 0 0 -10px; }
  .e::after, .w::after { width: 4px; height: 18px; margin: -10px 0 0 -3px; }

  .nw, .ne, .sw, .se { width: 18px; height: 18px; }
  .nw { left: -9px; top: -9px; cursor: nwse-resize; }
  .ne { right: -9px; top: -9px; cursor: nesw-resize; }
  .sw { left: -9px; bottom: -9px; cursor: nesw-resize; }
  .se { right: -9px; bottom: -9px; cursor: nwse-resize; }
</style>
