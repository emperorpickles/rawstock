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
    onpreview,
    oncommit,
  }: {
    crop: CropRect;
    width: number;
    height: number;
    lockRatio?: number | null;
    disabled?: boolean;
    onpreview: (crop: CropRect) => void;
    oncommit: (crop: CropRect) => void;
  } = $props();

  let box = $state<HTMLElement | null>(null);
  let handle: CropHandle | null = null;
  let originX = 0;
  let originY = 0;
  let origin: CropRect | null = null;
  let live: CropRect | null = null;

  function hit(event: PointerEvent): CropHandle | null {
    if (!box) {
      return null;
    }
    const rect = box.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return null;
    }
    const x = (event.clientX - rect.left) / rect.width;
    const y = (event.clientY - rect.top) / rect.height;
    const padX = 12 / rect.width;
    const padY = 12 / rect.height;
    const nearL = Math.abs(x - crop.x) <= padX;
    const nearR = Math.abs(x - (crop.x + crop.w)) <= padX;
    const nearT = Math.abs(y - crop.y) <= padY;
    const nearB = Math.abs(y - (crop.y + crop.h)) <= padY;
    if (nearT && nearL) return "nw";
    if (nearT && nearR) return "ne";
    if (nearB && nearL) return "sw";
    if (nearB && nearR) return "se";
    if (nearT && x >= crop.x && x <= crop.x + crop.w) return "n";
    if (nearB && x >= crop.x && x <= crop.x + crop.w) return "s";
    if (nearL && y >= crop.y && y <= crop.y + crop.h) return "w";
    if (nearR && y >= crop.y && y <= crop.y + crop.h) return "e";
    if (x >= crop.x && x <= crop.x + crop.w && y >= crop.y && y <= crop.y + crop.h) return "move";
    return null;
  }

  function pointerDown(event: PointerEvent) {
    if (disabled || event.button !== 0) {
      return;
    }
    const next = hit(event);
    if (!next) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    handle = next;
    origin = { ...crop };
    originX = event.clientX;
    originY = event.clientY;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function pointerMove(event: PointerEvent) {
    if (!handle || !origin || !box) {
      return;
    }
    const rect = box.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return;
    }
    const dx = (event.clientX - originX) / rect.width;
    const dy = (event.clientY - originY) / rect.height;
    live = CropMath.drag(origin, handle, dx, dy, lockRatio, width, height);
    onpreview(live);
  }

  function pointerUp() {
    const done = live;
    handle = null;
    origin = null;
    live = null;
    if (done) {
      oncommit(done);
    }
  }
</script>

<div
  role="group"
  aria-label="Crop"
  class="crop"
  class:full={crop.w >= 0.999 && crop.h >= 0.999 && crop.x <= 0.001 && crop.y <= 0.001}
  class:disabled
  bind:this={box}
  style:left={`${crop.x * 100}%`}
  style:top={`${crop.y * 100}%`}
  style:width={`${crop.w * 100}%`}
  style:height={`${crop.h * 100}%`}
  onpointerdown={pointerDown}
  onpointermove={pointerMove}
  onpointerup={pointerUp}
  onpointercancel={pointerUp}
>
  <span class="handle nw"></span>
  <span class="handle ne"></span>
  <span class="handle sw"></span>
  <span class="handle se"></span>
</div>

<style>
  .crop {
    position: absolute;
    border: 1px solid #f2f2f2;
    box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.45);
    cursor: move;
    touch-action: none;
  }

  .crop.full {
    box-shadow: none;
    border-color: rgba(242, 242, 242, 0.45);
  }

  .crop.disabled {
    pointer-events: none;
  }

  .handle {
    position: absolute;
    width: 9px;
    height: 9px;
    background: #f2f2f2;
    border: 1px solid #141414;
  }

  .nw { left: -5px; top: -5px; cursor: nwse-resize; }
  .ne { right: -5px; top: -5px; cursor: nesw-resize; }
  .sw { left: -5px; bottom: -5px; cursor: nesw-resize; }
  .se { right: -5px; bottom: -5px; cursor: nwse-resize; }
</style>
