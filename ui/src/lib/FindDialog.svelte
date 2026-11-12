<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorMessage, type Entry, type Location } from "./api";
  import { glyph } from "./fileTypes";

  /** Find files by name below the folder `location`. */
  let {
    location,
    onopen,
    onclose,
  }: {
    location: Location;
    /** Show the entry's folder in the pane, with the entry selected. */
    onopen: (entry: Entry) => void;
    onclose: () => void;
  } = $props();

  let query = $state("");
  /** Matches so far: filled by progress events while searching. */
  let hits = $state<Entry[]>([]);
  let folders = $state(0);
  let searching = $state(false);
  let truncated = $state(false);
  let skipped = $state(0);
  let error = $state("");
  let cursor = $state(0);
  let field: HTMLInputElement | undefined = $state();
  let list: HTMLUListElement | undefined = $state();
  // Each search gets a new request id; events and answers for older ones are ignored.
  let latest = 0;
  let running = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    field?.focus();
    const off = api.onFindProgress((p) => {
      if (p.request !== latest || !searching) return;
      if (p.hits.length) hits = [...hits, ...p.hits];
      folders = p.folders;
    });
    return () => {
      clearTimeout(timer);
      if (running) void api.cancelFindFiles(running);
      off.then((f) => f()).catch(() => {});
    };
  });

  function schedule() {
    clearTimeout(timer);
    const text = query.trim();
    // Clear at once so old matches never sit under a new query.
    if (!text) {
      void run("");
      return;
    }
    timer = setTimeout(() => run(text), 200);
  }

  async function run(text: string) {
    const id = ++latest;
    if (running) void api.cancelFindFiles(running);
    running = 0;
    error = "";
    hits = [];
    folders = 0;
    truncated = false;
    skipped = 0;
    cursor = 0;
    if (!text) {
      searching = false;
      return;
    }
    searching = true;
    running = id;
    try {
      const found = await api.findFiles(location, text, id);
      if (id !== latest) return;
      hits = found.hits;
      truncated = found.truncated;
      skipped = found.skipped;
      cursor = Math.min(cursor, Math.max(0, hits.length - 1));
    } catch (e) {
      if (id !== latest || (e as { kind?: string })?.kind === "cancelled") return;
      error = errorMessage(e);
    } finally {
      if (id === latest) {
        searching = false;
        running = 0;
      }
    }
  }

  function stop() {
    if (running) void api.cancelFindFiles(running);
  }

  /** The folder of `hit`, relative to the folder being searched. */
  function where(hit: Entry) {
    const sep = location.path.includes("\\") && !location.path.includes("/") ? "\\" : "/";
    const parent = hit.path.slice(0, Math.max(hit.path.lastIndexOf(sep), 0));
    const rel = parent.startsWith(location.path) ? parent.slice(location.path.length).replace(/^[\\/]/, "") : parent;
    return rel || "(this folder)";
  }

  function move(delta: number) {
    if (!hits.length) return;
    cursor = Math.max(0, Math.min(hits.length - 1, cursor + delta));
    list?.children[cursor]?.scrollIntoView({ block: "nearest" });
  }

  function keydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      if (searching) stop();
      else onclose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      move(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      move(-1);
    } else if (e.key === "PageDown") {
      e.preventDefault();
      move(10);
    } else if (e.key === "PageUp") {
      e.preventDefault();
      move(-10);
    } else if (e.key === "Enter" && hits[cursor]) {
      e.preventDefault();
      stop();
      onopen(hits[cursor]);
    }
  }

  const count = $derived(`${hits.length} ${hits.length === 1 ? "match" : "matches"}`);
  const scanned = $derived(`${folders.toLocaleString()} ${folders === 1 ? "folder" : "folders"}`);
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="scrim" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="find" role="dialog" aria-modal="true" aria-label="Find files" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={keydown}>
    <input
      bind:this={field}
      class="query"
      placeholder="Find files named…"
      aria-label="Find files named"
      spellcheck="false"
      autocomplete="off"
      bind:value={query}
      oninput={schedule}
    />
    <div class="status" role="status" aria-live="polite">
      <span class="scope" title={location.path}>In {location.path}</span>
      <span class="spacer"></span>
      {#if error}
        <span class="error">{error}</span>
      {:else if searching}
        <span class="spinner" aria-hidden="true"></span>
        Searching… {count} · {scanned}
        <button class="link" onclick={stop}>Stop</button>
      {:else if query.trim()}
        {folders ? `${count} in ${scanned}` : count}
        {#if truncated}· stopped early, more may match{/if}
        {#if skipped}· {skipped} {skipped === 1 ? "folder" : "folders"} could not be read{/if}
      {:else}
        Type part of a name. Enter opens the selected match.
      {/if}
    </div>
    {#if hits.length}
      <ul class="hits" role="listbox" aria-label="Matches" bind:this={list}>
        {#each hits as hit, i (hit.path)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li
            role="option"
            aria-selected={i === cursor}
            class:active={i === cursor}
            title={hit.path}
            onmousemove={() => (cursor = i)}
            onclick={() => (cursor = i)}
            ondblclick={() => {
              stop();
              onopen(hit);
            }}
          >
            <span class="icon" aria-hidden="true">{glyph(hit)}</span>
            <span class="name">{hit.name}</span>
            <span class="path">{where(hit)}</span>
          </li>
        {/each}
      </ul>
    {:else if query.trim() && !searching && !error}
      <p class="empty">No file or folder names contain “{query.trim()}”.</p>
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.2);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    z-index: 11;
  }
  .find {
    width: min(640px, calc(100vw - 32px));
    display: flex;
    flex-direction: column;
    background: var(--pane-bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 24px 60px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  .query {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-size: 15px;
    padding: 12px 14px;
    border: none;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    outline: none;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  .scope {
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .spacer {
    flex: 1;
  }
  .error {
    color: var(--danger);
    white-space: normal;
  }
  .link {
    border: none;
    background: none;
    padding: 0 2px;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .spinner {
    width: 10px;
    height: 10px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .hits {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 50vh;
    overflow-y: auto;
    border-top: 1px solid var(--border);
  }
  li {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) minmax(0, 1.2fr);
    align-items: center;
    gap: 8px;
    padding: 5px 10px;
    border-radius: 7px;
    cursor: default;
  }
  li.active {
    background: var(--selection);
    color: var(--selection-text);
  }
  .name,
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path {
    font-size: 12px;
    color: var(--muted);
    text-align: right;
  }
  li.active .path {
    color: inherit;
    opacity: 0.8;
  }
  .empty {
    margin: 0;
    padding: 10px 14px 14px;
    color: var(--muted);
    border-top: 1px solid var(--border);
  }
</style>
