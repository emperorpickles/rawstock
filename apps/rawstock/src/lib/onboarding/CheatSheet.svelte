<script lang="ts">
  import { Shortcuts } from "../shortcuts";
  import type { ShortcutPlatform } from "../shortcuts";

  let { platform }: { platform: ShortcutPlatform } = $props();

  const rows = $derived(Shortcuts.cheatSheet(platform));
</script>

<table class="cheat-sheet">
  <caption class="visually-hidden">Keyboard shortcuts</caption>
  <thead>
    <tr>
      <th scope="col">Action</th>
      <th scope="col">Keys</th>
    </tr>
  </thead>
  <tbody>
    {#each rows as row (row.action)}
      <tr>
        <td>{row.label}</td>
        <td><kbd>{row.keys}</kbd></td>
      </tr>
    {/each}
  </tbody>
</table>
<p class="hint">Single keys are ignored while you type in a text field.</p>

<style>
  .cheat-sheet {
    width: 100%;
    border-collapse: collapse;
  }

  th,
  td {
    text-align: left;
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid #2e2e2e;
  }

  th {
    color: #b8b8b8;
    font-weight: normal;
  }

  kbd {
    font: inherit;
    border: 1px solid #4a4a4a;
    border-radius: 4px;
    padding: 0 0.35rem;
    background: #141414;
  }

  .hint {
    font-size: 0.85rem;
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
  }
</style>
