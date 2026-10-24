<script lang="ts">
  import type { ImageInfo } from "./contract";
  import { filmstripBadges, filmstripLabel } from "./edits";

  let {
    images,
    selected = "",
    collapsed = false,
    onselect,
    oncollapse,
  }: {
    images: ImageInfo[];
    selected?: string;
    collapsed?: boolean;
    onselect: (imageId: string) => void;
    oncollapse: (collapsed: boolean) => void;
  } = $props();

  const position = $derived.by(() => {
    const index = images.findIndex((image) => image.imageId === selected);
    return index < 0 ? "" : `${index + 1} of ${images.length}`;
  });

  $effect(() => {
    const current = selected;
    if (!current || collapsed) {
      return;
    }
    document
      .getElementById(`strip-${current}`)
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  });

  /** Thumbnail box: the photo's own aspect, fitted inside 96 x 64 px. */
  function box(image: ImageInfo): { width: string; height: string } {
    const ratio = image.width && image.height ? image.width / image.height : 1.5;
    const width = Math.min(96, 64 * ratio);
    return { width: `${Math.round(width)}px`, height: `${Math.round(width / ratio)}px` };
  }
</script>

<section class="strip" class:collapsed aria-label="Filmstrip">
  <header>
    <h2>Filmstrip</h2>
    <span class="count">{position || (images.length ? `${images.length} photos` : "")}</span>
    <span class="spacer"></span>
    <button
      type="button"
      class="fold"
      aria-expanded={!collapsed}
      aria-controls="filmstrip-frames"
      onclick={() => oncollapse(!collapsed)}
    >
      {collapsed ? "Show" : "Hide"}
    </button>
  </header>

  {#if !collapsed}
    <nav id="filmstrip-frames" class="frames" aria-label="Photos">
      {#if images.length === 0}
        <p class="empty">Opened photos appear here.</p>
      {:else}
        {#each images as image (image.imageId)}
          {@const badges = filmstripBadges(image)}
          {@const size = box(image)}
          <button
            id="strip-{image.imageId}"
            type="button"
            class="frame"
            class:selected={image.imageId === selected}
            class:broken={image.error != null}
            aria-current={image.imageId === selected ? "true" : undefined}
            aria-label={filmstripLabel(image)}
            title={image.error?.message ?? image.fileName}
            onclick={() => onselect(image.imageId)}
          >
            <span class="mat" aria-hidden="true">
              <span class="thumb" style:width={size.width} style:height={size.height}></span>
              {#if badges.length}
                <span class="badges">
                  {#each badges as badge (badge)}
                    <span class="badge">{badge}</span>
                  {/each}
                </span>
              {/if}
            </span>
            <span class="name" aria-hidden="true">{image.fileName}</span>
          </button>
        {/each}
      {/if}
    </nav>
  {/if}
</section>

<style>
  .strip {
    background: var(--panel);
    border-top: 1px solid var(--line);
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.2rem 0.5rem 0.2rem 0.75rem;
    min-height: 1.9rem;
  }

  h2 {
    margin: 0;
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .count {
    font-family: var(--font-mono);
    font-size: 0.72rem;
    color: var(--muted);
  }

  .spacer {
    flex: 1;
  }

  .fold {
    padding: 0.1rem 0.6rem;
    font-size: 0.75rem;
    color: var(--muted);
  }

  .frames {
    display: flex;
    gap: 0.5rem;
    align-items: flex-start;
    overflow-x: auto;
    overflow-y: hidden;
    padding: 0.15rem 0.75rem 0.6rem;
    scrollbar-width: thin;
  }

  .empty {
    margin: 0.4rem 0;
    color: var(--muted);
    font-size: 0.85rem;
  }

  .frame {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.3rem;
    width: 108px;
    padding: 4px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
  }

  .frame:hover {
    background: var(--paper);
  }

  .frame.selected {
    border-color: var(--accent);
    background: var(--paper);
  }

  /* The mat is never themed: thumbnails sit on 2 px of neutral grey. */
  .mat {
    position: relative;
    height: 64px;
    width: 96px;
    display: grid;
    place-items: center;
  }

  .thumb {
    box-sizing: border-box;
    border: 2px solid var(--mat);
    background: var(--surround);
    border-radius: 2px;
  }

  .broken .thumb {
    border-style: dashed;
    opacity: 0.6;
  }

  .badges {
    position: absolute;
    left: 0.2rem;
    bottom: 0.2rem;
    display: flex;
    flex-wrap: wrap;
    gap: 0.15rem;
  }

  .badge {
    font-size: 0.62rem;
    letter-spacing: 0.02em;
    color: var(--ink);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0 0.3rem;
  }

  .name {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
    font-size: 0.68rem;
    color: var(--muted);
  }

  .selected .name {
    color: var(--ink);
  }
</style>
