<script lang="ts">
  import { Report } from "./report";
  import type { AppInfo } from "./report";

  let {
    open = false,
    info,
    onclose,
    onlicenses,
  }: {
    open?: boolean;
    info: AppInfo | null;
    onclose: () => void;
    onlicenses: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let copied = $state("");

  $effect(() => {
    const node = dialog;
    if (!node) {
      return;
    }
    if (open && !node.open) {
      copied = "";
      node.showModal();
    }
    if (!open && node.open) {
      node.close();
    }
  });

  async function copyDetails() {
    if (!info) {
      return;
    }
    try {
      await navigator.clipboard.writeText(Report.details(info));
      copied = "Details copied.";
    } catch {
      copied = "Copying failed. Select the details and copy them instead.";
    }
  }
</script>

<dialog bind:this={dialog} aria-labelledby="about-title" onclose={onclose}>
  <h2 id="about-title">About rawstock</h2>
  <p>rawstock develops Fuji X-Trans and iPhone ProRAW files through film stocks.</p>
  {#if info}
    <dl class="facts">
      <dt>Version</dt>
      <dd>{info.version}</dd>
      <dt>Commit</dt>
      <dd><code>{info.commit}</code></dd>
      <dt>Built</dt>
      <dd>{info.buildDate}</dd>
      <dt>System</dt>
      <dd>{Report.osName(info.os)} ({info.arch})</dd>
      <dt>Log file</dt>
      <dd><code class="path">{info.logPath}</code></dd>
    </dl>
  {:else}
    <p>Build details are shown in the desktop app.</p>
  {/if}
  <p>
    Copyright the rawstock contributors. rawstock is free software: you can redistribute it and
    modify it under the GNU General Public License, version 3 or any later version. It comes with
    no warranty.
  </p>
  <p class="status" aria-live="polite">{copied}</p>
  <div class="dialog-actions">
    <button type="button" onclick={() => void copyDetails()} disabled={!info}>Copy details</button>
    <button type="button" onclick={onlicenses}>Licenses…</button>
    <button type="button" onclick={onclose}>Close</button>
  </div>
</dialog>

<style>
  .facts {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 0.25rem 0.9rem;
    margin: 0.8rem 0;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
  }

  .path {
    overflow-wrap: anywhere;
  }

  .status {
    min-height: 1.4em;
    margin: 0;
  }
</style>
