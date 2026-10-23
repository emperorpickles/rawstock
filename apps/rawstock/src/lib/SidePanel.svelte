<script lang="ts">
  import type { EditSettings, StockSummary } from "./contract";
  import type { PackedFrame } from "./frame";
  import { exposureText, quantityText } from "./shortcuts";
  import StockBrowser from "./StockBrowser.svelte";

  let {
    stocks,
    stockId = "",
    thumbnails = {},
    settings,
    disabled = false,
    onstock,
    onpreview,
    oncommit,
  }: {
    stocks: StockSummary[];
    stockId?: string;
    thumbnails?: Record<string, PackedFrame>;
    settings: EditSettings;
    disabled?: boolean;
    onstock: (stock: StockSummary) => void;
    onpreview: (settings: EditSettings) => void;
    oncommit: (settings: EditSettings) => void;
  } = $props();

  function patch(next: EditSettings, commit: boolean) {
    if (commit) {
      oncommit(next);
    } else {
      onpreview(next);
    }
  }
</script>

<aside class="side" aria-label="Stock and adjustments">
  <section class="stocks" aria-label="Stocks">
    <h2>Stocks</h2>
    {#if stocks.length === 0}
      <p class="hint">Stocks appear here when the catalog can be read.</p>
    {:else}
      <StockBrowser {stocks} {stockId} {thumbnails} {disabled} {onstock} />
    {/if}
  </section>

  <section class="adjust" aria-label="Adjust">
    <h2>Adjust</h2>
    <label class="field" for="exposure">Exposure</label>
    <input
      id="exposure"
      type="range"
      min="-3"
      max="3"
      step="0.01"
      {disabled}
      value={settings.exposureEv}
      aria-valuetext={exposureText(settings.exposureEv)}
      oninput={(event) =>
        patch({ ...settings, exposureEv: Number(event.currentTarget.value) }, false)}
      onchange={(event) =>
        patch({ ...settings, exposureEv: Number(event.currentTarget.value) }, true)}
    />
    <p class="readout">{exposureText(settings.exposureEv)}</p>

    <p class="field" id="wb-label">White balance</p>
    {#if settings.whiteBalance.mode === "custom"}
      <p class="readout" aria-labelledby="wb-label">
        Custom white balance is saved. This build develops as shot.
      </p>
    {:else}
      <p class="readout" aria-labelledby="wb-label">As shot</p>
    {/if}

    <label class="field" for="print">Print exposure</label>
    <input
      id="print"
      type="range"
      min="0"
      max="2.5"
      step="0.01"
      {disabled}
      value={settings.printExposure}
      aria-valuetext={quantityText(settings.printExposure, "print")}
      oninput={(event) =>
        patch({ ...settings, printExposure: Number(event.currentTarget.value) }, false)}
      onchange={(event) =>
        patch({ ...settings, printExposure: Number(event.currentTarget.value) }, true)}
    />
    <p class="readout">{settings.printExposure.toFixed(2)}</p>

    <label class="field" for="grain">Grain amount</label>
    <input
      id="grain"
      type="range"
      min="0"
      max="2"
      step="0.01"
      {disabled}
      value={settings.grain.amount}
      aria-valuetext={quantityText(settings.grain.amount, "times the stock grain")}
      oninput={(event) =>
        patch(
          { ...settings, grain: { ...settings.grain, amount: Number(event.currentTarget.value) } },
          false,
        )}
      onchange={(event) =>
        patch(
          { ...settings, grain: { ...settings.grain, amount: Number(event.currentTarget.value) } },
          true,
        )}
    />
    <p class="readout">{settings.grain.amount.toFixed(2)}×</p>
  </section>
</aside>

<style>
  .side {
    background: #1c1c1c;
    border-left: 1px solid #3a3a3a;
    overflow: auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  section {
    padding: 0.85rem 0.9rem 1rem;
  }

  section + section {
    border-top: 1px solid #3a3a3a;
  }

  h2 {
    margin: 0 0 0.7rem;
    font-size: 0.78rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: #b8b8b8;
  }

  .hint,
  .readout {
    color: #b8b8b8;
    font-size: 0.85rem;
  }

  .hint,
  .readout,
  .field {
    margin: 0.15rem 0 0.45rem;
  }

  input[type="range"] {
    width: 100%;
    accent-color: #d0d0d0;
  }
</style>
