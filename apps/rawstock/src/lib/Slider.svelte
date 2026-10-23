<script lang="ts">
  import { SliderGesture, SliderMath } from "./slider";
  import type { SliderScale } from "./slider";

  let {
    label,
    value,
    scale,
    disabled = false,
    onpreview,
    oncommit,
  }: {
    label: string;
    value: number;
    scale: SliderScale;
    disabled?: boolean;
    onpreview: (value: number) => void;
    oncommit: (value: number) => void;
  } = $props();

  let track = $state<HTMLElement | null>(null);
  let editing = $state(false);
  let draft = $state("");
  let field = $state<HTMLInputElement | null>(null);
  let gesture: SliderGesture | null = null;

  const bounds = $derived(SliderMath.bounds(scale, value));
  const text = $derived(SliderMath.format(value, scale));
  const spoken = $derived(SliderMath.ariaText(value, scale));
  const fill = $derived(SliderMath.fill(value, scale));
  const majors = $derived(SliderMath.ticks(bounds.min, bounds.max, scale.majorEvery));
  const minors = $derived(
    SliderMath.ticks(bounds.min, bounds.max, scale.minorEvery).filter(
      (tick) => !majors.some((major) => Math.abs(major - tick) <= scale.fine / 2),
    ),
  );
  const labelId = $derived(`slider-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`);

  $effect(() => {
    if (editing) {
      field?.focus();
      field?.select();
    }
  });

  function fraction(tick: number): string {
    const span = bounds.max - bounds.min;
    if (span <= 0) {
      return "0%";
    }
    return `${((tick - bounds.min) / span) * 100}%`;
  }

  function pointerDown(event: PointerEvent) {
    if (disabled || editing || event.button !== 0 || !track) {
      return;
    }
    gesture = new SliderGesture(value);
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    const next = gesture.preview(
      SliderMath.fromPointer(event.clientX, track.getBoundingClientRect(), scale, value),
    );
    onpreview(next);
  }

  function pointerMove(event: PointerEvent) {
    if (!gesture || !track || disabled) {
      return;
    }
    const next = gesture.preview(
      SliderMath.fromPointer(event.clientX, track.getBoundingClientRect(), scale, gesture.latest()),
    );
    onpreview(next);
  }

  function pointerUp() {
    if (!gesture) {
      return;
    }
    const done = gesture.end();
    gesture = null;
    if (done != null) {
      oncommit(done);
    }
  }

  function reset() {
    if (disabled) {
      return;
    }
    gesture = null;
    oncommit(SliderMath.reset(scale));
  }

  function onKeyDown(event: KeyboardEvent) {
    if (disabled) {
      return;
    }
    const direction =
      event.key === "ArrowRight" || event.key === "ArrowUp"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowDown"
          ? -1
          : 0;
    if (direction === 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (!gesture) {
      gesture = new SliderGesture(value);
    }
    const next = gesture.preview(
      SliderMath.nudge(gesture.latest(), scale, direction, {
        shift: event.shiftKey,
        alt: event.altKey,
      }),
    );
    onpreview(next);
  }

  function finishKeys(event: KeyboardEvent) {
    if (!event.key.startsWith("Arrow")) {
      return;
    }
    event.stopPropagation();
    endKeys();
  }

  function endKeys() {
    if (!gesture) {
      return;
    }
    const done = gesture.end();
    gesture = null;
    if (done != null) {
      oncommit(done);
    }
  }

  function startType() {
    if (disabled) {
      return;
    }
    draft = SliderMath.formatNumber(value, scale);
    editing = true;
  }

  function commitType() {
    const parsed = SliderMath.parse(draft);
    editing = false;
    if (parsed == null) {
      return;
    }
    oncommit(parsed);
  }

  function onTypeKey(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      commitType();
    }
    if (event.key === "Escape") {
      event.preventDefault();
      editing = false;
    }
  }
</script>

<div class="slider" class:disabled>
  <div class="head">
    <span id={labelId} class="name">{label}</span>
    {#if editing}
      <input
        bind:this={field}
        class="readout"
        type="text"
        inputmode="decimal"
        aria-label={`${label} value`}
        bind:value={draft}
        onkeydown={onTypeKey}
        onblur={commitType}
      />
    {:else}
      <button type="button" class="readout" {disabled} onclick={startType}>
        {text}
      </button>
    {/if}
    <button type="button" class="reset" aria-label={`Reset ${label}`} {disabled} onclick={reset}>
      Reset
    </button>
  </div>
  <div
    class="control"
    bind:this={track}
    role="slider"
    tabindex={disabled ? -1 : 0}
    aria-labelledby={labelId}
    aria-disabled={disabled}
    aria-valuemin={bounds.min}
    aria-valuemax={bounds.max}
    aria-valuenow={value}
    aria-valuetext={spoken}
    onpointerdown={pointerDown}
    onpointermove={pointerMove}
    onpointerup={pointerUp}
    onpointercancel={pointerUp}
    ondblclick={reset}
    onkeydown={onKeyDown}
    onkeyup={finishKeys}
    onblur={endKeys}
  >
    <div class="rail">
      <div
        class="fill"
        style:left={`${fill.start * 100}%`}
        style:width={`${(fill.end - fill.start) * 100}%`}
      ></div>
      <div class="thumb" style:left={fraction(value)}></div>
    </div>
    <div class="ticks" aria-hidden="true">
      {#each minors as tick (`m${tick}`)}
        <span class="tick minor" style:left={fraction(tick)}></span>
      {/each}
      {#each majors as tick (`M${tick}`)}
        <span class="tick major" style:left={fraction(tick)}></span>
      {/each}
    </div>
  </div>
</div>

<style>
  .slider {
    display: grid;
    gap: 0.2rem;
    margin: 0 0 0.55rem;
  }

  .slider.disabled {
    opacity: 0.45;
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
  }

  .name {
    font-size: 0.85rem;
  }

  .readout {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 0.78rem;
    font-variant-numeric: tabular-nums;
    color: var(--accent);
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    padding: 0 0.25rem;
    min-width: 4.5rem;
    text-align: right;
  }

  button.readout {
    cursor: text;
  }

  input.readout {
    color: var(--ink);
    border-color: var(--accent);
    background: var(--app);
  }

  .reset {
    border-radius: 4px;
    padding: 0 0.35rem;
    font-size: 0.72rem;
    color: var(--muted);
  }

  .control {
    position: relative;
    height: 1.55rem;
    touch-action: none;
    cursor: pointer;
  }

  .control:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .rail {
    position: absolute;
    left: 0;
    right: 0;
    top: 0.28rem;
    height: 2px;
    background: var(--line);
  }

  .fill {
    position: absolute;
    top: 0;
    height: 100%;
    background: var(--accent);
  }

  .thumb {
    position: absolute;
    top: 50%;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--accent);
    border: 2px solid var(--on-accent);
    transform: translate(-50%, -50%);
  }

  .ticks {
    position: absolute;
    left: 0;
    right: 0;
    top: 0.7rem;
    height: 0.7rem;
  }

  .tick {
    position: absolute;
    top: 0;
    width: 1px;
    transform: translateX(-50%);
  }

  .tick.major {
    height: 0.55rem;
    background: var(--tick-major);
  }

  .tick.minor {
    height: 0.32rem;
    background: var(--tick-minor);
  }
</style>
