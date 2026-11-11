<script lang="ts">
  import { api, type Entry, type Location, type SearchResult } from "./api";

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
  let result = $state<SearchResult | null>(null);
  let searching = $state(false);
  let error = $state("");
  let cursor = $state(0);
  // Each search gets a new request id; a late answer for an old one is ignored.
  let latest = 0;
  let previous = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function schedule() {
    clearTimeout(timer);
    const text = query.trim();
    timer = setTimeout(() => run(text), 250);
  }

  async function run(text: string) {
    const id = ++latest;
    if (previous) void api.cancelFindFiles(previous);
    previous = id;
    error = "";
    if (!text) {
      result = null;
      searching = false;
      return;
    }
    searching = true;
    try {
      const found = await api.findFiles(location, text, id);
      if (id !== latest) return;
      result = found;
      cursor = 0;
    } catch (e) {
      if (id !== latest) return;
      result = null;
      error = String(e);
    } finally {
      if (id === latest) searching = false;
    }
  }


  function keydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      onclose();
    } else if (e.key === "ArrowDown" && result) {
      e.preventDefault();
      cursor = Math.min(cursor + 1, result.hits.length - 1);
    } else if (e.key === "ArrowUp" && result) {
      e.preventDefault();
      cursor = Math.max(cursor - 1, 0);
    } else if (e.key === "Enter" && result?.hits[cursor]) {
      e.preventDefault();
      onopen(result.hits[cursor]);
    }
  }

  $effect(() => () => {
    clearTimeout(timer);
    if (previous) void api.cancelFindFiles(previous);
  });
</script>

<div class="scrim" role="presentation" onclick={onclose}>
  <div class="find" role="dialog" aria-modal="true" aria-label="Find files" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={keydown}>
    <input
      class="query"
      placeholder="Find files named…"
      aria-label="Find files named"
      bind:value={query}
      oninput={schedule}
    />
    <div class="status" role="status" aria-live="polite">
      {#if error}
        <span class="error">{error}</span>
      {:else if searching}
        Searching…
      {:else if result}
        {result.hits.length} {result.hits.length === 1 ? "match" : "matches"}
        {#if result.truncated}· first {result.hits.length} shown{/if}
        {#if result.skipped}· {result.skipped} folders could not be read{/if}
      {:else}
        Type part of a name. Enter opens the selected match.
      {/if}
    </div>
    <ul class="hits" role="listbox" aria-label="Matches">
      {#each result?.hits ?? [] as hit, i (hit.path)}
        <li
          role="option"
          aria-selected={i === cursor}
          class:active={i === cursor}
          onmouseenter={() => (cursor = i)}
          onclick={() => onopen(hit)}
          onkeydown={(e) => e.key === "Enter" && onopen(hit)}
          ondblclick={() => onopen(hit)}
        >
          <span class="name">{hit.name}</span>
          <span class="path">{hit.path}</span>
        </li>
      {/each}
    </ul>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.25);
    display: flex;
    justify-content: center;
    padding-top: 12vh;
    z-index: 50;
  }
  .find {
    width: min(640px, calc(100vw - 32px));
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    background: var(--surface, #fff);
    color: var(--text, #111);
    border: 1px solid var(--border, #ccc);
    border-radius: 8px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.25);
    overflow: hidden;
  }
  .query {
    font: inherit;
    padding: 10px 12px;
    border: none;
    border-bottom: 1px solid var(--border, #ccc);
    background: transparent;
    color: inherit;
  }
  .status {
    padding: 6px 12px;
    font-size: 0.85em;
    color: var(--muted, #555);
  }
  .error {
    color: var(--danger, #b00020);
  }
  .hits {
    list-style: none;
    margin: 0;
    padding: 0 0 6px;
    overflow: auto;
  }
  li {
    display: flex;
    flex-direction: column;
    padding: 4px 12px;
    cursor: pointer;
  }
  li.active {
    background: var(--accent-soft, #e6f0ff);
  }
  .path {
    font-size: 0.8em;
    color: var(--muted, #555);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
