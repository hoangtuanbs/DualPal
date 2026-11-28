<script lang="ts">
  import { onMount } from "svelte";
  import type { Entry, Location, SearchHit } from "./api";
  import { glyph } from "./fileTypes";
  import { formatBytes } from "./format";
  import { FindSession, hasCriteria, REMOTE_CONTENT_LIMIT } from "./find-session.svelte";

  /** Find files below the folder `location`: by name, text inside, size, date. */
  let {
    location,
    onopen,
    onclose,
    onfeed,
    session: given,
    showHidden = false,
  }: {
    location: Location;
    /** Show the entry's folder in the pane, with the entry selected. */
    onopen: (entry: Entry) => void;
    onclose: () => void;
    /** Show the session's results in the active pane. */
    onfeed?: (session: FindSession) => void;
    /** A search that outlives the dialog. Without one, closing stops the search. */
    session?: FindSession;
    /** Whether the pane shows hidden files: the first default of "Hidden items". */
    showHidden?: boolean;
  } = $props();

  // svelte-ignore state_referenced_locally
  const session = given ?? new FindSession();
  // svelte-ignore state_referenced_locally
  if (!session.ran && !hasCriteria(session.form)) session.form.includeHidden = showHidden;
  const form = session.form;

  let cursor = $state(0);
  let optionsOpen = $state(false);
  let field: HTMLInputElement | undefined = $state();
  let list: HTMLUListElement | undefined = $state();
  let timer: ReturnType<typeof setTimeout> | undefined;

  const sameBase = (a: Location | null, b: Location) => !!a && a.fs === b.fs && a.path === b.path;

  onMount(() => {
    field?.focus();
    field?.select();
    // A remembered search: show its results here, or run it again in a new folder.
    if (hasCriteria(form) && !sameBase(session.base, location)) void session.start(location);
    return () => {
      clearTimeout(timer);
      if (!given) session.dispose();
    };
  });

  function schedule() {
    clearTimeout(timer);
    cursor = 0;
    // Clear at once so old matches never sit under a new query.
    if (!hasCriteria(form)) {
      session.clear();
      return;
    }
    timer = setTimeout(() => session.start(location), form.content ? 400 : 200);
  }

  function stop() {
    session.stop();
  }

  function feed() {
    if (!onfeed || (!session.hits.length && !session.searching)) return;
    onfeed(session);
  }

  /** The folder of `hit`, relative to the folder being searched. */
  function where(hit: Entry) {
    const sep = location.path.includes("\\") && !location.path.includes("/") ? "\\" : "/";
    const parent = hit.path.slice(0, Math.max(hit.path.lastIndexOf(sep), 0));
    const rel = parent.startsWith(location.path) ? parent.slice(location.path.length).replace(/^[\\/]/, "") : parent;
    return rel || "(this folder)";
  }

  function move(delta: number) {
    if (!session.hits.length) return;
    cursor = Math.max(0, Math.min(session.hits.length - 1, cursor + delta));
    list?.children[cursor]?.scrollIntoView({ block: "nearest" });
  }

  function keydown(e: KeyboardEvent) {
    e.stopPropagation();
    const hits = session.hits;
    if (e.key === "Escape") {
      e.preventDefault();
      if (session.searching) stop();
      else onclose();
    } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      feed();
    } else if ((e.target as HTMLElement).tagName === "SELECT") {
      // Arrows and Enter belong to the drop-down.
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
    } else if (e.key === "Enter" && (e.target as HTMLElement).tagName !== "BUTTON" && hits[cursor]) {
      e.preventDefault();
      stop();
      onopen(hits[cursor]);
    }
  }

  const n = $derived(session.hits.length);
  const count = $derived(`${n} ${n === 1 ? "match" : "matches"}`);
  const scanned = $derived(`${session.folders.toLocaleString()} ${session.folders === 1 ? "folder" : "folders"}`);
  const read = $derived(session.files ? ` · ${session.files.toLocaleString()} ${session.files === 1 ? "file" : "files"} read` : "");
  const remote = $derived(location.fs !== "local");
  /** How many options differ from the defaults, shown on the Options button. */
  const changed = $derived(
    [
      form.nameRegex,
      form.contentRegex,
      !form.ignoreCase,
      form.includeBinary,
      form.includeHidden !== showHidden,
      form.kind !== "any",
      form.minSize.trim() !== "",
      form.maxSize.trim() !== "",
      form.date !== "any",
    ].filter(Boolean).length,
  );
  const nameOnly = $derived(!form.content && changed === 0);
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="scrim" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="find" role="dialog" aria-modal="true" aria-label="Find files" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={keydown}>
    <input
      bind:this={field}
      class="query"
      placeholder="Find files named…  (e.g. report or *.rs;*.toml)"
      aria-label="Find files named"
      spellcheck="false"
      autocomplete="off"
      bind:value={form.name}
      oninput={schedule}
    />
    <div class="second">
      <input
        class="content"
        placeholder="Containing text (optional)"
        aria-label="Containing text"
        spellcheck="false"
        autocomplete="off"
        bind:value={form.content}
        oninput={schedule}
      />
      <button class="disclose" aria-expanded={optionsOpen} aria-controls="find-options" onclick={() => (optionsOpen = !optionsOpen)}>
        {optionsOpen ? "▾" : "▸"} Options{changed ? ` (${changed})` : ""}
      </button>
    </div>
    {#if optionsOpen}
      <div class="options" id="find-options" role="group" aria-label="Search options">
        <div class="line">
          <label><input type="checkbox" bind:checked={form.ignoreCase} onchange={schedule} /> Ignore case</label>
          <label><input type="checkbox" bind:checked={form.nameRegex} onchange={schedule} /> Regex in name</label>
          <label><input type="checkbox" bind:checked={form.contentRegex} onchange={schedule} /> Regex in text</label>
        </div>
        <div class="line">
          <select aria-label="Kind" bind:value={form.kind} onchange={schedule}>
            <option value="any">Files and folders</option>
            <option value="files">Files only</option>
            <option value="folders">Folders only</option>
          </select>
          <label><input type="checkbox" bind:checked={form.includeHidden} onchange={schedule} /> Hidden items</label>
          <label><input type="checkbox" bind:checked={form.includeBinary} onchange={schedule} /> Search binary files</label>
        </div>
        <div class="line">
          <span class="caption">Size</span>
          <input class="size" aria-label="At least" placeholder="at least, e.g. 1 MB" bind:value={form.minSize} oninput={schedule} />
          <span class="caption">to</span>
          <input class="size" aria-label="At most" placeholder="at most" bind:value={form.maxSize} oninput={schedule} />
        </div>
        <div class="line">
          <span class="caption">Modified</span>
          <select aria-label="Modified" bind:value={form.date} onchange={schedule}>
            <option value="any">Any time</option>
            <option value="today">Today</option>
            <option value="7d">Last 7 days</option>
            <option value="30d">Last 30 days</option>
            <option value="newer">On or after…</option>
            <option value="older">Before…</option>
          </select>
          {#if form.date === "newer" || form.date === "older"}
            <input type="date" aria-label="Day" bind:value={form.day} onchange={schedule} />
          {/if}
        </div>
        {#if remote}
          <p class="note">On servers, files over {formatBytes(REMOTE_CONTENT_LIMIT)} are not searched for text.</p>
        {/if}
      </div>
    {/if}
    <div class="status" role="status" aria-live="polite">
      <span class="scope" title={location.path}>In {location.path}</span>
      <span class="spacer"></span>
      {#if session.error}
        <span class="error">{session.error}</span>
      {:else if session.searching}
        <span class="spinner" aria-hidden="true"></span>
        Searching… {count} · {scanned}{read}
        <button class="link" onclick={stop}>Stop</button>
      {:else if session.ran}
        {session.folders ? `${count} in ${scanned}` : count}{read}
        {#if session.truncated}· stopped early, more may match{/if}
        {#if session.skipped}· {session.skipped} {session.skipped === 1 ? "folder" : "folders"} could not be read{/if}
        {#if session.filesSkipped}· {session.filesSkipped} {session.filesSkipped === 1 ? "file" : "files"} could not be read{/if}
        {#if session.binary}· {session.binary} binary {session.binary === 1 ? "file" : "files"} skipped{/if}
        {#if session.tooLarge}· {session.tooLarge} {session.tooLarge === 1 ? "file" : "files"} over {formatBytes(REMOTE_CONTENT_LIMIT)} not searched{/if}
      {:else}
        Type part of a name. Enter opens the selected match.
      {/if}
      {#if onfeed && (n || session.searching)}
        <button class="link" title="Show the matches in the pane (Ctrl/⌘ Enter)" onclick={feed}>Show in pane</button>
      {/if}
    </div>
    {#if n}
      <ul class="hits" role="listbox" aria-label="Matches" bind:this={list}>
        {#each session.hits as hit, i (hit.path)}
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
            {#if (hit as SearchHit).snippet}
              {@const s = (hit as SearchHit).snippet!}
              <span class="snippet"><span class="line">{s.line}:</span> {s.text}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if session.ran && !session.searching && !session.error}
      <p class="empty">
        {nameOnly ? `No file or folder names contain “${form.name.trim()}”.` : "Nothing matches."}
      </p>
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
  .second {
    display: flex;
    align-items: center;
    border-bottom: 1px solid var(--border);
  }
  .content {
    flex: 1;
    min-width: 0;
    font: inherit;
    padding: 8px 14px;
    border: none;
    background: transparent;
    color: var(--text);
    outline: none;
  }
  .disclose {
    border: none;
    background: none;
    color: var(--muted);
    font: inherit;
    font-size: 12px;
    padding: 4px 14px;
    cursor: pointer;
    white-space: nowrap;
  }
  .disclose:hover,
  .disclose[aria-expanded="true"] {
    color: var(--text);
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 14px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .options .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 14px;
  }
  .options label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .options select,
  .options input:not([type="checkbox"]) {
    font: inherit;
    padding: 2px 6px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--pane-bg);
    color: var(--text);
  }
  .options .size {
    width: 9em;
  }
  .caption {
    color: var(--muted);
  }
  .note {
    margin: 0;
    color: var(--muted);
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
  .snippet {
    grid-column: 2 / -1;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 11.5px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .snippet .line {
    opacity: 0.7;
  }
  li.active .snippet {
    color: inherit;
    opacity: 0.85;
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
