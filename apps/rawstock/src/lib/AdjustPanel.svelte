<script lang="ts">
  import type { AsShotWhiteBalance, EditSettings, StockSummary } from "./contract";
  import { Adjustments } from "./adjust";
  import { CropMath } from "./geometry";
  import type { AspectId } from "./geometry";
  import Slider from "./Slider.svelte";

  let {
    settings,
    stock = null,
    disabled = false,
    frameWidth = 0,
    frameHeight = 0,
    horizon = false,
    cropping = false,
    asShot = undefined,
    onpreview,
    oncommit,
    onhorizon,
    oncropping,
  }: {
    settings: EditSettings;
    stock?: StockSummary | null;
    disabled?: boolean;
    frameWidth?: number;
    frameHeight?: number;
    horizon?: boolean;
    /** The viewer is in crop mode, with the crop handles. */
    cropping?: boolean;
    /** The file's as-shot light. `undefined` while it is read, `null` when the file cannot take a custom balance. */
    asShot?: AsShotWhiteBalance | null;
    onpreview: (settings: EditSettings) => void;
    oncommit: (settings: EditSettings) => void;
    onhorizon: (on: boolean) => void;
    oncropping: (on: boolean) => void;
  } = $props();

  const frame = $derived(Adjustments.orientedFrame(frameWidth, frameHeight, settings.geometry.quarterTurns));
  const aspect = $derived(CropMath.infer(settings.geometry.crop, frame.width, frame.height));
  const dye = $derived(settings.dye?.scales ?? [1, 1, 1]);
  const showDye = $derived(Adjustments.showDye(stock, settings));
  const showContrast = $derived(Adjustments.showContrast(stock, settings));
  const showGrain = $derived(Adjustments.showGrain(stock, settings));
  const thresholdDefault = $derived(Adjustments.thresholdDefault(stock));
  const radiusDefault = $derived(Adjustments.radiusDefault(stock));
  const thresholdScale = $derived({ ...Adjustments.threshold, defaultValue: thresholdDefault });
  const radiusScale = $derived({ ...Adjustments.radius, defaultValue: radiusDefault });
  const showHalationAmounts = $derived(
    settings.halation.enabled ||
      settings.halation.threshold != null ||
      settings.halation.radius != null,
  );
  const looks = $derived(Adjustments.scanOptions(settings.scan.look));
  const dyeNames = ["Red dye", "Green dye", "Blue dye"] as const;
  const customWb = $derived(settings.whiteBalance.mode === "custom");
  const wbValues = $derived(asShot ? Adjustments.whiteBalanceValues(settings, asShot) : null);
  const temperatureScale = $derived(
    asShot
      ? { ...Adjustments.temperatureScale, defaultValue: asShot.temperatureK }
      : Adjustments.temperatureScale,
  );
  const tintScale = $derived(
    asShot ? { ...Adjustments.tintScale, defaultValue: asShot.tint } : Adjustments.tintScale,
  );

  function send(next: EditSettings, commit: boolean) {
    if (commit) {
      oncommit(next);
    } else {
      onpreview(next);
    }
  }

  /** A new aspect opens crop mode, so its handles are there to place it. */
  function chooseAspect(id: AspectId) {
    oncommit(Adjustments.aspect(settings, id, frame.width, frame.height));
    oncropping(true);
  }
</script>

