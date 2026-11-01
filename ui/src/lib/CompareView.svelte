<script lang="ts">
  import { onMount, untrack } from "svelte";
  import {
    api,
    errorMessage,
    defaultCompareOptions,
    type CompareBy,
    type CompareOptions,
    type DiffItem,
    type Location,
    type SyncAction,
    type SyncFavorite,
    type SyncMode,
    type SyncPreview,
    type SyncStep,
  } from "./api";
  import { formatBytes, formatShortDate } from "./format";

  /** Compare two folders side by side and sync them. */
  let {
    left: initialLeft,
    right: initialRight,
    favorite = null,
    nameOf,
    onclose,
    onsavefavorite,
    oncomparefiles,
  }: {
    left: Location;
    right: Location;
    /** The sync favorite being shown, if any: its mode and options are used. */
    favorite?: SyncFavorite | null;
    /** Display name of a file system id. */
    nameOf: (fs: string) => string;
    onclose: () => void;
    /** Save (or update) a sync favorite from the current settings. */
    onsavefavorite: (fav: SyncFavorite) => Promise<SyncFavorite | null>;
    oncomparefiles: (left: Location, right: Location) => Promise<void>;
  } = $props();

  // svelte-ignore state_referenced_locally
  let left = $state(initialLeft);
  // svelte-ignore state_referenced_locally
  let right = $state(initialRight);
  // svelte-ignore state_referenced_locally
  let fav = $state<SyncFavorite | null>(favorite);
  // svelte-ignore state_referenced_locally
  let mode = $state<SyncMode>(favorite?.mode ?? "update");
  // svelte-ignore state_referenced_locally
  let options = $state<CompareOptions>(favorite?.options ?? defaultCompareOptions());
  // svelte-ignore state_referenced_locally
  let excludeText = $state(options.exclude.join("; "));

  let preview = $state<SyncPreview | null>(null);
  let loading = $state(false);
  let progress = $state<{ folders: number; current: string } | null>(null);
  let error = $state<string | null>(null);
  let filter = $state<"changes" | "all" | "conflicts">("changes");
  let overrides = $state<Record<string, SyncAction>>({});
  let selected = $state<string | null>(null);
  let collapsed = $state<Record<string, boolean>>({});
  /** Operation id of the sync that is running. */
  let syncing = $state<string | null>(null);
  let confirmDeletes = $state(false);
  let naming = $state<string | null>(null);

  const steps = $derived(new Map((preview?.steps ?? []).map((s) => [s.rel, s])));

  function actionOf(s: SyncStep): SyncAction {
    return overrides[s.rel] ?? s.action;
  }

  /** The step that covers `rel`: its own, or a folder step above it. */
  function coveringStep(rel: string): SyncStep | undefined {
    let r = rel;
    for (;;) {
      const i = r.lastIndexOf("/");
      if (i < 0) return undefined;
      r = r.slice(0, i);
      const s = steps.get(r);
      if (s) return s;
    }
  }

  function hiddenByCollapse(rel: string) {
    let r = rel;
    for (;;) {
      const i = r.lastIndexOf("/");
      if (i < 0) return false;
      r = r.slice(0, i);
      if (collapsed[r]) return true;
    }
  }

  const rows = $derived.by(() => {
    const items = preview?.comparison.items ?? [];
    if (filter === "conflicts") return items.filter((i) => steps.get(i.rel)?.conflict);
    const shown = filter === "all" ? items : items.filter((i) => i.state !== "same");
    return shown.filter((i) => !hiddenByCollapse(i.rel));
  });

  const totals = $derived.by(() => {
    const t = { right: 0, left: 0, rightBytes: 0, leftBytes: 0, deletes: 0, skipped: 0, conflicts: 0 };
    for (const s of preview?.steps ?? []) {
      const a = actionOf(s);
      if (s.conflict && a === "skip") t.conflicts++;
      if (a === "copyToRight") {
        t.right += s.left?.files ?? 0;
        t.rightBytes += s.left?.bytes ?? 0;
      } else if (a === "copyToLeft") {
        t.left += s.right?.files ?? 0;
        t.leftBytes += s.right?.bytes ?? 0;
      } else if (a === "deleteLeft" || a === "deleteRight") t.deletes++;
      else t.skipped++;
    }
    return t;
  });
  const work = $derived((preview?.steps ?? []).some((s) => actionOf(s) !== "skip"));

  async function run() {
    loading = true;
    error = null;
    progress = null;
    confirmDeletes = false;
    options.exclude = excludeText
      .split(/[;,]/)
      .map((s) => s.trim())
      .filter(Boolean);
    try {
      preview = await api.syncPlan(left, right, mode, $state.snapshot(options));
      overrides = {};
      if (selected && !preview.comparison.items.some((i) => i.rel === selected)) selected = null;
    } catch (e) {
      if ((e as { kind?: string }).kind !== "cancelled") error = errorMessage(e);
    } finally {
      loading = false;
      progress = null;
    }
  }

  onMount(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    api.onCompareProgress((p) => (progress = p)).then((f) => (off1 = f));
    api
      .onOperationUpdated((info) => {
        if (info.id !== syncing) return;
        if (info.status === "completed" || info.status === "failed" || info.status === "cancelled") {
          syncing = null;
          if (info.status === "failed") error = `Sync stopped: ${info.error?.message ?? "failed"}`;
          run();
        }
      })
      .then((f) => (off2 = f));
    untrack(run);
    return () => {
      off1?.();
      off2?.();
      if (loading) api.cancelCompare();
    };
  });

  function swap() {
    [left, right] = [right, left];
    run();
  }

  function setMode(m: SyncMode) {
    if (m === mode) return;
    mode = m;
    run();
  }

  async function synchronize() {
    if (!preview) return;
    if (totals.deletes > 0 && !confirmDeletes) {
      confirmDeletes = true;
      return;
    }
    confirmDeletes = false;
    try {
      syncing = await api.runSync(preview.id, $state.snapshot(overrides));
      preview = { ...preview, steps: [] };
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function forgetHistory() {
    await api.forgetSyncHistory(left, right);
    run();
  }

  async function saveFavorite(name: string) {
    naming = null;
    const saved = await onsavefavorite({
      id: fav?.id ?? "",
      name,
      left,
      right,
      mode,
      options: { ...$state.snapshot(options) },
    });
    if (saved) fav = saved;
  }

  function compareSelected(item?: DiffItem) {
    const it = item ?? preview?.comparison.items.find((i) => i.rel === selected);
    if (!it || it.dir || !it.left || !it.right) return;
    oncomparefiles(at(left, it.rel), at(right, it.rel)).catch((e) => (error = errorMessage(e)));
  }

  /** The location of a relative path below `root`. */
  function at(root: Location, rel: string): Location {
    const sep = root.path.includes("\\") && !root.path.includes("/") ? "\\" : "/";
    const base = root.path.endsWith(sep) ? root.path : root.path + sep;
    return { fs: root.fs, path: base + rel.split("/").join(sep) };
  }

  const ACTIONS: { value: SyncAction; label: string }[] = [
    { value: "copyToRight", label: "→  Copy to right" },
    { value: "copyToLeft", label: "←  Copy to left" },
    { value: "deleteLeft", label: "✕  Delete on left" },
    { value: "deleteRight", label: "✕  Delete on right" },
    { value: "skip", label: "–  Skip" },
  ];

  function allowed(s: SyncStep, a: SyncAction) {
    if (a === "copyToRight" || a === "deleteLeft") return !!s.left;
    if (a === "copyToLeft" || a === "deleteRight") return !!s.right;
    return true;
  }

  const ICON: Record<SyncAction, string> = {
    copyToRight: "→",
    copyToLeft: "←",
    deleteLeft: "✕ ←",
    deleteRight: "→ ✕",
    skip: "–",
  };

  const REASON: Record<string, string> = {
    new: "New",
    changed: "Changed",
    deleted: "Deleted on the other side since the last sync",
    extra: "Only on the right; a mirror removes it",
    onlyOnTarget: "Only on the right",
    targetChanged: "Changed on the right",
    conflict: "Changed on both sides: choose which to keep",
    changedAndDeleted: "Changed on one side, deleted on the other: it is copied back",
    kindMismatch: "A file on one side and a folder on the other: choose which to keep",
  };

  const STATE: Record<string, string> = {
    same: "Same",
    leftOnly: "Only on the left",
    rightOnly: "Only on the right",
    leftNewer: "Newer on the left",
    rightNewer: "Newer on the right",
    different: "Different",
    kindMismatch: "File and folder",
  };

  function sideText(item: DiffItem, which: "left" | "right") {
    const s = item[which];
    if (!s) return "";
    if (s.dir) return "";
    return `${formatBytes(s.size)} · ${formatShortDate(s.modifiedMs)}`;
  }

  function keydown(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (e.key === "Escape") {
      e.stopPropagation();
      if (confirmDeletes) confirmDeletes = false;
      else if (naming !== null) naming = null;
      else onclose();
      return;
    }
    if (t.tagName === "INPUT" || t.tagName === "SELECT") return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const i = rows.findIndex((r) => r.rel === selected);
      const n = Math.max(0, Math.min(rows.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)));
      selected = rows[n]?.rel ?? null;
      document.getElementById(`row-${n}`)?.scrollIntoView({ block: "nearest" });
      e.preventDefault();
    }
    if (e.key === "Enter" && selected) {
      compareSelected();
      e.preventDefault();
    }
  }

  const location = (l: Location) => `${nameOf(l.fs)} · ${l.path}`;
  const nothingDiffers = $derived(!!preview && !loading && preview.comparison.items.every((i) => i.state === "same"));
