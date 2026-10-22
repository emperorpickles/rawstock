<script lang="ts">
  import { tick } from "svelte";

  import type { ShortcutPlatform } from "../shortcuts";
  import CheatSheet from "./CheatSheet.svelte";

  type Mode = "welcome" | "cheat-sheet";

  let {
    open = false,
    mode = "welcome",
    platform,
    sampleAvailable = false,
    onopen,
    onopenfolder,
    onsample,
    onclose,
  }: {
    open?: boolean;
    mode?: Mode;
    platform: ShortcutPlatform;
    sampleAvailable?: boolean;
    onopen: () => void;
    onopenfolder: () => void;
    onsample: () => void;
    /** `dontShowAgain` is only meaningful for the welcome flow. */
    onclose: (dontShowAgain: boolean) => void;
  } = $props();

  const mod = $derived(platform === "mac" ? "Cmd" : "Ctrl");
  const steps = ["Open a RAF or ProRAW DNG", "Pick a stock", "Export", "Keyboard shortcuts"];

  let dialog = $state<HTMLDialogElement | null>(null);
  let heading = $state<HTMLHeadingElement | null>(null);
  let step = $state(0);
  let dontShowAgain = $state(true);
  // `close` fires for Esc, our own buttons, and a parent closing us. Report once.
  let reported = false;
  const last = $derived(mode === "cheat-sheet" || step === steps.length - 1);

  $effect(() => {
    const node = dialog;
    if (!node) {
      return;
    }
    if (open && !node.open) {
      step = mode === "cheat-sheet" ? steps.length - 1 : 0;
      dontShowAgain = true;
      reported = false;
      node.showModal();
      void focusHeading();
    }
    if (!open && node.open) {
      node.close();
    }
  });

  async function focusHeading() {
    await tick();
    heading?.focus();
  }

  function go(delta: number) {
    step = Math.min(steps.length - 1, Math.max(0, step + delta));
    void focusHeading();
  }

  function finish() {
    dialog?.close();
  }

  function report() {
    if (reported) {
      return;
    }
    reported = true;
    onclose(dontShowAgain);
  }

  function leaveFor(action: () => void) {
    finish();
    action();
  }
</script>

<!-- Esc fires `close`, which skips the flow. -->
<dialog
  bind:this={dialog}
  class="onboarding"
  aria-labelledby="onboarding-step-title"
  onclose={report}
>
  {#if mode === "welcome"}
    <p class="progress">Step {step + 1} of {steps.length}</p>
  {/if}
  <h2 id="onboarding-step-title" tabindex="-1" bind:this={heading}>
    {mode === "welcome" && step === 0 ? `Welcome to rawstock. ${steps[0]}` : steps[step]}
  </h2>

  <div class="body" aria-live="polite">
    {#if step === 0}
      <p>
        rawstock develops Fuji X-Trans RAF files and iPhone ProRAW DNGs. Open one, open a folder of
        them, or drop files on the window.
      </p>
      <div class="actions">
        <button type="button" class="primary" onclick={() => leaveFor(onopen)}>Open…</button>
        <button type="button" onclick={() => leaveFor(onopenfolder)}>Open Folder…</button>
        {#if sampleAvailable}
          <button type="button" onclick={() => leaveFor(onsample)}>Try the sample photo</button>
        {/if}
      </div>
    {:else if step === 1}
      <p>
        The stock list on the right holds film stocks and Fuji simulations. Choosing one develops
        the photo through that film.
      </p>
      <p>
        Judge grain at 100% ({mod}+1), not in Fit. Fit scales the photo down, which hides grain.
      </p>
    {:else if step === 2}
      <p>Export with {mod}+E to write a JPEG. Your RAW file is never changed.</p>
    {:else}
      <CheatSheet {platform} />
    {/if}
  </div>

  {#if mode === "welcome"}
    <label class="check">
      <input type="checkbox" bind:checked={dontShowAgain} />
      Don't show this again
    </label>
  {/if}

  <div class="dialog-actions">
    {#if mode === "welcome" && !last}
      <button type="button" onclick={finish}>Skip</button>
    {/if}
    <span class="spacer"></span>
    {#if mode === "welcome" && step > 0}
      <button type="button" onclick={() => go(-1)}>Back</button>
    {/if}
    {#if last}
      <button type="button" class="primary" onclick={finish}>Done</button>
    {:else}
      <button type="button" class="primary" onclick={() => go(1)}>Next</button>
    {/if}
  </div>
</dialog>

<style>
  .onboarding {
    max-width: 32rem;
  }

  .progress {
    margin: 0 0 0.3rem;
    font-size: 0.85rem;
    color: #b8b8b8;
  }

  /* Focus moves to the heading so a screen reader reads each step. It is
     not a control, so it gets no ring. */
  h2:focus {
    outline: none;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-top: 0.8rem;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 1rem;
  }

  .spacer {
    flex: 1;
  }
</style>
