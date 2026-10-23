<script lang="ts">
  import { save } from "@tauri-apps/plugin-dialog";

  import { appError } from "./contract";
  import type { EditSettings, ImageInfo } from "./contract";
  import { ExportPlan } from "./export";
  import type { ExportRequest } from "./export";
  import { Host } from "./host";
  import Slider from "./Slider.svelte";

  let {
    open = false,
    image,
    settings,
    quality = 92,
    lastFolder = null,
    onclose,
    onexport,
  }: {
    open?: boolean;
    image: ImageInfo | null;
    settings: EditSettings;
    quality?: number;
    lastFolder?: string | null;
    onclose: () => void;
    onexport: (request: ExportRequest) => Promise<void>;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let replaceButton = $state<HTMLButtonElement | null>(null);
  let step = $state<"form" | "confirm">("form");
  let draftQuality = $state(92);
  let destination = $state("");
  let problem = $state("");
  let confirmText = $state("");
  let busy = $state(false);

  const review = $derived(
    image && destination ? ExportPlan.review(image.path, destination, "missing") : null,
  );
  const refused = $derived(review?.action === "refuse");
  const fileLabel = $derived(destination ? ExportPlan.baseName(destination) : "—");
  const folderLabel = $derived(destination ? ExportPlan.folder(destination) || "—" : "—");

  $effect(() => {
    const node = dialog;
    if (!node) {
      return;
    }
    if (open && !node.open) {
      step = "form";
      problem = "";
      confirmText = "";
      busy = false;
      draftQuality = quality;
      destination = image ? ExportPlan.defaultPath(image.path, lastFolder) : "";
      node.showModal();
    }
    if (!open && node.open) {
      node.close();
    }
  });

  $effect(() => {
    if (step === "confirm") {
      replaceButton?.focus();
    }
  });

  async function choose() {
    if (!image || busy) {
      return;
    }
    if (!Host.available()) {
      problem = "Choose the file from the rawstock desktop window.";
      return;
    }
    const picked = await save({
      title: "Export JPEG",
      defaultPath: destination || ExportPlan.defaultPath(image.path, lastFolder),
      filters: [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
      canCreateDirectories: true,
    });
    if (!picked) {
      return;
    }
    const next = ExportPlan.review(image.path, picked, "missing");
    destination = next.path;
    step = "form";
    problem = next.action === "refuse" ? next.message : "";
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    void send(false);
  }

  async function send(overwrite: boolean) {
    if (!image || busy) {
      return;
    }
    const next = ExportPlan.review(image.path, destination, "missing");
    destination = next.path;
    if (next.action === "refuse") {
      problem = next.message;
      step = "form";
      return;
    }
    busy = true;
    problem = "";
    try {
      await onexport(
        ExportPlan.request(image.imageId, settings, draftQuality, next.path, overwrite),
      );
    } catch (error) {
      const app = appError(error);
      busy = false;
      if (ExportPlan.isReplacePrompt(app.message)) {
        confirmText = app.message;
        step = "confirm";
        return;
      }
      problem = app.message;
      step = "form";
    }
  }
</script>

<dialog
  bind:this={dialog}
  class="sheet"
  aria-labelledby="export-title"
  aria-describedby="export-note"
  onclose={onclose}
>
  {#if step === "confirm"}
    <h2 id="export-title">Replace file?</h2>
    <p id="export-note">{confirmText}</p>
    <div class="dialog-actions">
      <button type="button" onclick={() => (step = "form")} disabled={busy}>Back</button>
      <button
        type="button"
        class="primary"
        bind:this={replaceButton}
        onclick={() => void send(true)}
        disabled={busy}
      >
        Replace
      </button>
    </div>
  {:else}
    <form onsubmit={submit}>
      <h2 id="export-title">Export</h2>
      <div class="group">
        <label class="field">
          Format
          <select aria-label="Format" disabled={!image || busy}>
            <option value="jpeg">JPEG</option>
          </select>
        </label>
        <p class="name" id="color-label">Color</p>
        <p class="readout" aria-labelledby="color-label">sRGB</p>
      </div>

      <Slider
        label="Quality"
        value={draftQuality}
        scale={ExportPlan.scale}
        disabled={!image || busy}
        onpreview={(value) => (draftQuality = value)}
        oncommit={(value) => (draftQuality = ExportPlan.quality(value))}
      />

      <div class="group">
        <p class="name" id="file-label">File</p>
        <p class="path" aria-labelledby="file-label">{fileLabel}</p>
        <p class="name" id="folder-label">Folder</p>
        <p class="path folder" aria-labelledby="folder-label">{folderLabel}</p>
        <button type="button" onclick={() => void choose()} disabled={!image || busy}>
          Choose…
        </button>
      </div>

      {#if refused && review}
        <p class="problem" role="alert">{review.message}</p>
      {:else if problem}
        <p class="problem" role="alert">{problem}</p>
      {/if}
      <p id="export-note" class="note">The RAW file and its edit are not changed.</p>

      <div class="dialog-actions">
        <button type="button" onclick={onclose}>Cancel</button>
        <button type="submit" class="primary" disabled={!image || refused || busy || !destination}>
          Export
        </button>
      </div>
    </form>
  {/if}
</dialog>

<style>
  dialog.sheet {
    width: min(24rem, calc(100% - 2rem));
    max-height: calc(100% - 2rem);
    overflow: auto;
    padding: 0.85rem 0.9rem 0.9rem;
  }

  h2 {
    margin: 0 0 0.65rem;
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .group {
    margin: 0 0 0.55rem;
  }

  .field {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 0.25rem;
    margin: 0 0 0.45rem;
    font-size: 0.85rem;
  }

  .name,
  .note,
  .readout,
  .path {
    margin: 0.1rem 0 0.3rem;
  }

  .name {
    font-size: 0.85rem;
  }

  .note,
  .readout,
  .folder {
    color: var(--muted);
    font-size: 0.78rem;
  }

  .readout,
  .path {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }

  .readout {
    color: var(--accent);
  }

  .path {
    color: var(--ink);
    font-size: 0.78rem;
    word-break: break-all;
  }

  .path.folder {
    color: var(--muted);
  }

  .problem {
    margin: 0.2rem 0 0.45rem;
    color: var(--danger);
    font-size: 0.78rem;
  }

  select {
    background: var(--app);
    color: var(--ink);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 0.25rem 0.35rem;
  }

  button {
    border-radius: 4px;
    font-size: 0.78rem;
  }

  .dialog-actions {
    margin-top: 0.75rem;
  }
</style>
