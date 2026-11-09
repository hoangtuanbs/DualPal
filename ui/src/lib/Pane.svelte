<script lang="ts">
  import type { Entry, Location } from "./api";
  import { glyph, thumbnailUrl } from "./fileTypes";
  import { formatBytes, formatDate, formatShortDate } from "./format";
  import type { PaneGroup, SortKey } from "./pane-state.svelte";
  import { fsNames } from "./fs-names.svelte";
  import { drag, dragging, fromKeyboard, type DropTarget } from "./drag.svelte";

  let {
    group,
    index,
    active,
    onfocus,
    oncontext,
    ondrop,
  }: {
    group: PaneGroup;
    /** 0 = left pane, 1 = right pane. */
    index: number;
    active: boolean;
    onfocus: () => void;
    /** Right-click at (x, y) on an entry, or on empty space (null). */
    oncontext: (x: number, y: number, entry: Entry | null) => void;
    /** Dragged items let go over a folder. */
    ondrop: (items: Entry[], from: Location, target: DropTarget, move: boolean) => void;
  } = $props();
  const pane = $derived(group.current);
  const side = $derived(index === 0 ? "left" : "right");

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
    // A server's root shows the server's name.
    const root = (pane.location && fsNames[pane.location.fs]) || "/";
    const out: { name: string; path: string }[] = windows ? [] : [{ name: root, path: "/" }];
    let acc = "";
    let parts = path.split(/[\\/]/).filter(Boolean);
    // A network share (\\server\share\, also WSL's \\wsl.localhost\Ubuntu\)
    // is one root crumb named after the share.
    if (path.startsWith("\\\\") && parts.length >= 2) {
      acc = `\\\\${parts[0]}\\${parts[1]}\\`;
      out.push({ name: parts[1], path: acc });
      parts = parts.slice(2);
    }
    for (const part of parts) {
      if (!windows) acc = `${acc}/${part}`;
      // Drive roots keep their trailing separator ("C:\").
      else acc = acc === "" ? `${part}\\` : `${acc.endsWith("\\") ? acc : acc + "\\"}${part}`;
      out.push({ name: part, path: acc });
    }
    return out;
  });

  // ---- drag and drop ------------------------------------------------------
  // Pointer events rather than HTML drag and drop, which the Windows webview
  // only delivers when the app does not take file drops from Explorer.

  /** Where a press started that may become a drag. */
  let press: { x: number; y: number; entry: Entry; index: number } | null = null;
  /** The item last clicked, which a Ctrl/⌘-click adds to the marked items. */
  let clicked: string | null = null;
  /** Swallows the click that ends a drag. */
  let justDragged = false;

  function pointerdown(e: PointerEvent, entry: Entry, i: number) {
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
    e.preventDefault(); // no text selection while dragging
    // That also keeps focus where it was, e.g. in the quick editor: move it
    // to the list so that the keys work on the files again.
    listEl?.focus({ preventScroll: true });
    press = { x: e.clientX, y: e.clientY, entry, index: i };
    window.addEventListener("pointermove", pointermove);
    window.addEventListener("pointerup", pointerup);
    window.addEventListener("pointercancel", cancelDrag);
    window.addEventListener("blur", cancelDrag);
  }

  function stopListening() {
    window.removeEventListener("pointermove", pointermove);
    window.removeEventListener("pointerup", pointerup);
    window.removeEventListener("pointercancel", cancelDrag);
    window.removeEventListener("blur", cancelDrag);
    press = null;
  }

  /** A press or drag that ends without a drop: Esc, focus lost, cancelled. */
  function cancelDrag() {
    const was = press !== null;
    stopListening();
    if (was && dragging() && drag.from === index) endDrag();
  }

  function pointermove(e: PointerEvent) {
    if (!press) return;
    if (!dragging()) {
      if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < 6) return;
      onfocus();
      // Dragging an unmarked item drags just that item.
      if (!pane.selected.has(press.entry.path)) {
        pane.selected.clear();
        pane.cursor = press.index;
      }
      drag.items = pane.targets;
      drag.from = index;
      drag.source = pane.location;
    }
    drag.x = e.clientX;
    drag.y = e.clientY;
    drag.move = e.shiftKey;
    drag.target = dropTarget(e.clientX, e.clientY);
  }

  function pointerup(e: PointerEvent) {
    stopListening();
    if (!dragging()) return;
    justDragged = true;
    setTimeout(() => (justDragged = false));
    const { items, source, target } = drag;
    endDrag();
    if (source && target) ondrop(items, source, target, e.shiftKey);
  }

  function endDrag() {
    drag.items = [];
    drag.target = null;
    drag.source = null;
    drag.from = -1;
  }

  /** The folder under (x, y): a folder row, or else the pane's own folder. */
  function dropTarget(x: number, y: number): DropTarget | null {
    const el = document.elementFromPoint(x, y);
    const paneEl = el?.closest<HTMLElement>("[data-pane]");
    const fs = paneEl?.dataset.fs;
    const path = paneEl?.dataset.path;
    if (!paneEl || !fs || path == null) return null;
    const p = Number(paneEl.dataset.pane);
    const dragged = new Set(drag.items.map((it) => it.path));
    const row = el?.closest<HTMLElement>("[data-dir]");
    const sameFs = drag.source?.fs === fs;
    if (row?.dataset.dir && !(sameFs && dragged.has(row.dataset.dir))) {
      return { pane: p, folder: { fs, path: row.dataset.dir }, name: row.dataset.name ?? "", row: row.dataset.dir };
    }
    // Dropping back into the folder the items came from does nothing.
    if (sameFs && drag.source?.path === path) return null;
    return { pane: p, folder: { fs, path }, name: paneEl.dataset.title || path, row: null };
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape" && dragging() && drag.from === index) {
      e.stopPropagation();
      e.preventDefault();
      cancelDrag();
    }
  }

  function contextmenu(e: MouseEvent, entry: Entry, i: number) {
    e.preventDefault();
    e.stopPropagation();
    onfocus();
    // Right-clicking an unmarked item acts on just that item.
    if (!pane.selected.has(entry.path)) pane.selected.clear();
    pane.cursor = i;
    oncontext(e.clientX, e.clientY, entry);
  }

  function click(e: MouseEvent, entry: Entry, index: number) {
    if (justDragged) return;
    onfocus();
    if (e.metaKey || e.ctrlKey) {
      // Like Explorer and Finder: the item clicked just before joins in.
      const prev = pane.current;
      if (pane.selected.size === 0 && prev && prev.path === clicked && prev.path !== entry.path) pane.selected.add(prev.path);
      pane.toggle(entry);
    } else if (e.shiftKey) {
      const [a, b] = [Math.min(pane.cursor, index), Math.max(pane.cursor, index)];
      for (const it of pane.visible.slice(a, b + 1)) pane.selected.add(it.path);
    } else {
      pane.selected.clear();
    }
    clicked = entry.path;
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

<svelte:window onkeydown={keydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<section
  class="pane"
  class:active
  class:drop-target={drag.target?.pane === index && !drag.target.row}
  data-pane={index}
  data-testid={`pane-${side}`}
  data-fs={pane.location?.fs}
  data-path={pane.location?.path}
  data-title={pane.title}
  onclick={onfocus}
>
  {#if group.tabs.length > 1}
    <div class="tabs" role="tablist">
      {#each group.tabs as tab, i}
        <div class="tab" class:selected={i === group.index} role="tab" aria-selected={i === group.index} tabindex="-1"
          data-testid={`pane-${side}-tab`}
          onclick={() => (group.index = i)} onkeydown={() => {}} onauxclick={(e) => e.button === 1 && group.closeTab(i)}
          title={tab.location?.path}>
          <span class="tab-title">{tab.title || "/"}</span>
          <button class="tab-close" aria-label="Close tab" onclick={(e) => { e.stopPropagation(); group.closeTab(i); }}>×</button>
        </div>
      {/each}
      <button class="tab-add" title="New tab (Ctrl/⌘ T)" aria-label="New tab" data-testid={`pane-${side}-tab-add`} onclick={() => group.newTab()}>+</button>
    </div>
  {/if}
  <header>
    <div class="nav">
      <button title="Back" disabled={!pane.canBack} onclick={() => pane.back()}>‹</button>
      <button title="Forward" disabled={!pane.canForward} onclick={() => pane.forward()}>›</button>
      <button title="Enclosing folder (Backspace)" onclick={() => pane.up()}>↑</button>
    </div>
    <nav class="crumbs" aria-label="Path" data-testid={`pane-${side}-crumbs`} bind:this={crumbsEl}>
      {#each crumbs as c, i (c.path)}
        {#if i > 0}<span class="sep">›</span>{/if}
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
    data-testid={`pane-${side}-list`}
    aria-multiselectable="true"
    tabindex="-1"
    oncontextmenu={(e) => {
      if (fromKeyboard(e)) return; // the app opens the menu for the cursor row
      e.preventDefault();
      onfocus();
      oncontext(e.clientX, e.clientY, null);
    }}
  >
    {#if pane.error}
      <p class="message error" data-testid={`pane-${side}-error`}>{pane.error}</p>
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
          class:drop-target={drag.target?.pane === index && drag.target.row === entry.path}
          data-index={i}
          data-dir={entry.kind === "dir" ? entry.path : undefined}
          data-name={entry.name}
          data-testid={`pane-${side}-row`}
          role="option"
          aria-selected={pane.selected.has(entry.path)}
          tabindex="-1"
          title={entry.modifiedMs != null ? `${entry.name} — modified ${formatDate(entry.modifiedMs)}` : entry.name}
          onclick={(e) => click(e, entry, i)}
          ondblclick={() => pane.activate(entry)}
          onpointerdown={(e) => pointerdown(e, entry, i)}
          oncontextmenu={(e) => contextmenu(e, entry, i)}
        >
          <span class="name">
            <span class="icon" aria-hidden="true">{glyph(entry)}</span>
            <span class="label">{entry.name}</span>
            {#if entry.symlink}<span class="badge" title="Symbolic link">↪</span>{/if}
          </span>
          <span class="size">{entry.kind === "dir" ? "—" : formatBytes(entry.size)}</span>
          <span class="date">{formatShortDate(entry.modifiedMs)}</span>
        </div>
      {:else}
        {@const thumb = thumbnailUrl(pane.location, entry)}
        <div
          class="cell"
          class:cursor={i === pane.cursor}
          class:selected={pane.selected.has(entry.path)}
          class:hidden-file={entry.hidden}
          class:drop-target={drag.target?.pane === index && drag.target.row === entry.path}
          data-index={i}
          data-dir={entry.kind === "dir" ? entry.path : undefined}
          data-name={entry.name}
          role="option"
          aria-selected={pane.selected.has(entry.path)}
          tabindex="-1"
          title={entry.name}
          onclick={(e) => click(e, entry, i)}
          ondblclick={() => pane.activate(entry)}
          onpointerdown={(e) => pointerdown(e, entry, i)}
          oncontextmenu={(e) => contextmenu(e, entry, i)}
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

  <footer data-testid={`pane-${side}-status`}>{summary}</footer>
</section>

<style>
  .pane {
    container-type: inline-size;
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
  /* Narrow panes (e.g. with the Info panel open): give the space to names. */
  @container (max-width: 560px) {
    .columns,
    .row {
      grid-template-columns: minmax(0, 1fr) 72px 120px;
    }
  }
  @container (max-width: 430px) {
    .columns,
    .row {
      grid-template-columns: minmax(0, 1fr) 72px;
    }
    .columns .date,
    .row .date {
      display: none;
    }
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
  .pane.drop-target .list {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    background: color-mix(in srgb, var(--accent) 7%, transparent);
  }
  .row.drop-target,
  .cell.drop-target {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    border-radius: 4px;
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