<section class="adjust" aria-label="Adjust">
  <h2>Adjust</h2>

  <Slider
    label="Exposure"
    value={settings.exposureEv}
    scale={Adjustments.exposureScale}
    {disabled}
    onpreview={(exposureEv) => send(Adjustments.exposure(settings, exposureEv), false)}
    oncommit={(exposureEv) => send(Adjustments.exposure(settings, exposureEv), true)}
  />

  <div class="group" role="group" aria-labelledby="wb-label">
    <div class="heading">
      <p class="name" id="wb-label">White balance</p>
      {#if asShot}
        <button
          type="button"
          disabled={disabled || !customWb}
          aria-pressed={!customWb}
          onclick={() => send(Adjustments.asShotWhiteBalance(settings), true)}
        >
          As shot
        </button>
      {/if}
    </div>
    {#if asShot && wbValues}
      <Slider
        label="Temperature"
        value={wbValues.temperatureK}
        scale={temperatureScale}
        {disabled}
        onpreview={(kelvin) => send(Adjustments.temperature(settings, asShot, kelvin), false)}
        oncommit={(kelvin) => send(Adjustments.temperature(settings, asShot, kelvin), true)}
      />
      <Slider
        label="Tint"
        value={wbValues.tint}
        scale={tintScale}
        {disabled}
        onpreview={(tint) => send(Adjustments.tint(settings, asShot, tint), false)}
        oncommit={(tint) => send(Adjustments.tint(settings, asShot, tint), true)}
      />
    {:else if asShot === null}
      <p class="note">
        This file cannot take a custom white balance, so it develops as shot.
      </p>
    {:else if settings.whiteBalance.mode === "custom"}
      <p class="readout">
        {settings.whiteBalance.temperatureK.toFixed(0)} K, tint {settings.whiteBalance.tint.toFixed(0)}
      </p>
    {:else}
      <p class="readout">As shot</p>
    {/if}
  </div>

  <Slider
    label="Print exposure"
    value={settings.printExposure}
    scale={Adjustments.print}
    {disabled}
    onpreview={(printExposure) => send(Adjustments.printExposure(settings, printExposure), false)}
    oncommit={(printExposure) => send(Adjustments.printExposure(settings, printExposure), true)}
  />

  {#if showDye}
    {#each dyeNames as name, index (name)}
      <Slider
        label={name}
        value={dye[index] ?? 1}
        scale={Adjustments.dye}
        {disabled}
        onpreview={(next) => send(Adjustments.dyeChannel(settings, index as 0 | 1 | 2, next), false)}
        oncommit={(next) => send(Adjustments.dyeChannel(settings, index as 0 | 1 | 2, next), true)}
      />
    {/each}
  {/if}

  {#if showContrast}
    <Slider
      label="Contrast"
      value={settings.contrast ?? 1}
      scale={Adjustments.contrastScale}
      {disabled}
      onpreview={(next) => send(Adjustments.contrast(settings, next), false)}
      oncommit={(next) => send(Adjustments.contrast(settings, next), true)}
    />
  {/if}

  {#if showGrain}
    <Slider
      label="Grain amount"
      value={settings.grain.amount}
      scale={Adjustments.grainAmountScale}
      {disabled}
      onpreview={(amount) => send(Adjustments.grainAmount(settings, amount), false)}
      oncommit={(amount) => send(Adjustments.grainAmount(settings, amount), true)}
    />
    <Slider
      label="Grain size"
      value={settings.grain.size}
      scale={Adjustments.grainSizeScale}
      {disabled}
      onpreview={(size) => send(Adjustments.grainSize(settings, size), false)}
      oncommit={(size) => send(Adjustments.grainSize(settings, size), true)}
    />
  {/if}

  <label class="check">
    <input
      type="checkbox"
      checked={settings.halation.enabled}
      {disabled}
      onchange={(event) =>
        send(Adjustments.halationEnabled(settings, event.currentTarget.checked), true)}
    />
    Halation
  </label>
  {#if showHalationAmounts}
    <Slider
      label="Threshold"
      value={settings.halation.threshold ?? thresholdDefault}
      scale={thresholdScale}
      {disabled}
      onpreview={(next) => send(Adjustments.halationThreshold(settings, next, thresholdDefault), false)}
      oncommit={(next) => send(Adjustments.halationThreshold(settings, next, thresholdDefault), true)}
    />
    <Slider
      label="Radius"
      value={settings.halation.radius ?? radiusDefault}
      scale={radiusScale}
      {disabled}
      onpreview={(next) => send(Adjustments.halationRadius(settings, next, radiusDefault), false)}
      oncommit={(next) => send(Adjustments.halationRadius(settings, next, radiusDefault), true)}
    />
  {/if}

  <label class="field">
    Scan / print look
    <select
      value={settings.scan.look}
      {disabled}
      onchange={(event) => send(Adjustments.scan(settings, event.currentTarget.value), true)}
    >
      {#each looks as look (look.id)}
        <option value={look.id}>{look.label}</option>
      {/each}
    </select>
  </label>

  <div class="group">
    <p class="name" id="geometry-label">Crop, straighten, rotate</p>
    <div class="row" role="group" aria-labelledby="geometry-label">
      <button
        type="button"
        {disabled}
        aria-label="Rotate left"
        onclick={() => send(Adjustments.rotate(settings, -1), true)}
      >
        Rotate left
      </button>
      <button
        type="button"
        {disabled}
        aria-label="Rotate right"
        onclick={() => send(Adjustments.rotate(settings, 1), true)}
      >
        Rotate right
      </button>
    </div>
    <Slider
      label="Straighten"
      value={settings.geometry.straightenDeg}
      scale={Adjustments.straightenScale}
      {disabled}
      onpreview={(degrees) => send(Adjustments.straighten(settings, degrees), false)}
      oncommit={(degrees) => send(Adjustments.straighten(settings, degrees), true)}
    />
    <button
      type="button"
      aria-pressed={horizon}
      {disabled}
      onclick={() => onhorizon(!horizon)}
    >
      {horizon ? "Drawing a horizon" : "Draw a horizon"}
    </button>
    <button
      type="button"
      aria-pressed={cropping}
      aria-keyshortcuts="R"
      {disabled}
      onclick={() => oncropping(!cropping)}
    >
      {cropping ? "Done cropping" : "Crop"}
    </button>
    <label class="field">
      Aspect
      <select
        value={aspect}
        {disabled}
        onchange={(event) => chooseAspect(event.currentTarget.value as AspectId)}
      >
        {#each CropMath.aspects() as id (id)}
          <option value={id}>{CropMath.label(id)}</option>
        {/each}
      </select>
    </label>
  </div>
</section>

<style>
  .adjust {
    padding: 0.7rem 0.75rem 1rem;
    overflow: auto;
    min-height: 0;
  }

  h2 {
    margin: 0 0 0.65rem;
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .group {
    margin: 0 0 0.65rem;
  }

  .heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.45rem;
  }

  .name,
  .note,
  .readout {
    margin: 0.1rem 0 0.3rem;
  }

  .name {
    font-size: 0.85rem;
  }

  .note,
  .readout {
    color: var(--muted);
    font-size: 0.78rem;
  }

  .readout {
    font-family: var(--font-mono);
    color: var(--accent);
  }

  .check,
  .field {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    margin: 0.35rem 0 0.55rem;
    font-size: 0.85rem;
  }

  .field {
    flex-direction: column;
    align-items: stretch;
    gap: 0.25rem;
  }

  select {
    background: var(--app);
    color: var(--ink);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 0.25rem 0.35rem;
  }

  .row {
    display: flex;
    gap: 0.35rem;
    margin-bottom: 0.45rem;
  }

  .row button,
  button[aria-pressed] {
    border-radius: 4px;
    font-size: 0.78rem;
  }

  button[aria-pressed="true"] {
    background: var(--accent);
    color: var(--on-accent);
    border-color: var(--accent);
  }
</style>
