<script lang="ts">
  import type { Entry } from "./api";
  import { glyph, thumbnailUrl } from "./fileTypes";
  import { formatBytes, formatDate } from "./format";
  import type { PaneGroup, SortKey } from "./pane-state.svelte";

  let { group, active, onfocus }: { group: PaneGroup; active: boolean; onfocus: () => void } = $props();
  const pane = $derived(group.current);

  /** Icon grid cell width incl. gap, in px. */
  const CELL = 112;
  let listWidth = $state(0);
  $effect(() => {
    if (pane.view === "icons") pane.columns = Math.max(1, Math.floor((listWidth - 16) / CELL));
  });

  let listEl: HTMLDivElement | undefined = $state();
  let crumbsEl: HTMLElement | undefined = $state();

  // Long paths: keep the deepest folders visible.
  $effect(() => {
    void crumbs;
    if (crumbsEl) crumbsEl.scrollLeft = crumbsEl.scrollWidth;
  });

  // Keep the cursor row in view when it moves via the keyboard.
  // Adjusts only the list's own scrollTop: scrollIntoView misbehaves in
  // WebKit (macOS) while the list is still being laid out.
  $effect(() => {
    const list = listEl;
    const row = list?.querySelector<HTMLElement>(`[data-index="${pane.cursor}"]`);
    if (!list || !row || list.clientHeight === 0) return;
    const top = row.offsetTop; // .list is position: relative
    const bottom = top + row.offsetHeight;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight;
  });

  const crumbs = $derived.by(() => {
    const path = pane.location?.path ?? "";
    const windows = /^[A-Za-z]:/.test(path) || path.startsWith("\\\\");
    const out: { name: string; path: string }[] = windows ? [] : [{ name: "/", path: "/" }];
    let acc = "";
    path
      .split(/[\\/]/)
      .filter(Boolean)
      .forEach((part, i) => {
        if (!windows) acc = `${acc}/${part}`;
        // Drive roots keep their trailing separator ("C:\").
        else acc = i === 0 ? `${part}\\` : `${acc.endsWith("\\") ? acc : acc + "\\"}${part}`;
        out.push({ name: part, path: acc });
      });
    return out;
  });

  function click(e: MouseEvent, entry: Entry, index: number) {
    onfocus();
    if (e.metaKey || e.ctrlKey) {
      pane.toggle(entry);
    } else if (e.shiftKey) {
      const [a, b] = [Math.min(pane.cursor, index), Math.max(pane.cursor, index)];
      for (const it of pane.visible.slice(a, b + 1)) pane.selected.add(it.path);
    } else {
      pane.selected.clear();
    }
    pane.cursor = index;
  }

  function sortMark(key: SortKey) {
    return pane.sortKey === key ? (pane.sortAsc ? "▲" : "▼") : "";
  }

  const summary = $derived.by(() => {
    const n = pane.visible.length;
    const sel = pane.selected.size;
    const items = `${n} ${n === 1 ? "item" : "items"}`;
    const base = sel ? `${sel} of ${items} selected` : items;
    return pane.freeBytes != null ? `${base}, ${formatBytes(pane.freeBytes)} available` : base;
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<section class="pane" class:active onclick={onfocus}>
  {#if group.tabs.length > 1}
    <div class="tabs" role="tablist">
      {#each group.tabs as tab, i}
        <div class="tab" class:selected={i === group.index} role="tab" aria-selected={i === group.index} tabindex="-1"
          onclick={() => (group.index = i)} onkeydown={() => {}} onauxclick={(e) => e.button === 1 && group.closeTab(i)}
          title={tab.location?.path}>
          <span class="tab-title">{tab.title || "/"}</span>
          <button class="tab-close" aria-label="Close tab" onclick={(e) => { e.stopPropagation(); group.closeTab(i); }}>×</button>
        </div>
      {/each}
      <button class="tab-add" title="New tab (Ctrl/⌘ T)" aria-label="New tab" onclick={() => group.newTab()}>+</button>
    </div>
  {/if}
  <header>
    <div class="nav">
      <button title="Back" disabled={!pane.canBack} onclick={() => pane.back()}>‹</button>
      <button title="Forward" disabled={!pane.canForward} onclick={() => pane.forward()}>›</button>
      <button title="Enclosing folder (Backspace)" onclick={() => pane.up()}>↑</button>
    </div>
    <nav class="crumbs" aria-label="Path" bind:this={crumbsEl}>
      {#each crumbs as c, i (c.path)}
        {#if i > 0 && c.name !== "/"}<span class="sep">›</span>{/if}
        <button
          class="crumb"
          onclick={() => pane.location && pane.open({ fs: pane.location.fs, path: c.path })}>{c.name}</button
        >
      {/each}
    </nav>
  </header>

  {#if pane.view === "list"}
    <div class="columns">
      <button class="name" onclick={() => pane.setSort("name")}>Name {sortMark("name")}</button>
      <button class="size" onclick={() => pane.setSort("size")}>Size {sortMark("size")}</button>
      <button class="date" onclick={() => pane.setSort("modified")}>Date Modified {sortMark("modified")}</button>
    </div>
  {/if}

  <div
    class="list"
    class:grid={pane.view === "icons"}
    style:--cols={pane.columns}
    bind:this={listEl}
    bind:clientWidth={listWidth}
    role="listbox"
    aria-multiselectable="true"
  >
    {#if pane.error}
      <p class="message error">{pane.error}</p>
    {:else if pane.visible.length === 0 && !pane.loading}
      <p class="message">This folder is empty.</p>
    {/if}
    {#each pane.visible as entry, i (entry.path)}
      {#if pane.view === "list"}
        <div
          class="row"
          class:cursor={i === pane.cursor}
          class:selected={pane.selected.has(entry.path)}
          class:hidden-file={entry.hidden}
          data-index={i}
          role="option"
          aria-selected={pane.selected.has(entry.path)}
          tabindex="-1"
          onclick={(e) => click(e, entry, i)}
          ondblclick={() => pane.activate(entry)}
        >
          <span class="name">
            <span class="icon" aria-hidden="true">{glyph(entry)}</span>
            <span class="label">{entry.name}</span>
            {#if entry.symlink}<span class="badge" title="Symbolic link">↪</span>{/if}
          </span>
          <span class="size">{entry.kind === "dir" ? "—" : formatBytes(entry.size)}</span>
          <span class="date">{formatDate(entry.modifiedMs)}</span>
        </div>
      {:else}
        {@const thumb = thumbnailUrl(pane.location, entry)}
        <div
          class="cell"
          class:cursor={i === pane.cursor}
          class:selected={pane.selected.has(entry.path)}
          class:hidden-file={entry.hidden}
          data-index={i}
          role="option"
          aria-selected={pane.selected.has(entry.path)}
          tabindex="-1"
          title={entry.name}
          onclick={(e) => click(e, entry, i)}
          ondblclick={() => pane.activate(entry)}
        >
          <div class="thumb">
            {#if thumb}
              <img src={thumb} alt="" loading="lazy" decoding="async" draggable="false" />
            {:else}
              <span class="big-icon" aria-hidden="true">{glyph(entry)}</span>
            {/if}
          </div>
          <span class="cell-label">{entry.name}</span>
        </div>
      {/if}
    {/each}
  </div>

  <footer>{summary}</footer>
</section>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--pane-bg);
    border-top: 2px solid transparent;
  }
  .pane.active {
    border-top-color: var(--accent);
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
  }
  .nav {
    display: flex;
    gap: 2px;
  }
  .nav button {
    width: 26px;
    height: 24px;
    border-radius: 6px;
    font-size: 15px;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    overflow-x: auto;
    scrollbar-width: none;
    white-space: nowrap;
    min-width: 0;
  }
  .crumbs::-webkit-scrollbar {
    display: none;
  }
  .crumb {
    flex-shrink: 0;
    padding: 2px 4px;
    border-radius: 4px;
    color: var(--text);
    font-weight: 500;
  }
  .sep {
    color: var(--muted);
  }
  .columns,
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 90px 170px;
    gap: 8px;
    padding: 0 12px;
  }
  .columns {
    border-bottom: 1px solid var(--border);
  }
  .columns button {
    text-align: left;
    padding: 5px 0;
    font-size: 12px;
    color: var(--muted);
    font-weight: 600;
  }
  .columns .size,
  .row .size {
    text-align: right;
  }
  .list {
    position: relative;
    flex: 1;
    overflow: auto;
    outline: none;
  }
  .row {
    height: 26px;
    align-items: center;
    cursor: default;
    user-select: none;
    border-radius: 5px;
    margin: 0 4px;
  }
  .row:nth-child(even) {
    background: var(--stripe);
  }
  .row.selected {
    background: var(--selection);
    color: var(--selection-text);
  }
  .pane.active .row.cursor,
  .pane.active .row.cursor:focus {
    outline: 1px solid var(--accent);
    outline-offset: -1px;
  }
  .row.hidden-file {
    opacity: 0.55;
  }
  .row .name {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    color: var(--muted);
    font-size: 11px;
  }
  .row .size,
  .row .date {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
  }
  .row.selected .size,
  .row.selected .date {
    color: inherit;
  }
  .message {
    padding: 16px;
    color: var(--muted);
  }
  .message.error {
    color: var(--danger);
  }
  .tabs {
    display: flex;
    gap: 1px;
    padding: 4px 6px 0;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 80px;
    max-width: 180px;
    padding: 4px 6px 4px 10px;
    border-radius: 6px 6px 0 0;
    color: var(--muted);
    cursor: default;
  }
  .tab.selected {
    background: var(--pane-bg);
    color: var(--text);
    font-weight: 500;
  }
  .tab-title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tab-close {
    visibility: hidden;
    color: var(--muted);
    padding: 0 3px;
  }
  .tab:hover .tab-close,
  .tab.selected .tab-close {
    visibility: visible;
  }
  .tab-add {
    padding: 2px 10px;
    color: var(--muted);
    font-size: 15px;
  }
  .list.grid {
    display: grid;
    grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
    grid-auto-rows: 124px;
    gap: 4px;
    padding: 8px;
    align-content: start;
  }
  .cell {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 8px 4px 6px;
    border-radius: 8px;
    min-width: 0;
    user-select: none;
    cursor: default;
  }
  .thumb {
    width: 72px;
    height: 64px;
    display: grid;
    place-items: center;
  }
  .thumb img {
    max-width: 72px;
    max-height: 64px;
    object-fit: contain;
    border-radius: 4px;
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.25);
  }
  .big-icon {
    font-size: 46px;
    line-height: 1;
  }
  .cell-label {
    max-width: 100%;
    padding: 1px 5px;
    border-radius: 4px;
    font-size: 12px;
    text-align: center;
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow-wrap: anywhere;
  }
  .cell.selected .thumb {
    background: var(--stripe);
    border-radius: 8px;
  }
  .cell.selected .cell-label {
    background: var(--selection);
    color: var(--selection-text);
  }
  .pane.active .cell.cursor,
  .pane.active .cell.cursor:focus {
    outline: 1px solid var(--accent);
    outline-offset: -1px;
  }
  /* The keyboard cursor is the focus indicator; hide the browser's. */
  .row:focus,
  .cell:focus {
    outline: none;
  }
  .cell.hidden-file {
    opacity: 0.55;
  }
  footer {
    padding: 4px 12px;
    font-size: 12px;
    color: var(--muted);
    border-top: 1px solid var(--border);
  }
</style>
