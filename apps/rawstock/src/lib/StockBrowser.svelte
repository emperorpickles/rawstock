<script lang="ts">
  import { tick } from "svelte";

  import type { StockSummary } from "./contract";
  import type { PackedFrame } from "./frame";
  import { StockBrowser } from "./stockBrowser";

  let {
    stocks,
    stockId = "",
    thumbnails,
    disabled = false,
    onstock,
  }: {
    stocks: StockSummary[];
    stockId?: string;
    thumbnails: Record<string, PackedFrame>;
    disabled?: boolean;
    onstock: (stock: StockSummary) => void;
  } = $props();

  const groups = $derived(StockBrowser.groups(stocks));
  const flat = $derived(StockBrowser.flatten(groups));
  const selectedIndex = $derived(flat.findIndex((stock) => stock.id === stockId));
  /** Roving tab stop. Until the grid is used it sits on the selected stock. */
  let focusIndex = $state(-1);
  let hovered = $state<StockSummary | null>(null);
  let grid = $state<HTMLElement | null>(null);

  const tabIndex = $derived(
    Math.min(Math.max(focusIndex >= 0 ? focusIndex : selectedIndex, 0), Math.max(flat.length - 1, 0)),
  );
  const readout = $derived(hovered ?? flat[selectedIndex] ?? null);

  function indexOf(stock: StockSummary): number {
    return flat.findIndex((item) => item.id === stock.id);
  }

  async function onkeydown(event: KeyboardEvent, stock: StockSummary) {
    const next = StockBrowser.move(groups, indexOf(stock), event.key);
    if (next === null) {
      return;
    }
    event.preventDefault();
    focusIndex = next;
    await tick();
    grid?.querySelector<HTMLButtonElement>(`[data-index="${next}"]`)?.focus();
  }

  function paint(canvas: HTMLCanvasElement, frame: PackedFrame) {
    const draw = (next: PackedFrame) => {
      canvas.width = next.width;
      canvas.height = next.height;
      canvas.getContext("2d")?.putImageData(new ImageData(next.rgba, next.width, next.height), 0, 0);
    };
    draw(frame);
    return { update: draw };
  }
</script>

<div class="browser" bind:this={grid}>
  {#each groups as group (group.category)}
    <h3 id="stock-group-{group.category}">{group.label}</h3>
    <ul class="grid" aria-labelledby="stock-group-{group.category}">
      {#each group.stocks as stock (stock.id)}
        {@const index = indexOf(stock)}
        {@const frame = thumbnails[stock.id]}
        <li>
          <button
            type="button"
            class="item"
            data-index={index}
            tabindex={index === tabIndex ? 0 : -1}
            aria-pressed={stock.id === stockId}
            aria-label={StockBrowser.label(stock)}
            title={StockBrowser.detail(stock)}
            {disabled}
            onclick={() => {
              focusIndex = index;
              onstock(stock);
            }}
            onkeydown={(event) => void onkeydown(event, stock)}
            onfocus={() => {
              focusIndex = index;
              hovered = stock;
            }}
            onblur={() => (hovered = null)}
            onpointerenter={() => (hovered = stock)}
            onpointerleave={() => (hovered = null)}
          >
            <span class="mat">
              {#if frame}
                <canvas use:paint={frame} aria-hidden="true"></canvas>
              {:else}
                <span class="placeholder" aria-hidden="true">{stock.name}</span>
              {/if}
            </span>
            <span class="name" aria-hidden="true">{stock.name}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/each}
  <p class="readout" aria-hidden="true">
    {#if readout}
      {readout.name} · {readout.model} · <span class="number">rev {readout.revision}</span>
    {/if}
  </p>
  <p class="hint">Thumbnails show tone and color. Judge grain at 100%.</p>
</div>

<style>
  .browser {
    display: flex;
    flex-direction: column;
  }

  h3 {
    margin: 0.55rem 0 0.35rem;
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--muted, #b3a594);
  }

  h3:first-child {
    margin-top: 0;
  }

  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.45rem;
  }

  .item {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.25rem;
    border-radius: 6px;
    border: 1px solid transparent;
    text-align: left;
  }

  .item:hover:not(:disabled) {
    border-color: var(--line, #3a332c);
  }

  .item[aria-pressed="true"] {
    background: var(--paper, #231f1b);
    border-color: var(--accent, #d7a15a);
  }

  /* The mat around a thumbnail is never themed. */
  .mat {
    display: grid;
    place-items: center;
    aspect-ratio: 3 / 2;
    padding: 2px;
    background: #2e2e2e;
    border-radius: 3px;
    overflow: hidden;
  }

  canvas {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .placeholder {
    padding: 0 0.3rem;
    font-size: 0.75rem;
    color: #a0a0a0;
    text-align: center;
  }

  .name {
    font-size: 0.8rem;
    color: var(--ink, #f3eadc);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .readout,
  .hint {
    margin: 0.5rem 0 0;
    font-size: 0.75rem;
    color: var(--muted, #b3a594);
  }

  .readout {
    min-height: 1.1em;
  }

  .hint {
    margin-top: 0.15rem;
  }

  .number {
    font-family: "JetBrains Mono", ui-monospace, monospace;
  }
</style>
