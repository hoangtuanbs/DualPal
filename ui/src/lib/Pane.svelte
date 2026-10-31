<script lang="ts">
  import type { Entry } from "./api";
  import { formatBytes, formatDate } from "./format";
  import type { PaneState, SortKey } from "./pane.svelte";

  let { pane, active, onfocus }: { pane: PaneState; active: boolean; onfocus: () => void } = $props();

  let listEl: HTMLDivElement | undefined = $state();
  let crumbsEl: HTMLElement | undefined = $state();

  // Long paths: keep the deepest folders visible.
  $effect(() => {
    void crumbs;
    if (crumbsEl) crumbsEl.scrollLeft = crumbsEl.scrollWidth;
  });

  // Keep the cursor row in view when it moves via the keyboard.
  $effect(() => {
    const row = listEl?.querySelector<HTMLElement>(`[data-index="${pane.cursor}"]`);
    row?.scrollIntoView({ block: "nearest" });
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
    return sel ? `${sel} of ${items} selected` : items;
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<section class="pane" class:active onclick={onfocus}>
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

  <div class="columns">
    <button class="name" onclick={() => pane.setSort("name")}>Name {sortMark("name")}</button>
    <button class="size" onclick={() => pane.setSort("size")}>Size {sortMark("size")}</button>
    <button class="date" onclick={() => pane.setSort("modified")}>Date Modified {sortMark("modified")}</button>
  </div>

  <div class="list" bind:this={listEl} role="listbox" aria-multiselectable="true">
    {#if pane.error}
      <p class="message error">{pane.error}</p>
    {:else if pane.visible.length === 0 && !pane.loading}
      <p class="message">This folder is empty.</p>
    {/if}
    {#each pane.visible as entry, i (entry.path)}
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
          <span class="icon" aria-hidden="true">{entry.kind === "dir" ? "📁" : "📄"}</span>
          <span class="label">{entry.name}</span>
          {#if entry.symlink}<span class="badge" title="Symbolic link">↪</span>{/if}
        </span>
        <span class="size">{entry.kind === "dir" ? "—" : formatBytes(entry.size)}</span>
        <span class="date">{formatDate(entry.modifiedMs)}</span>
      </div>
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
  .pane.active .row.cursor {
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
  footer {
    padding: 4px 12px;
    font-size: 12px;
    color: var(--muted);
    border-top: 1px solid var(--border);
  }
</style>
