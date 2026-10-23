<script lang="ts">
  import type { ImageInfo } from "./contract";
  import { filmstripBadges, filmstripLabel } from "./edits";

  let {
    images,
    selected = "",
    onselect,
  }: {
    images: ImageInfo[];
    selected?: string;
    onselect: (imageId: string) => void;
  } = $props();

  $effect(() => {
    const current = selected;
    if (!current) {
      return;
    }
    document.getElementById(`strip-${current}`)?.scrollIntoView({ block: "nearest" });
  });
</script>

<nav class="filmstrip" aria-label="Filmstrip">
  {#if images.length === 0}
    <p class="empty">Opened photos appear here.</p>
  {:else}
    {#each images as image (image.imageId)}
      {@const badges = filmstripBadges(image)}
      <button
        id="strip-{image.imageId}"
        type="button"
        class:selected={image.imageId === selected}
        aria-current={image.imageId === selected ? "true" : undefined}
        aria-label={filmstripLabel(image)}
        title={image.error?.message}
        onclick={() => onselect(image.imageId)}
      >
        <span class="thumb" aria-hidden="true"></span>
        <span class="copy">
          <span class="name">{image.fileName}</span>
          {#if badges.length}
            <span class="badges">
              {#each badges as badge (badge)}
                <span class="badge">{badge}</span>
              {/each}
            </span>
          {/if}
        </span>
      </button>
    {/each}
  {/if}
</nav>

<style>
  .filmstrip {
    background: #1c1c1c;
    border-right: 1px solid #3a3a3a;
    overflow: auto;
    padding: 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    min-height: 0;
  }

  .empty {
    margin: 0;
    color: #b8b8b8;
    font-size: 0.85rem;
  }

  button {
    display: flex;
    gap: 0.45rem;
    align-items: center;
    text-align: left;
    border-radius: 6px;
    padding: 0.4rem;
    background: transparent;
    color: inherit;
    border: 1px solid transparent;
  }

  button.selected {
    border-color: #6a6a6a;
    background: #2a2a2a;
  }

  .thumb {
    width: 2.4rem;
    height: 1.8rem;
    flex: none;
    border-radius: 3px;
    background: #2e2e2e;
    border: 1px solid #3a3a3a;
  }

  .copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: 0.2rem;
  }

  .badge {
    font-size: 0.68rem;
    letter-spacing: 0.02em;
    color: #d0d0d0;
    background: #2a2a2a;
    border: 1px solid #4a4a4a;
    border-radius: 999px;
    padding: 0 0.35rem;
  }
</style>
