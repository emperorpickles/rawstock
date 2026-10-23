<script lang="ts">
  import AdjustPanel from "./AdjustPanel.svelte";
  import type { EditSettings, StockSummary } from "./contract";

  let {
    stocks,
    stockId = "",
    settings,
    disabled = false,
    frameWidth = 0,
    frameHeight = 0,
    horizon = false,
    onstock,
    onpreview,
    oncommit,
    onhorizon,
  }: {
    stocks: StockSummary[];
    stockId?: string;
    settings: EditSettings;
    disabled?: boolean;
    frameWidth?: number;
    frameHeight?: number;
    horizon?: boolean;
    onstock: (stock: StockSummary) => void;
    onpreview: (settings: EditSettings) => void;
    oncommit: (settings: EditSettings) => void;
    onhorizon: (on: boolean) => void;
  } = $props();

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
              {disabled}
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

  <AdjustPanel
    {settings}
    stock={stocks.find((item) => item.id === stockId) ?? null}
    {disabled}
    {frameWidth}
    {frameHeight}
    {horizon}
    {onpreview}
    {oncommit}
    {onhorizon}
  />
</aside>

<style>
  .side {
    background: var(--panel);
    border-left: 1px solid var(--line);
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  section {
    padding: 0.7rem 0.75rem 0.8rem;
  }

  .stocks {
    max-height: 32%;
    overflow: auto;
    flex: none;
  }

  h2 {
    margin: 0 0 0.55rem;
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .hint,
  .meta {
    color: var(--muted);
    font-size: 0.78rem;
  }

  .hint {
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
    background: var(--paper);
    border-color: var(--accent);
    color: var(--ink);
  }

  .name {
    color: var(--ink);
  }
</style>
