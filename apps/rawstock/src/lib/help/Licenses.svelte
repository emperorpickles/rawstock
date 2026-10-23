<script lang="ts">
  import { LicenseIndex } from "./licenses";
  import type { LicenseData } from "./licenses";

  let { open = false, onclose }: { open?: boolean; onclose: () => void } = $props();

  // Written by scripts/licenses.mjs before every dev and build run, and
  // gitignored. The glob is empty when the script has not run, so a type
  // check works from a fresh checkout.
  // `?url` ships it as a plain asset, fetched only when the view opens.
  const generated = import.meta.glob<string>("../../generated/licenses.json", {
    query: "?url",
    import: "default",
  });

  let dialog = $state<HTMLDialogElement | null>(null);
  let data = $state<LicenseData | null>(null);
  let failed = $state(false);
  let query = $state("");

  const groups = $derived(data ? LicenseIndex.groups(data.packages, query) : []);
  const total = $derived(data?.packages.length ?? 0);

  $effect(() => {
    const node = dialog;
    if (!node) {
      return;
    }
    if (open && !node.open) {
      node.showModal();
      void load();
    }
    if (!open && node.open) {
      node.close();
    }
  });

  async function load() {
    if (data) {
      return;
    }
    const loader = Object.values(generated)[0];
    if (!loader) {
      failed = true;
      return;
    }
    try {
      const response = await fetch(await loader());
      data = (await response.json()) as LicenseData;
    } catch {
      failed = true;
    }
  }
</script>

<dialog bind:this={dialog} class="licenses" aria-labelledby="licenses-title" onclose={onclose}>
  <h2 id="licenses-title">Licenses</h2>
  {#if failed}
    <p>The license list was not generated for this build. Run <code>npm run licenses</code>.</p>
  {:else if !data}
    <p aria-live="polite">Loading licenses…</p>
  {:else}
    <details open>
      <summary>rawstock: {data.rawstock.license}</summary>
      <pre>{data.texts[data.rawstock.text]}</pre>
    </details>

    <label class="field">
      Search {total} packages
      <input type="search" bind:value={query} placeholder="Name or license" />
    </label>

    <div class="groups">
      {#each groups as group (group.license)}
        <section aria-label={group.license}>
          <h3>{group.license} <span class="count">({group.packages.length})</span></h3>
          {#each group.packages as pkg (`${pkg.ecosystem}:${pkg.name}@${pkg.version}`)}
            <details>
              <summary>
                {pkg.name} {pkg.version}
                {#if pkg.ecosystem === "npm"}<span class="count">npm</span>{/if}
              </summary>
              {#if pkg.note}<p class="note">{pkg.note}</p>{/if}
              {#if pkg.repository}<p class="note">Source: {pkg.repository}</p>{/if}
              {#each pkg.texts as id (id)}
                <pre>{data.texts[id]}</pre>
              {/each}
            </details>
          {/each}
        </section>
      {:else}
        <p>No package matches “{query}”.</p>
      {/each}
    </div>
  {/if}
  <div class="dialog-actions">
    <button type="button" onclick={onclose}>Close</button>
  </div>
</dialog>

<style>
  .licenses {
    max-width: 52rem;
    max-height: calc(100vh - 4rem);
  }

  .groups {
    max-height: 50vh;
    overflow: auto;
    padding-right: 0.3rem;
  }

  h3 {
    font-size: 0.95rem;
    margin: 0.9rem 0 0.3rem;
  }

  .count {
    color: #9a9a9a;
    font-weight: normal;
    margin-left: 0.3rem;
  }

  summary {
    cursor: pointer;
    padding: 0.15rem 0;
  }

  pre {
    white-space: pre-wrap;
    font-size: 0.8rem;
    max-height: 18rem;
    overflow: auto;
    background: #141414;
    border: 1px solid #3a3a3a;
    border-radius: 6px;
    padding: 0.6rem;
  }

  .note {
    font-size: 0.85rem;
  }

  input[type="search"] {
    background: #141414;
    border: 1px solid #3a3a3a;
    border-radius: 6px;
    padding: 0.3rem 0.5rem;
  }
</style>
