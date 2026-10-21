<script lang="ts">
  import { LogSlider, Readout, SliderRange } from "./ranges";
  import type { ControlEdit, StockState, StockSummary } from "./types";

  let {
    stocks,
    selected,
    stock,
    fileOpen,
    onchoose,
    onopenstock,
    onedit,
  }: {
    stocks: StockSummary[];
    selected: string;
    stock: StockState | null;
    fileOpen: boolean;
    onchoose: (path: string) => void;
    onopenstock: () => void;
    onedit: (edit: ControlEdit) => void;
  } = $props();

  let exposure = $derived(
    SliderRange.including(-3, 3, 0.01, stock?.exposureEv ?? 0),
  );
  let printExposure = $derived(
    SliderRange.including(0, 2.5, 0.01, stock?.printExposure ?? 0.9),
  );
  let grain = $derived(SliderRange.including(0, 2, 0.01, stock?.grainAmount ?? 1));
  let radius = $derived(SliderRange.including(0, 0.05, 0.0001, stock?.halationRadius ?? 0.008));
  let red = $derived(SliderRange.including(0, 1, 0.01, stock?.halationWeight?.[0] ?? 0));
  let threshold = $derived(LogSlider.threshold(stock?.halationThreshold ?? 0.25));

  function numberValue(event: Event): number {
    return Number((event.currentTarget as HTMLInputElement).value);
  }
</script>

<aside class="panel">
  <h2>Stocks</h2>
  <div class="stocks">
    {#each stocks as item (item.id)}
      <button
        type="button"
        class:on={item.id === selected}
        disabled={!fileOpen}
        onclick={() => onchoose(item.path)}
      >
        <span>{item.name}</span>
        <small>{item.model} · revision {item.revision}</small>
      </button>
    {/each}
    <button type="button" disabled={!fileOpen} onclick={onopenstock}>Open stock…</button>
  </div>

  {#if stock}
    <p class="meta">
      {stock.model === "spectral" ? "Spectral" : "Parametric"} model. Sensitivities and wavelength
      samples are measured data and stay as loaded.
    </p>
    <div class="group">
      <label class="slider">
        <span>Exposure</span>
        <span class="readout">{Readout.fixed(stock.exposureEv, 2)} ev</span>
        <input
          type="range"
          min={exposure.min}
          max={exposure.max}
          step={exposure.step}
          value={stock.exposureEv}
          oninput={(event) => onedit({ control: "exposure", value: numberValue(event) })}
        />
      </label>
      <label class="slider">
        <span>Print exposure</span>
        <span class="readout">{Readout.fixed(stock.printExposure, 2)}</span>
        <input
          type="range"
          min={printExposure.min}
          max={printExposure.max}
          step={printExposure.step}
          value={stock.printExposure}
          oninput={(event) => onedit({ control: "printExposure", value: numberValue(event) })}
        />
      </label>
      {#if stock.dyeScales}
        {#each ["Red dye", "Green dye", "Blue dye"] as label, index (label)}
          <label class="slider">
            <span>{label}</span>
            <span class="readout">{Readout.fixed(stock.dyeScales[index] ?? 1, 2)}</span>
            <input
              type="range"
              min={SliderRange.including(0.25, 2.5, 0.01, stock.dyeScales[index] ?? 1).min}
              max={SliderRange.including(0.25, 2.5, 0.01, stock.dyeScales[index] ?? 1).max}
              step="0.01"
              value={stock.dyeScales[index]}
              oninput={(event) => {
                const next: [number, number, number] = [
                  stock.dyeScales?.[0] ?? 1,
                  stock.dyeScales?.[1] ?? 1,
                  stock.dyeScales?.[2] ?? 1,
                ];
                next[index] = numberValue(event);
                onedit({ control: "dyeScales", value: next });
              }}
            />
          </label>
        {/each}
      {/if}
      <label class="slider">
        <span>Grain amount</span>
        <span class="readout">{Readout.fixed(stock.grainAmount, 2)}</span>
        <input
          type="range"
          min={grain.min}
          max={grain.max}
          step={grain.step}
          value={stock.grainAmount}
          disabled={!stock.grainAvailable}
          oninput={(event) => onedit({ control: "grainAmount", value: numberValue(event) })}
        />
      </label>
      {#if !stock.grainAvailable}
        <p class="hint">This stock has no grain table.</p>
      {/if}
      <label class="check">
        <input
          type="checkbox"
          checked={stock.halationEnabled}
          onchange={(event) =>
            onedit({
              control: "halationEnabled",
              value: (event.currentTarget as HTMLInputElement).checked,
            })}
        />
        Halation
      </label>
      <label class="slider">
        <span>Threshold</span>
        <span class="readout">{Readout.fixed(stock.halationThreshold, 3)}</span>
        <input
          type="range"
          min="0"
          max="1000"
          step="1"
          value={threshold.toInput(stock.halationThreshold)}
          oninput={(event) =>
            onedit({ control: "halationThreshold", value: threshold.fromInput(numberValue(event)) })}
        />
      </label>
      <label class="slider">
        <span>Radius</span>
        <span class="readout">{Readout.fixed(stock.halationRadius, 4)}</span>
        <input
          type="range"
          min={radius.min}
          max={radius.max}
          step={radius.step}
          value={stock.halationRadius}
          oninput={(event) => onedit({ control: "halationRadius", value: numberValue(event) })}
        />
      </label>
      <label class="slider">
        <span>Red weight</span>
        <span class="readout">{Readout.fixed(stock.halationWeight[0], 2)}</span>
        <input
          type="range"
          min={red.min}
          max={red.max}
          step={red.step}
          value={stock.halationWeight[0]}
          oninput={(event) => onedit({ control: "halationRed", value: numberValue(event) })}
        />
      </label>
      <p class="weights">
        Green {Readout.fixed(stock.halationWeight[1], 2)}, blue {Readout.fixed(stock.halationWeight[2], 2)}.
        Those two stay as loaded.
      </p>
    </div>
  {:else}
    <p class="hint">Open a RAW file, then pick a stock. Dye scale appears on spectral stocks.</p>
  {/if}
</aside>
