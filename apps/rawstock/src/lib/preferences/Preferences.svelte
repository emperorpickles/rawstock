<script lang="ts">
  import type { StockSummary } from "../contract";
  import { RECENT_MAX, RECENT_MIN } from "./settings";
  import type { AppSettings, ReduceMotion, Surround, Theme } from "./settings";

  let {
    open = false,
    settings,
    stocks,
    title = "Preferences",
    onchange,
    onreset,
    onclose,
  }: {
    open?: boolean;
    settings: AppSettings;
    stocks: StockSummary[];
    title?: string;
    onchange: (next: AppSettings) => void;
    onreset: (clearRecent: boolean) => void;
    onclose: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let confirming = $state(false);
  let clearRecent = $state(false);
  let recentLimit = $state(10);

  $effect(() => {
    recentLimit = settings.recent.limit;
  });

  $effect(() => {
    const node = dialog;
    if (!node) {
      return;
    }
    if (open && !node.open) {
      confirming = false;
      clearRecent = false;
      node.showModal();
    }
    if (!open && node.open) {
      node.close();
    }
  });

  function setTheme(theme: Theme) {
    onchange({ ...settings, appearance: { ...settings.appearance, theme } });
  }

  function setSurround(surround: Surround) {
    onchange({ ...settings, appearance: { ...settings.appearance, surround } });
  }

  function setReduceMotion(reduceMotion: ReduceMotion) {
    onchange({ ...settings, appearance: { ...settings.appearance, reduceMotion } });
  }

  function setDefaultStock(value: string) {
    onchange({
      ...settings,
      newImage: { ...settings.newImage, defaultStockId: value === "" ? null : value },
    });
  }

  function commitRecentLimit() {
    const limit = Math.min(RECENT_MAX, Math.max(RECENT_MIN, Math.round(recentLimit)));
    if (limit !== settings.recent.limit) {
      onchange({ ...settings, recent: { ...settings.recent, limit } });
    }
  }

  function confirmReset() {
    onreset(clearRecent);
    confirming = false;
    clearRecent = false;
  }
</script>

<dialog bind:this={dialog} aria-labelledby="preferences-title" onclose={onclose}>
  <h2 id="preferences-title">{title}</h2>

  {#if confirming}
    <section aria-labelledby="reset-title">
      <h3 id="reset-title">Reset all settings?</h3>
      <p>
        Every preference goes back to its default. Photos, sidecars, and exports are not
        touched.
      </p>
      <label class="check">
        <input type="checkbox" bind:checked={clearRecent} />
        Also clear recent files
      </label>
      <div class="dialog-actions">
        <button type="button" onclick={() => (confirming = false)}>Cancel</button>
        <button type="button" class="primary" onclick={confirmReset}>Reset</button>
      </div>
    </section>
  {:else}
    <fieldset>
      <legend>Appearance</legend>
      <label class="check">
        <input
          type="radio"
          name="theme"
          value="warm"
          checked={settings.appearance.theme !== "neutral"}
          onchange={() => setTheme("warm")}
        />
        Warm
      </label>
      <label class="check">
        <input
          type="radio"
          name="theme"
          value="neutral"
          checked={settings.appearance.theme === "neutral"}
          onchange={() => setTheme("neutral")}
        />
        Neutral
      </label>
    </fieldset>

    <fieldset>
      <legend>Viewer surround</legend>
      <label class="check">
        <input
          type="radio"
          name="surround"
          value="dark"
          checked={settings.appearance.surround === "dark"}
          onchange={() => setSurround("dark")}
        />
        Dark grey
      </label>
      <label class="check">
        <input
          type="radio"
          name="surround"
          value="mid"
          checked={settings.appearance.surround === "mid"}
          onchange={() => setSurround("mid")}
        />
        Mid grey
      </label>
    </fieldset>

    <label class="field">
      Reduce motion
      <select
        value={settings.appearance.reduceMotion}
        onchange={(event) => setReduceMotion(event.currentTarget.value as ReduceMotion)}
      >
        <option value="system">Follow the system</option>
        <option value="on">Always</option>
        <option value="off">Never</option>
      </select>
    </label>

    <label class="field">
      Stock for new photos
      <select
        value={settings.newImage.defaultStockId ?? ""}
        onchange={(event) => setDefaultStock(event.currentTarget.value)}
      >
        <option value="">First stock in the list</option>
        {#each stocks as stock (stock.id)}
          <option value={stock.id}>{stock.name}</option>
        {/each}
        {#if settings.newImage.defaultStockId && !stocks.some((stock) => stock.id === settings.newImage.defaultStockId)}
          <option value={settings.newImage.defaultStockId}>
            {settings.newImage.defaultStockId} (not installed)
          </option>
        {/if}
      </select>
    </label>

    <label class="field">
      Recent files to remember: {recentLimit}
      <input
        type="range"
        min={RECENT_MIN}
        max={RECENT_MAX}
        step="1"
        bind:value={recentLimit}
        onchange={commitRecentLimit}
        aria-valuetext={`${recentLimit} files`}
      />
    </label>

    <div class="dialog-actions">
      <button type="button" onclick={() => (confirming = true)}>Reset all settings…</button>
      <span class="spacer"></span>
      <button type="button" onclick={onclose}>Close</button>
    </div>
  {/if}
</dialog>

<style>
  fieldset {
    border: 1px solid var(--line);
    border-radius: 8px;
    margin: 0.7rem 0;
    padding: 0.5rem 0.8rem 0.7rem;
  }

  legend {
    padding: 0 0.3rem;
  }

  h3 {
    margin: 0 0 0.5rem;
    font-size: 1rem;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0.35rem 0;
  }

  select {
    background: var(--app);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 0.3rem 0.4rem;
  }

  .spacer {
    flex: 1;
  }
</style>
