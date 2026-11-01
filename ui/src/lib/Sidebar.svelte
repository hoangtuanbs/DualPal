<script lang="ts">
  import type { ConnectionView, Favorite, Location, SyncFavorite, Volume } from "./api";
  import { formatBytes } from "./format";

  let {
    volumes,
    favorites,
    current,
    onopen,
    onremovefavorite,
    onreorder,
    connections,
    onconnect,
    onaddserver,
    oneditserver,
    ondisconnect,
    onremoveserver,
    onremoveremotefavorite,
    syncs,
    onopensync,
    onremovesync,
  }: {
    volumes: Volume[];
    favorites: Favorite[];
    /** Location of the active pane, to highlight the matching item. */
    current: Location | null;
    onopen: (location: Location) => void;
    onremovefavorite: (location: Location) => void;
    onreorder: (from: number, to: number) => void;
    connections: ConnectionView[];
    /** Connect if needed and open `path` (or the start folder). */
    onconnect: (c: ConnectionView, path?: string) => void;
    onaddserver: () => void;
    oneditserver: (c: ConnectionView) => void;
    ondisconnect: (c: ConnectionView) => void;
    onremoveserver: (c: ConnectionView) => void;
    onremoveremotefavorite: (c: ConnectionView, path: string) => void;
    /** Saved folder pairs to compare and sync. */
    syncs: SyncFavorite[];
    onopensync: (s: SyncFavorite) => void;
    onremovesync: (s: SyncFavorite) => void;
  } = $props();

  const MODE: Record<string, string> = { update: "→", mirror: "⇥", twoWay: "⇄" };

  /** Connections whose favorites are folded away. */
  let collapsed = $state<Record<string, boolean>>({});

  function serverIcon(c: ConnectionView) {
    return c.endpoint.type === "s3" ? "🪣" : "🖧";
  }

  function serverTitle(c: ConnectionView) {
    const e = c.endpoint;
    return e.type === "sftp" ? `${e.user}@${e.host}${e.port !== 22 ? `:${e.port}` : ""}` : `s3://${e.bucket}`;
  }

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
  {#each volumes.filter((v) => !v.wsl) as v (v.location.path)}
    <button class="item" class:current={isCurrent(v.location)} onclick={() => onopen(v.location)} title={v.location.path}>
      <span class="icon" aria-hidden="true">{v.removable ? "💽" : "🖴"}</span>
      <span class="name">{v.name}</span>
      <span class="meta">{formatBytes(v.availableBytes)}</span>
    </button>
  {/each}

  {#if volumes.some((v) => v.wsl)}
    <h3>Linux (WSL)</h3>
    {#each volumes.filter((v) => v.wsl) as v (v.location.path)}
      <button
        class="item"
        class:current={isCurrent(v.location)}
        onclick={() => onopen(v.location)}
        title={`${v.location.path}\nOpening a stopped distribution starts it, which can take a few seconds.`}
      >
        <span class="icon" aria-hidden="true">🐧</span>
        <span class="name">{v.name}</span>
      </button>
    {/each}
  {/if}

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

  <h3 class="with-action">
    Servers
    <button class="add" title="Add an SFTP or S3 server" aria-label="Add server" onclick={onaddserver}>+</button>
  </h3>
  {#each connections as c (c.id)}
    <div
      class="item"
      class:current={current?.fs === c.fsId}
      role="button"
      tabindex="0"
      title={serverTitle(c)}
      onclick={() => onconnect(c)}
      onkeydown={(e) => e.key === "Enter" && onconnect(c)}
    >
      {#if c.favorites.length > 0}
        <button
          class="disclose"
          aria-label={collapsed[c.id] ? "Show favorites" : "Hide favorites"}
          aria-expanded={!collapsed[c.id]}
          onclick={(e) => {
            e.stopPropagation();
            collapsed[c.id] = !collapsed[c.id];
          }}>{collapsed[c.id] ? "▸" : "▾"}</button
        >
      {/if}
      <span class="icon" aria-hidden="true">{serverIcon(c)}</span>
      <span class="name">{c.name}</span>
      {#if c.connected}<span class="dot" title="Connected" aria-label="Connected"></span>{/if}
      <span class="actions">
        <button
          title="Edit server"
          aria-label="Edit {c.name}"
          onclick={(e) => {
            e.stopPropagation();
            oneditserver(c);
          }}>✎</button
        >
        {#if c.connected}
          <button
            title="Disconnect"
            aria-label="Disconnect {c.name}"
            onclick={(e) => {
              e.stopPropagation();
              ondisconnect(c);
            }}>⏏</button
          >
        {:else}
          <button
            title="Remove server"
            aria-label="Remove {c.name}"
            onclick={(e) => {
              e.stopPropagation();
              onremoveserver(c);
            }}>×</button
          >
        {/if}
      </span>
    </div>
    {#if !collapsed[c.id]}
      {#each c.favorites as f (f.path)}
        <div
          class="item nested"
          class:current={current?.fs === c.fsId && current.path === f.path}
          role="button"
          tabindex="0"
          title="{serverTitle(c)}:{f.path}"
          onclick={() => onconnect(c, f.path)}
          onkeydown={(e) => e.key === "Enter" && onconnect(c, f.path)}
        >
          <span class="icon" aria-hidden="true">📁</span>
          <span class="name">{f.name}</span>
          <button
            class="remove"
            title="Remove from this server's favorites"
            aria-label="Remove {f.name}"
            onclick={(e) => {
              e.stopPropagation();
              onremoveremotefavorite(c, f.path);
            }}>×</button
          >
        </div>
      {/each}
    {/if}
  {/each}
  {#if connections.length === 0}
    <p class="hint">Connect to SFTP servers and S3 buckets with +.</p>
  {/if}

  {#if syncs.length > 0}
    <h3>Syncs</h3>
    {#each syncs as s (s.id)}
      <div
        class="item"
        role="button"
        tabindex="0"
        title="{s.left.path} {MODE[s.mode]} {s.right.path}"
        onclick={() => onopensync(s)}
        onkeydown={(e) => e.key === "Enter" && onopensync(s)}
      >
        <span class="icon" aria-hidden="true">{MODE[s.mode]}</span>
        <span class="name">{s.name}</span>
        <button
          class="remove"
          title="Remove this sync favorite"
          aria-label="Remove {s.name}"
          onclick={(e) => {
            e.stopPropagation();
            onremovesync(s);
          }}>×</button
        >
      </div>
    {/each}
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
  .with-action {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .add {
    font-size: 15px;
    line-height: 1;
    padding: 0 4px;
    color: var(--muted);
    border-radius: 4px;
  }
  .add:hover {
    background: var(--control);
    color: var(--text);
  }
  .nested {
    padding-left: 28px;
  }
  .disclose {
    width: 12px;
    margin-left: -10px;
    margin-right: -6px;
    padding: 0;
    font-size: 9px;
    color: var(--muted);
    flex-shrink: 0;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--success);
    flex-shrink: 0;
  }
  .actions {
    display: none;
    gap: 2px;
  }
  .item:hover .actions {
    display: flex;
  }
  .actions button {
    color: var(--muted);
    padding: 0 3px;
    font-size: 13px;
    line-height: 1;
  }
  .actions button:hover,
  .remove:hover {
    color: var(--text);
  }
  .hint {
    margin: 4px 8px;
    font-size: 12px;
    color: var(--muted);
  }
</style>
