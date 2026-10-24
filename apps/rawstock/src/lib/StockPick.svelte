<script lang="ts">
  import type { StockSummary } from "./contract";
  import { StockBrowser } from "./stockBrowser";
  import { StockCompare } from "./stockCompare";

  let {
    letter,
    label,
    stocks,
    value,
    allowOpened = false,
    disabled = false,
    onchange,
    onswap,
  }: {
    /** "A" or "B", drawn before the menu. */
    letter: string;
    /** Accessible name for the menu. */
    label: string;
    stocks: StockSummary[];
    /** Stock id, or `null` for "as opened" when `allowOpened`. */
    value: string | null;
    allowOpened?: boolean;
    disabled?: boolean;
    onchange: (id: string | null) => void;
    /** Shows a swap button that trades the A and B stocks. Absent hides it. */
    onswap?: () => void;
  } = $props();

  const groups = $derived(StockBrowser.groups(stocks));
</script>

<span class="pick">
<label>
  <span class="letter" aria-hidden="true">{letter}</span>
  <select
    aria-label={label}
    value={value ?? ""}
    {disabled}
    onchange={(event) => onchange(event.currentTarget.value === "" ? null : event.currentTarget.value)}
  >
    {#if allowOpened}
      <option value="">{StockCompare.openedLabel}</option>
    {/if}
    {#each groups as group (group.category)}
      <optgroup label={group.label}>
        {#each group.stocks as stock (stock.id)}
          <option value={stock.id}>{stock.name}</option>
        {/each}
      </optgroup>
    {/each}
  </select>
</label>
{#if onswap}
  <button
    type="button"
    class="swap"
    aria-label="Swap the A and B stocks"
    title="Swap A and B"
    disabled={disabled || value === null}
    onclick={onswap}>⇄</button
  >
{/if}
</span>

<style>
  /* Sits on the photo like the viewer's badges, so it is never themed. */
  .pick {
    pointer-events: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.1rem 0.25rem 0.1rem 0.5rem;
    border-radius: 4px;
    background: rgba(20, 20, 20, 0.78);
    color: #f2f2f2;
    font-size: 0.8rem;
  }

  label {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
  }

  .swap {
    font: inherit;
    color: inherit;
    background: transparent;
    border: 1px solid rgba(242, 242, 242, 0.25);
    border-radius: 3px;
    padding: 0 0.4rem;
    line-height: 1.4;
    cursor: pointer;
  }

  .swap:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .swap:focus-visible {
    outline: 2px solid #f2f2f2;
    outline-offset: 1px;
  }

  .letter {
    font-weight: 700;
  }

  select {
    font: inherit;
    color: inherit;
    background: transparent;
    border: 1px solid rgba(242, 242, 242, 0.25);
    border-radius: 3px;
    padding: 0.05rem 0.25rem;
    max-width: 12rem;
  }

  select:focus-visible {
    outline: 2px solid #f2f2f2;
    outline-offset: 1px;
  }

  option,
  optgroup {
    color: #f2f2f2;
    background: #2e2e2e;
  }
</style>
