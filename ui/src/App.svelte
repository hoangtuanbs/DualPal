<script lang="ts">
  import { onMount } from "svelte";
  import {
    api,
    errorMessage,
    type ActivityRecord,
    type ConflictPolicy,
    type Location,
    type Operation,
    type OperationInfo,
  } from "./lib/api";
  import Dialog from "./lib/Dialog.svelte";
  import OperationsPanel from "./lib/OperationsPanel.svelte";
  import Pane from "./lib/Pane.svelte";
  import { PaneState } from "./lib/pane.svelte";

  const panes = [new PaneState(), new PaneState()];
  let activeIndex = $state(0);
  const active = $derived(panes[activeIndex]);
  const other = $derived(panes[1 - activeIndex]);

  let operations = $state<OperationInfo[]>([]);
  let activity = $state<ActivityRecord[]>([]);
  /** The request behind each operation id, so a failed one can be retried. */
  const requests = new Map<string, Operation>();
  let notice = $state<string | null>(null);

  type DialogSpec = {
    title: string;
    message?: string;
    input?: string | null;
    confirmLabel?: string;
    danger?: boolean;
    onconfirm: (value: string) => void;
  };
  let dialog = $state<DialogSpec | null>(null);

  onMount(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      unlisten = await api.onOperationUpdated(onOperationUpdated);
      const home = await api.homeLocation();
      await Promise.all(panes.map((p) => p.open(home)));
      operations = await api.operations();
      activity = await api.activity();
    })().catch((e) => (notice = errorMessage(e)));
    return () => unlisten?.();
  });

  function onOperationUpdated(info: OperationInfo) {
    const i = operations.findIndex((o) => o.id === info.id);
    if (i >= 0) operations[i] = info;
    else operations.push(info);
    if (info.status === "completed" || info.status === "failed" || info.status === "cancelled") {
      panes.forEach((p) => p.refresh());
      api.activity().then((a) => (activity = a));
    }
  }

  async function enqueue(op: Operation) {
    try {
      const id = await api.enqueue(op);
      requests.set(id, op);
    } catch (e) {
      notice = errorMessage(e);
    }
  }

  function locationsOf(pane: PaneState): Location[] {
    const fs = pane.location?.fs ?? "local";
    return pane.targets.map((e) => ({ fs, path: e.path }));
  }

  function describe(pane: PaneState) {
    const t = pane.targets;
    return t.length === 1 ? `“${t[0].name}”` : `${t.length} items`;
  }

  function transfer(type: "copy" | "move") {
    const sources = locationsOf(active);
    if (!sources.length || !other.location) return;
    enqueue({ type, sources, destination: other.location, conflict: "fail" });
  }

  function remove() {
    const targets = locationsOf(active);
    if (!targets.length) return;
    dialog = {
      title: `Delete ${describe(active)}?`,
      message: "This permanently deletes the selected items. It cannot be undone.",
      confirmLabel: "Delete",
      danger: true,
      onconfirm: () => enqueue({ type: "delete", targets }),
    };
  }

  function newFolder() {
    const loc = active.location;
    if (!loc) return;
    dialog = {
      title: "New folder",
      input: "untitled folder",
      confirmLabel: "Create",
      onconfirm: (name) => {
        const path = join(loc, name);
        enqueue({ type: "createDir", location: { fs: loc.fs, path } });
      },
    };
  }

  function rename() {
    const entry = active.current;
    const loc = active.location;
    if (!entry || !loc) return;
    dialog = {
      title: "Rename",
      input: entry.name,
      confirmLabel: "Rename",
      onconfirm: (newName) => {
        if (newName !== entry.name) enqueue({ type: "rename", location: { fs: loc.fs, path: entry.path }, newName });
      },
    };
  }

  /** Join a child name onto a folder path using the folder's separator. */
  function join(dir: Location, name: string) {
    const sep = dir.path.includes("\\") && !dir.path.includes("/") ? "\\" : "/";
    return dir.path.endsWith(sep) ? dir.path + name : dir.path + sep + name;
  }

  function retry(id: string, conflict: ConflictPolicy) {
    const op = requests.get(id);
    if (op && (op.type === "copy" || op.type === "move")) enqueue({ ...op, conflict });
  }

  async function clearFinished() {
    await api.clearFinished();
    operations = await api.operations();
  }

  async function clearActivity() {
    await api.clearActivity();
    activity = [];
  }

  function onkeydown(e: KeyboardEvent) {
    if (dialog) return;
    const mod = e.metaKey || e.ctrlKey;
    const handled = () => e.preventDefault();
    switch (e.key) {
      case "Tab":
        activeIndex = 1 - activeIndex;
        return handled();
      case "ArrowDown":
        active.moveCursor(1);
        return handled();
      case "ArrowUp":
        if (mod) active.up();
        else active.moveCursor(-1);
        return handled();
      case "PageDown":
        active.moveCursor(20);
        return handled();
      case "PageUp":
        active.moveCursor(-20);
        return handled();
      case "Home":
        active.cursor = 0;
        return handled();
      case "End":
        active.moveCursor(active.visible.length);
        return handled();
      case "Enter":
        if (active.current) active.activate(active.current);
        return handled();
      case "Backspace":
        if (mod) remove();
        else active.up();
        return handled();
      case " ":
      case "Insert":
        if (active.current) active.toggle(active.current);
        active.moveCursor(1);
        return handled();
      case "F2":
        rename();
        return handled();
      case "F5":
        transfer("copy");
        return handled();
      case "F6":
        transfer("move");
        return handled();
      case "F7":
        newFolder();
        return handled();
      case "F8":
      case "Delete":
        remove();
        return handled();
    }
    if (mod && e.key.toLowerCase() === "r") {
      active.refresh();
      return handled();
    }
    if (mod && e.key.toLowerCase() === "a") {
      active.visible.forEach((it) => active.selected.add(it.path));
      return handled();
    }
    if (mod && e.shiftKey && e.key === ".") {
      active.showHidden = !active.showHidden;
      return handled();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="app">
  <div class="toolbar">
    <div class="group">
      <button onclick={() => transfer("copy")} title="Copy to the other pane (F5)">Copy →</button>
      <button onclick={() => transfer("move")} title="Move to the other pane (F6)">Move →</button>
    </div>
    <div class="group">
      <button onclick={newFolder} title="New folder (F7)">New Folder</button>
      <button onclick={rename} title="Rename (F2)">Rename</button>
      <button onclick={remove} title="Delete (F8)">Delete</button>
    </div>
    <div class="group">
      <button onclick={() => active.refresh()} title="Refresh (Ctrl/⌘ R)">Refresh</button>
      <button
        aria-pressed={active.showHidden}
        onclick={() => (active.showHidden = !active.showHidden)}
        title="Show hidden files (Ctrl/⌘ Shift .)">Hidden files</button
      >
    </div>
    {#if notice}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <span class="notice" onclick={() => (notice = null)}>{notice}</span>
    {/if}
  </div>

  <main class="panes">
    {#each panes as pane, i}
      <Pane {pane} active={i === activeIndex} onfocus={() => (activeIndex = i)} />
    {/each}
  </main>

  <OperationsPanel
    {operations}
    {activity}
    oncancel={(id) => api.cancel(id)}
    onretry={retry}
    onclear={clearFinished}
    onclearactivity={clearActivity}
  />
</div>

{#if dialog}
  {@const d = dialog}
  <Dialog
    title={d.title}
    message={d.message}
    input={d.input ?? null}
    confirmLabel={d.confirmLabel}
    danger={d.danger}
    oncancel={() => (dialog = null)}
    onconfirm={(v) => {
      // Read the callback before clearing: `d` is derived from `dialog`.
      const confirm = d.onconfirm;
      dialog = null;
      confirm(v);
    }}
  />
{/if}

<style>
  .app {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) 190px;
    height: 100vh;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
  }
  .group {
    display: flex;
    gap: 1px;
    border-radius: 7px;
    overflow: hidden;
    background: var(--border);
    flex-shrink: 0;
  }
  .group button {
    padding: 5px 11px;
    background: var(--control);
    white-space: nowrap;
  }
  .group button:hover {
    background: var(--control-hover);
  }
  .group button[aria-pressed="true"] {
    background: var(--accent);
    color: white;
  }
  .notice {
    color: var(--danger);
    font-size: 12px;
    cursor: pointer;
  }
  .panes {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1px;
    background: var(--border);
    min-height: 0;
  }
</style>
