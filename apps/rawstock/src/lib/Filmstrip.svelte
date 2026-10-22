<script lang="ts">
  import type { ImageInfo } from "./contract";

  let {
    images,
    selected = "",
    onselect,
  }: {
    images: ImageInfo[];
    selected?: string;
    onselect: (imageId: string) => void;
  } = $props();
</script>

<nav class="filmstrip" aria-label="Filmstrip">
  {#if images.length === 0}
    <p class="empty">Opened photos appear here.</p>
  {:else}
    {#each images as image (image.imageId)}
      <button
        type="button"
        class:selected={image.imageId === selected}
        aria-current={image.imageId === selected ? "true" : undefined}
        onclick={() => onselect(image.imageId)}
      >
        {image.fileName}
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
    text-align: left;
    border-radius: 6px;
    padding: 0.45rem 0.55rem;
    background: transparent;
    color: inherit;
    border: 1px solid transparent;
  }

  button.selected {
    border-color: #6a6a6a;
    background: #2a2a2a;
  }
</style>
