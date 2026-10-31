<script lang="ts">
  import type { Favorite, Location, Volume } from "./api";
  import { formatBytes } from "./format";

  let {
    volumes,
    favorites,
    current,
    onopen,
    onremovefavorite,
    onreorder,
  }: {
    volumes: Volume[];
    favorites: Favorite[];
    /** Location of the active pane, to highlight the matching item. */
    current: Location | null;
    onopen: (location: Location) => void;
    onremovefavorite: (location: Location) => void;
    onreorder: (from: number, to: number) => void;
  } = $props();

  const ICONS: Record<string, string> = {
    home: "🏠",
    desktop: "🖥️",
    documents: "📄",
    downloads: "⬇️",
    pictures: "🖼️",
    music: "🎵",
    movies: "🎬",
    applications: "🧩",
    folder: "📁",
  };

  let dragFrom: number | null = $state(null);
  let dragOver: number | null = $state(null);

  function isCurrent(loc: Location) {
    return !!current && current.fs === loc.fs && current.path === loc.path;
  }

  function drop(to: number) {
    if (dragFrom !== null && dragFrom !== to) onreorder(dragFrom, to);
    dragFrom = dragOver = null;
  }
</script>

<aside class="sidebar">
  <h3>Devices</h3>
  {#each volumes as v (v.location.path)}
    <button class="item" class:current={isCurrent(v.location)} onclick={() => onopen(v.location)} title={v.location.path}>
      <span class="icon" aria-hidden="true">{v.removable ? "💽" : "🖴"}</span>
      <span class="name">{v.name}</span>
      <span class="meta">{formatBytes(v.availableBytes)}</span>
    </button>
  {/each}

  <h3>Favorites</h3>
  {#each favorites as f, i (f.location.path)}
    <div
      class="item"
      class:current={isCurrent(f.location)}
      class:drag-over={dragOver === i}
      role="button"
      tabindex="0"
      title={f.location.path}
      draggable="true"
      ondragstart={() => (dragFrom = i)}
      ondragover={(e) => {
        e.preventDefault();
        dragOver = i;
      }}
      ondragleave={() => (dragOver = null)}
      ondrop={() => drop(i)}
      onclick={() => onopen(f.location)}
      onkeydown={(e) => e.key === "Enter" && onopen(f.location)}
    >
      <span class="icon" aria-hidden="true">{ICONS[f.icon] ?? "📁"}</span>
      <span class="name">{f.name}</span>
      <button
        class="remove"
        title="Remove from Favorites"
        aria-label="Remove {f.name} from Favorites"
        onclick={(e) => {
          e.stopPropagation();
          onremovefavorite(f.location);
        }}>×</button
      >
    </div>
  {/each}
  {#if favorites.length === 0}
    <p class="hint">Add folders with ★ in the toolbar.</p>
  {/if}
</aside>

<style>
  .sidebar {
    background: var(--sidebar);
    border-right: 1px solid var(--border);
    overflow-y: auto;
    padding: 6px 8px 12px;
    min-height: 0;
  }
  h3 {
    margin: 14px 8px 4px;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
  }
  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    box-sizing: border-box;
    padding: 4px 8px;
    border-radius: 6px;
    text-align: left;
    cursor: default;
    outline: none;
  }
  .item:hover {
    background: var(--control);
  }
  .item.current {
    background: var(--control-hover);
    font-weight: 500;
  }
  .item.drag-over {
    box-shadow: inset 0 2px 0 var(--accent);
  }
  .item:focus-visible {
    outline: 2px solid var(--accent);
  }
  .icon {
    width: 18px;
    text-align: center;
    flex-shrink: 0;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
  }
  .remove {
    visibility: hidden;
    color: var(--muted);
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
  }
  .item:hover .remove {
    visibility: visible;
  }
  .hint {
    margin: 4px 8px;
    font-size: 12px;
    color: var(--muted);
  }
</style>
