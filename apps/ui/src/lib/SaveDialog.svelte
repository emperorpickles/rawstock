<script lang="ts">
  import { untrack } from "svelte";

  import type { StockState } from "./types";

  let {
    stock,
    oncancel,
    onsave,
  }: {
    stock: StockState;
    oncancel: () => void;
    onsave: (draft: { name: string; source: string; license: string; revision: number }) => void;
  } = $props();

  const opened = untrack(() => stock);
  let name = $state(opened.name);
  let source = $state(`Tuned from ${opened.name} in rawstock.`);
  let license = $state(opened.license);
  let revision = $state(opened.revision + 1);
  let problem = $state("");

  function submit(event: Event) {
    event.preventDefault();
    if (!name.trim() || !source.trim() || !license.trim()) {
      problem = "Name, source, and license need text.";
      return;
    }
    if (!Number.isInteger(revision) || revision < 0) {
      problem = "Revision has to be a whole number.";
      return;
    }
    onsave({ name: name.trim(), source: source.trim(), license: license.trim(), revision });
  }
</script>

<div class="dialog-back">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="save-title">
  <form onsubmit={submit}>
    <h2 id="save-title">Save as a new stock</h2>
    <p>
      An invented look needs an original name. A transcription of a published film may keep that
      film's name when source and license attribute it. Comments in the opened file are not kept.
      Bundled stocks are not overwritten.
    </p>
    <div class="fields">
      <label>
        Name
        <input bind:value={name} />
      </label>
      <label>
        Source
        <textarea rows="3" bind:value={source}></textarea>
      </label>
      <label>
        License
        <input bind:value={license} />
      </label>
      <label>
        Revision
        <input type="number" min="0" step="1" bind:value={revision} />
      </label>
    </div>
    {#if problem}
      <p class="fault">{problem}</p>
    {/if}
    <div class="dialog-actions">
      <button type="button" onclick={oncancel}>Cancel</button>
      <button type="submit" class="primary">Choose file…</button>
    </div>
  </form>
  </div>
</div>