</script>

<svelte:window onkeydown={keydown} />

<div class="backdrop">
  <div class="sheet" role="dialog" aria-modal="true" aria-label="Compare and sync folders">
    <header>
      <h2>{fav ? fav.name : "Compare & Sync"}</h2>
      <div class="pair">
        <span class="loc" title={location(left)}>{location(left)}</span>
        <button class="swap" onclick={swap} title="Swap sides" aria-label="Swap sides">⇄</button>
        <span class="loc" title={location(right)}>{location(right)}</span>
      </div>
      <button class="close" onclick={onclose} aria-label="Close">×</button>
    </header>

    <div class="controls">
      <div class="seg" role="group" aria-label="Sync mode">
        <button aria-pressed={mode === "update"} onclick={() => setMode("update")} title="Copy new and changed items from left to right. Nothing is deleted."
          >Update →</button
        >
        <button aria-pressed={mode === "mirror"} onclick={() => setMode("mirror")} title="Make the right side an exact copy of the left, deleting extra items there."
          >Mirror →</button
        >
        <button aria-pressed={mode === "twoWay"} onclick={() => setMode("twoWay")} title="Copy changes both ways and repeat deletions made since the last sync."
          >Two-way ⇄</button
        >
      </div>
      <label>
        Compare by
        <select bind:value={options.by} onchange={run}>
          <option value={"sizeAndDate" satisfies CompareBy}>Size and date</option>
          <option value={"size" satisfies CompareBy}>Size</option>
          <option value={"content" satisfies CompareBy}>Content</option>
        </select>
      </label>
      <label class="grow">
        Ignore
        <input bind:value={excludeText} onkeydown={(e) => e.key === "Enter" && run()} placeholder="*.tmp; .git" spellcheck="false" />
      </label>
      <label class="check"><input type="checkbox" bind:checked={options.includeHidden} onchange={run} /> Hidden items</label>
      <button class="btn" onclick={run} disabled={loading}>Compare again</button>
    </div>

    <div class="filters" role="tablist">
      <button role="tab" aria-selected={filter === "changes"} onclick={() => (filter = "changes")}>Differences</button>
      <button role="tab" aria-selected={filter === "all"} onclick={() => (filter = "all")}>All items</button>
      <button role="tab" aria-selected={filter === "conflicts"} onclick={() => (filter = "conflicts")}>
        Conflicts {#if totals.conflicts}<span class="badge">{totals.conflicts}</span>{/if}
      </button>
      <span class="spacer"></span>
      {#if preview}
        {@const s = preview.comparison.summary}
        <span class="summary">
          {s.same} same · {s.leftOnly} only left · {s.rightOnly} only right · {s.leftNewer + s.rightNewer + s.different} changed
        </span>
      {/if}
    </div>

    <div class="table" role="grid" aria-busy={loading}>
      <div class="row head" role="row">
        <span role="columnheader">Name</span>
        <span role="columnheader" class="num">Left</span>
        <span role="columnheader" class="act">Action</span>
        <span role="columnheader" class="num">Right</span>
      </div>
      {#if loading}
        <p class="status">
          Comparing… {#if progress}{progress.folders} folders read<br /><span class="current">{progress.current}</span>{/if}
        </p>
      {:else if error && !preview}
        <p class="status err">{error}</p>
      {:else if nothingDiffers}
        <p class="status">The folders are the same.</p>
      {:else if rows.length === 0}
        <p class="status">{filter === "conflicts" ? "No conflicts." : "Nothing to show."}</p>
      {/if}
      {#if !loading}
        {#each rows as item, n (item.rel)}
          {@const step = steps.get(item.rel)}
          {@const cover = step ? undefined : coveringStep(item.rel)}
          {@const action = step ? actionOf(step) : null}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            id="row-{n}"
            class="row {item.state}"
            class:selected={selected === item.rel}
            class:conflict={step?.conflict && action === "skip"}
            role="row"
            tabindex="-1"
            aria-selected={selected === item.rel}
            onclick={() => (selected = item.rel)}
            ondblclick={() => compareSelected(item)}
            title={step ? REASON[step.reason] : STATE[item.state]}
          >
            <span class="name" style="padding-left: {(filter === 'conflicts' ? 0 : item.depth) * 16 + 4}px">
              {#if item.dir && filter !== "conflicts"}
                <button
                  class="disclose"
                  aria-label={collapsed[item.rel] ? "Expand" : "Collapse"}
                  onclick={(e) => {
                    e.stopPropagation();
                    collapsed[item.rel] = !collapsed[item.rel];
                  }}>{collapsed[item.rel] ? "▸" : "▾"}</button
                >
              {:else}
                <span class="disclose"></span>
              {/if}
              <span aria-hidden="true">{item.dir ? "📁" : "📄"}</span>
              <span class="label">{filter === "conflicts" ? item.rel : item.name}</span>
            </span>
            <span class="num side" class:strong={item.state === "leftNewer" || item.state === "leftOnly"}>
              {item.left ? sideText(item, "left") : "—"}
            </span>
            <span class="act">
              {#if step}
                <select
                  value={action}
                  aria-label="Action for {item.name}"
                  onclick={(e) => e.stopPropagation()}
                  onchange={(e) => {
                    const v = (e.currentTarget as HTMLSelectElement).value as SyncAction;
                    if (v === step.action) delete overrides[step.rel];
                    else overrides[step.rel] = v;
                  }}
                  class:delete={action === "deleteLeft" || action === "deleteRight"}
                  class:changed={overrides[step.rel] !== undefined}
                >
                  {#each ACTIONS.filter((a) => allowed(step, a.value)) as a (a.value)}
                    <option value={a.value}>{a.label}</option>
                  {/each}
                </select>
                <span class="icon" aria-hidden="true">{ICON[action!]}</span>
              {:else if cover}
                <span class="muted">with folder</span>
              {:else if item.state === "same"}
                <span class="muted">=</span>
              {:else}
                <span class="muted">{STATE[item.state]}</span>
              {/if}
            </span>
            <span class="num side" class:strong={item.state === "rightNewer" || item.state === "rightOnly"}>
              {item.right ? sideText(item, "right") : "—"}
            </span>
          </div>
        {/each}
      {/if}
    </div>

    <footer>
      {#if confirmDeletes}
        <span class="warn">This deletes {totals.deletes} {totals.deletes === 1 ? "item" : "items"} for good. Synchronize anyway?</span>
        <span class="spacer"></span>
        <button class="btn" onclick={() => (confirmDeletes = false)}>Cancel</button>
        <button class="btn primary danger" onclick={synchronize}>Delete and sync</button>
      {:else if naming !== null}
        <form
          class="naming"
          onsubmit={(e) => {
            e.preventDefault();
            if (naming?.trim()) saveFavorite(naming.trim());
          }}
        >
          <!-- svelte-ignore a11y_autofocus -->
          <label>Favorite name <input bind:value={naming} spellcheck="false" autofocus /></label>
          <button type="button" class="btn" onclick={() => (naming = null)}>Cancel</button>
          <button type="submit" class="btn primary" disabled={!naming.trim()}>Save</button>
        </form>
      {:else}
        <span class="plan">
          {#if syncing}
            Syncing… progress is in Operations below.
          {:else if error && preview}
            <span class="err">{error}</span>
          {:else if preview}
            {#if totals.right}{totals.right} to the right ({formatBytes(totals.rightBytes)}) ·{/if}
            {#if totals.left}{totals.left} to the left ({formatBytes(totals.leftBytes)}) ·{/if}
            {#if totals.deletes}{totals.deletes} to delete ·{/if}
            {#if totals.conflicts}<span class="err">{totals.conflicts} {totals.conflicts === 1 ? "conflict" : "conflicts"} skipped</span> ·{/if}
            {#if mode === "twoWay"}{preview.hasHistory ? "Uses the last sync to spot deletions" : "First two-way sync of these folders: newer wins and nothing is deleted"}{/if}
          {/if}
        </span>
        <span class="spacer"></span>
        {#if preview?.hasHistory}
          <button class="link" onclick={forgetHistory} title="Forget what the last sync saw, so deletions are not repeated">Forget last sync</button>
        {/if}
        <button class="btn" onclick={() => compareSelected()} disabled={!preview?.comparison.items.some((i) => i.rel === selected && !i.dir && i.left && i.right)}
          >Compare Files</button
        >
        <button class="btn" onclick={() => (naming = fav?.name ?? `${left.path.split(/[\\/]/).filter(Boolean).pop() ?? "Sync"} ⇄ ${right.path.split(/[\\/]/).filter(Boolean).pop() ?? ""}`)}
          >{fav ? "Update Favorite" : "Save as Favorite…"}</button
        >
        <button class="btn primary" onclick={synchronize} disabled={!work || loading || !!syncing} title="Carry out the actions above">Synchronize</button>
      {/if}
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: center;
    z-index: 9;
  }
  .sheet {
    width: calc(100vw - 48px);
    height: calc(100vh - 48px);
    max-width: 1200px;
    display: grid;
    grid-template-rows: auto auto auto minmax(0, 1fr) auto;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 16px 8px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
    white-space: nowrap;
  }
  .pair {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    align-items: center;
    gap: 8px;
  }
  .loc {
    padding: 4px 8px;
    border-radius: 6px;
    background: var(--stripe);
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    direction: rtl;
    text-align: left;
  }
  .swap,
  .close {
    font-size: 16px;
    padding: 2px 8px;
    border-radius: 6px;
    color: var(--muted);
  }
  .swap:hover,
  .close:hover {
    background: var(--control);
    color: var(--text);
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    padding: 4px 16px 10px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .controls label {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .controls .grow {
    flex: 1;
    min-width: 160px;
  }
  .controls .grow input {
    flex: 1;
  }
  input,
  select {
    font: inherit;
    color: var(--text);
    background: var(--control);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 3px 6px;
  }
  .check input {
    padding: 0;
  }
  .seg {
    display: flex;
    gap: 1px;
    border-radius: 7px;
    overflow: hidden;
    background: var(--border);
  }
  .seg button {
    padding: 4px 10px;
    background: var(--control);
  }
  .seg button[aria-pressed="true"] {
    background: var(--accent);
    color: white;
  }
  .btn {
    padding: 4px 12px;
    border-radius: 6px;
    background: var(--control);
    font-weight: 500;
    white-space: nowrap;
  }
  .btn:hover:not(:disabled) {
    background: var(--control-hover);
  }
  .btn.primary {
    background: var(--accent);
    color: white;
  }
  .btn.danger {
    background: var(--danger);
  }
  .link {
    color: var(--accent);
    font-size: 12px;
  }
  .filters {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 12px;
    font-size: 12px;
  }
  .filters [role="tab"] {
    padding: 3px 10px;
    border-radius: 6px;
    color: var(--muted);
  }
  .filters [role="tab"][aria-selected="true"] {
    background: var(--control);
    color: var(--text);
    font-weight: 500;
  }
  .badge {
    background: var(--danger);
    color: white;
    border-radius: 999px;
    padding: 0 6px;
    font-size: 11px;
    margin-left: 4px;
  }
  .summary {
    color: var(--muted);
  }
  .spacer {
    flex: 1;
  }
  .table {
    overflow: auto;
    border-top: 1px solid var(--border);
  }
  .row {
    display: grid;
    grid-template-columns: minmax(200px, 1fr) 190px 170px 190px;
    align-items: center;
    min-height: 26px;
    padding: 0 8px;
    border-bottom: 1px solid var(--stripe);
  }
  .row.head {
    position: sticky;
    top: 0;
    background: var(--chrome);
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    z-index: 1;
  }
  .row.selected {
    background: color-mix(in srgb, var(--selection) 18%, transparent);
  }
  .row.conflict {
    background: color-mix(in srgb, var(--danger) 10%, transparent);
  }
  .row.same {
    color: var(--muted);
  }
  .name {
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
  .disclose {
    width: 12px;
    font-size: 9px;
    color: var(--muted);
    flex-shrink: 0;
  }
  .num {
    text-align: right;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    padding: 0 8px;
  }
  .side {
    color: var(--muted);
  }
  .side.strong {
    color: var(--text);
    font-weight: 500;
  }
  .act {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    font-size: 12px;
  }
  .act select {
    width: 128px;
    padding: 1px 4px;
    font-size: 12px;
  }
  .act select.changed {
    border-color: var(--accent);
  }
  .act select.delete {
    color: var(--danger);
  }
  .act .icon {
    width: 28px;
    color: var(--accent);
    font-weight: 600;
  }
  .muted {
    color: var(--muted);
  }
  .status {
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }
  .status .current {
    font-size: 11px;
    overflow-wrap: anywhere;
  }
  .err {
    color: var(--danger);
  }
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--border);
    background: var(--chrome);
    font-size: 12px;
  }
  .plan {
    color: var(--muted);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .warn {
    color: var(--danger);
    font-weight: 500;
  }
  .naming {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
  }
  .naming label {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
  }
  .naming input {
    flex: 1;
  }
</style>
