<script lang="ts">
  import type { StockSummary } from "./contract";
  import { exposureText, quantityText } from "./shortcuts";

  let {
    stocks,
    stockId = "",
    onstock,
  }: {
    stocks: StockSummary[];
    stockId?: string;
    onstock: (stock: StockSummary) => void;
  } = $props();

  let exposure = $state(0);
  let printExposure = $state(0);
  let grain = $state(1);

  const categoryLabel: Record<StockSummary["category"], string> = {
    colorNegative: "Color negative",
    slide: "Slide",
    bw: "Black and white",
    fujiSim: "Fuji simulation",
  };
</script>

<aside class="side" aria-label="Stock and adjustments">
  <section class="stocks" aria-label="Stocks">
    <h2>Stocks</h2>
    {#if stocks.length === 0}
      <p class="hint">Stocks appear here when the catalog can be read.</p>
    {:else}
      <ul class="list">
        {#each stocks as stock (stock.id)}
          <li>
            <button
              type="button"
              aria-pressed={stock.id === stockId}
              onclick={() => onstock(stock)}
            >
              <span class="name">{stock.name}</span>
              <span class="meta">{categoryLabel[stock.category]} · rev {stock.revision}</span>
            </button>
          </li>
        {/each}
      </ul>
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
      bind:value={exposure}
      aria-valuetext={exposureText(exposure)}
    />
    <p class="readout">{exposureText(exposure)}</p>

    <p class="field" id="wb-label">White balance</p>
    <p class="readout" aria-labelledby="wb-label">As shot</p>

    <label class="field" for="print">Print exposure</label>
    <input
      id="print"
      type="range"
      min="0"
      max="2.5"
      step="0.01"
      bind:value={printExposure}
      aria-valuetext={quantityText(printExposure, "print")}
    />
    <p class="readout">{printExposure.toFixed(2)}</p>

    <label class="field" for="grain">Grain amount</label>
    <input
      id="grain"
      type="range"
      min="0"
      max="2"
      step="0.01"
      bind:value={grain}
      aria-valuetext={quantityText(grain, "times the stock grain")}
    />
    <p class="readout">{grain.toFixed(2)}×</p>
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
  .meta,
  .readout {
    color: #b8b8b8;
    font-size: 0.85rem;
  }

  .hint,
  .readout,
  .field {
    margin: 0.15rem 0 0.45rem;
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .list button {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.1rem;
    text-align: left;
    border-radius: 6px;
    padding: 0.4rem 0.5rem;
  }

  .list button[aria-pressed="true"] {
    background: #2a2a2a;
    border-color: #6a6a6a;
  }

  .name {
    color: #f2f2f2;
  }

  input[type="range"] {
    width: 100%;
    accent-color: #d0d0d0;
  }
</style>
