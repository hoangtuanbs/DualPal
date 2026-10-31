<script lang="ts">
  import { onMount } from "svelte";
  import {
    api,
    errorKind,
    errorMessage,
    type ActivityRecord,
    type Connection,
    type ConnectionView,
    type EditSession,
    type ConflictPolicy,
    type Favorite,
    type Volume,
    type Location,
    type Operation,
    type OperationInfo,
  } from "./lib/api";
  import Dialog from "./lib/Dialog.svelte";
  import OperationsPanel from "./lib/OperationsPanel.svelte";
  import Pane from "./lib/Pane.svelte";
  import Inspector from "./lib/Inspector.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import ConnectionDialog from "./lib/ConnectionDialog.svelte";
  import EditSessions from "./lib/EditSessions.svelte";
  import { fsNames } from "./lib/fs-names.svelte";
  import { patternMatcher } from "./lib/fileTypes";
  import { PaneGroup, PaneState } from "./lib/pane-state.svelte";

  const groups = [new PaneGroup(new PaneState()), new PaneGroup(new PaneState())];
  let activeIndex = $state(0);
  const active = $derived(groups[activeIndex].current);
  const other = $derived(groups[1 - activeIndex].current);
  /** Every open tab, for refreshing after operations. */
  const allPanes = () => groups.flatMap((g) => g.tabs);

  let showInspector = $state(true);
  let volumes = $state<Volume[]>([]);
  let favorites = $state<Favorite[]>([]);
  let connections = $state<ConnectionView[]>([]);
  let editSessions = $state<EditSession[]>([]);
  /** The connection being added (null) or edited; undefined when closed. */
  let editing = $state<Connection | null | undefined>(undefined);

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
    password?: boolean;
    checkbox?: string | null;
    onconfirm: (value: string, checked: boolean) => void;
  };
  let dialog = $state<DialogSpec | null>(null);

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let unlistenEdits: (() => void) | undefined;
    (async () => {
      unlisten = await api.onOperationUpdated(onOperationUpdated);
      unlistenEdits = await api.onEditSessionUpdated(onEditSessionUpdated);
      await loadConnections();
      const home = await api.homeLocation();
      await Promise.all(groups.map((g) => g.current.open(home)));
      operations = await api.operations();
      activity = await api.activity();
      favorites = await api.favorites();
      volumes = await api.volumes();
    })().catch((e) => (notice = errorMessage(e)));
    // Pick up mounted/unmounted drives and changing free space.
    const timer = setInterval(() => api.volumes().then((v) => (volumes = v)), 15000);
    return () => {
      unlisten?.();
      unlistenEdits?.();
      clearInterval(timer);
    };
  });

  function onOperationUpdated(info: OperationInfo) {
    const i = operations.findIndex((o) => o.id === info.id);
    if (i >= 0) operations[i] = info;
    else operations.push(info);
    if (info.status === "completed" || info.status === "failed" || info.status === "cancelled") {
      allPanes().forEach((p) => p.refresh());
      api.activity().then((a) => (activity = a));
    }
  }

  function onEditSessionUpdated(s: EditSession) {
    const i = editSessions.findIndex((x) => x.id === s.id);
    if (s.status === "stopped") {
      if (i >= 0) editSessions.splice(i, 1);
      return;
    }
    if (i >= 0) editSessions[i] = s;
    else editSessions.push(s);
    if (s.status === "uploaded") allPanes().forEach((p) => p.location?.fs === s.remote.fs && p.refresh());
  }

  // ---- servers --------------------------------------------------------------

  async function loadConnections() {
    connections = await api.connections();
    for (const c of connections) fsNames[c.fsId] = c.name;
  }

  /** Connect (asking for a password or a host key confirmation when needed)
   *  and open `path`, or the server's start folder, in the active pane. */
  async function connectTo(c: ConnectionView, path?: string, opts: { secret?: string; remember?: boolean; trust?: string } = {}) {
    notice = null;
    try {
      const r = await api.connect(c.id, opts);
      fsNames[c.fsId] = c.name;
      await active.open(path ? { fs: r.location.fs, path } : r.location);
      await loadConnections();
    } catch (e) {
      const kind = errorKind(e);
      const fp = (e as { fingerprint?: string }).fingerprint;
      if (kind === "unknownHost" && fp) {
        dialog = {
          title: `Trust ${c.name}?`,
          message: `Dual Pal has not connected to this server before. Check that its key fingerprint is ${fp} before you continue. It will be added to ~/.ssh/known_hosts.`,
          confirmLabel: "Trust and connect",
          onconfirm: () => connectTo(c, path, { ...opts, trust: fp }),
        };
      } else if (kind === "authFailed" && c.endpoint.type === "sftp" && c.endpoint.auth.method !== "agent") {
        const what = c.endpoint.auth.method === "keyFile" ? "key passphrase" : "password";
        dialog = {
          title: `Sign in to ${c.name}`,
          message: `${opts.secret ? `That ${what} did not work. ` : ""}Enter the ${what} for ${c.endpoint.user}@${c.endpoint.host}.`,
          input: "",
          password: true,
          checkbox: navigator.userAgent.includes("Windows") ? "Remember in Credential Manager" : "Remember in Keychain",
          confirmLabel: "Connect",
          onconfirm: (secret, remember) => connectTo(c, path, { ...opts, secret, remember }),
        };
      } else if (kind === "authFailed" && c.endpoint.type === "s3") {
        notice = errorMessage(e);
        editing = c;
      } else {
        notice = errorMessage(e);
      }
    }
  }

  async function disconnect(c: ConnectionView) {
    await api.disconnect(c.id);
    // Panes showing the server go back home.
    const home = await api.homeLocation();
    for (const p of allPanes()) if (p.location?.fs === c.fsId) await p.open(home, { record: false });
    editSessions = editSessions.filter((s) => s.remote.fs !== c.fsId);
    await loadConnections();
  }

  function removeServer(c: ConnectionView) {
    dialog = {
      title: `Remove ${c.name}?`,
      message: "The server and its favorites are removed from Dual Pal, along with its saved password. Nothing on the server changes.",
      confirmLabel: "Remove",
      danger: true,
      onconfirm: async () => {
        await api.removeConnection(c.id).catch((e) => (notice = errorMessage(e)));
        await loadConnections();
      },
    };
  }

  async function serverSaved(saved: Connection, connect: boolean) {
    editing = undefined;
    await loadConnections();
    const c = connections.find((x) => x.id === saved.id);
    if (c && connect) {
      // Settings may have changed: reconnect with the new ones.
      if (c.connected) await api.disconnect(c.id);
      await connectTo(c);
    }
  }

  /** The saved connection behind a file system id, if it is remote. */
  const connectionOf = (fs: string | undefined) => connections.find((c) => c.fsId === fs);

  // ---- editing --------------------------------------------------------------

  /** Open the file under the cursor in the preferred editor. */
  async function editInEditor() {
    const entry = active.current;
    const loc = active.location;
    if (!entry || !loc || entry.kind === "dir") return;
    try {
      await api.openFile({ fs: loc.fs, path: entry.path }, true);
    } catch (e) {
      notice = errorMessage(e);
    }
  }

  async function chooseEditor() {
    const s = await api.settings();
    const mac = !navigator.userAgent.includes("Windows");
    dialog = {
      title: "Preferred editor",
      message: mac
        ? "App used by Edit, e.g. Visual Studio Code, BBEdit or /Applications/Sublime Text.app. Leave empty for each file's default app."
        : "Program used by Edit, e.g. C:\\Program Files\\Notepad++\\notepad++.exe or code. Leave empty for each file's default app.",
      input: s.editor ?? "",
      confirmLabel: "Save",
      onconfirm: (editor) => api.saveSettings({ ...s, editor: editor || null }).catch((e) => (notice = errorMessage(e))),
    };
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

  function trash() {
    const targets = locationsOf(active);
    if (!targets.length) return;
    // Servers and buckets have no Trash: offer to delete instead.
    if (active.location?.fs !== "local") remove();
    else enqueue({ type: "trash", targets });
  }

  /** Zip the targets into a new archive next to them. */
  function compress() {
    const loc = active.location;
    const t = active.targets;
    if (!loc || !t.length) return;
    const taken = new Set(active.entries.map((e) => e.name.toLowerCase()));
    const stem = t.length === 1 ? t[0].name.replace(/\.[^.]+$/, "") || t[0].name : "Archive";
    let name = `${stem}.zip`;
    for (let n = 2; taken.has(name.toLowerCase()); n++) name = `${stem} ${n}.zip`;
    enqueue({ type: "compress", sources: locationsOf(active), destination: { fs: loc.fs, path: join(loc, name) } });
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

  function selectByPattern(select: boolean) {
    dialog = {
      title: select ? "Select items" : "Deselect items",
      message: "Names to match, e.g. *.jpg; *.png or report*. Separate several with ; or ,",
      input: select ? "*.*" : "*",
      confirmLabel: select ? "Select" : "Deselect",
      onconfirm: (pattern) => active.selectWhere(patternMatcher(pattern), select),
    };
  }

  async function toggleFavorite() {
    const loc = active.location;
    if (!loc) return;
    const conn = connectionOf(loc.fs);
    try {
      if (conn) {
        // A folder on a server becomes a favorite inside that server.
        if (isFavorite) await api.removeConnectionFavorite(conn.id, loc.path);
        else await api.addConnectionFavorite(conn.id, { name: active.title || loc.path, path: loc.path });
        await loadConnections();
        return;
      }
      favorites = isFavorite
        ? await api.removeFavorite(loc)
        : await api.addFavorite({ name: active.title || loc.path, location: loc, icon: "folder" });
    } catch (e) {
      notice = errorMessage(e);
    }
  }

  const isFavorite = $derived.by(() => {
    const loc = active.location;
    if (!loc) return false;
    const conn = connectionOf(loc.fs);
    if (conn) return conn.favorites.some((f) => f.path === loc.path);
    return favorites.some((f) => f.location.fs === loc.fs && f.location.path === loc.path);
  });

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
    // Typing in the quick editor or another field: leave keys alone.
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
    const mod = e.metaKey || e.ctrlKey;
    const handled = () => e.preventDefault();
    const group = groups[activeIndex];
    const icons = active.view === "icons";
    if (e.ctrlKey && e.key === "Tab") {
      group.cycle(e.shiftKey ? -1 : 1);
      return handled();
    }
    if (mod && !e.shiftKey && e.key.toLowerCase() === "t") {
      group.newTab();
      return handled();
    }
    if (mod && !e.shiftKey && e.key.toLowerCase() === "w") {
      group.closeTab();
      return handled();
    }
    if (mod && !e.shiftKey && e.key.toLowerCase() === "i") {
      showInspector = !showInspector;
      return handled();
    }
    if (mod && (e.key === "1" || e.key === "2")) {
      active.view = e.key === "1" ? "list" : "icons";
      return handled();
    }
    switch (e.key) {
      case "Tab":
        activeIndex = 1 - activeIndex;
        return handled();
      case "ArrowDown":
        active.moveCursor(icons ? active.columns : 1);
        return handled();
      case "ArrowUp":
        if (mod) active.up();
        else active.moveCursor(icons ? -active.columns : -1);
        return handled();
      case "ArrowLeft":
        if (!icons) return;
        active.moveCursor(-1);
        return handled();
      case "ArrowRight":
        if (!icons) return;
        active.moveCursor(1);
        return handled();
      case "+":
        selectByPattern(true);
        return handled();
      case "-":
        selectByPattern(false);
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
        if (mod) (e.altKey ? remove() : trash());
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
      case "F4":
        editInEditor();
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
        if (e.shiftKey) remove();
        else trash();
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
      <button onclick={editInEditor} title="Edit in your preferred editor (F4). Files on servers upload again when you save.">Edit</button>
      <button onclick={compress} title="Compress into a ZIP archive">Compress</button>
      <button onclick={trash} title="Move to Trash (F8 / Delete). Shift+Delete deletes permanently.">Trash</button>
    </div>
    <div class="group" role="group" aria-label="View">
      <button aria-pressed={active.view === "list"} onclick={() => (active.view = "list")} title="List view (Ctrl/⌘ 1)">☰ List</button>
      <button aria-pressed={active.view === "icons"} onclick={() => (active.view = "icons")} title="Icon view (Ctrl/⌘ 2)">▦ Icons</button>
    </div>
    <div class="group">
      <button onclick={() => selectByPattern(true)} title="Select by name or extension (+). Deselect with −.">Select…</button>
      <button aria-pressed={isFavorite} onclick={toggleFavorite} title={isFavorite ? "Remove folder from Favorites" : "Add folder to Favorites"}>★</button>
      <button onclick={() => groups[activeIndex].newTab()} title="New tab (Ctrl/⌘ T)">+ Tab</button>
      <button aria-pressed={showInspector} onclick={() => (showInspector = !showInspector)} title="Show preview and info (Ctrl/⌘ I)">ⓘ Info</button>
    </div>
    <div class="group">
      <button onclick={() => active.refresh()} title="Refresh (Ctrl/⌘ R)">Refresh</button>
      <button onclick={chooseEditor} title="Choose the editor used by Edit">Editor…</button>
      <button
        aria-pressed={active.showHidden}
        onclick={() => (active.showHidden = !active.showHidden)}
        title="Show hidden files (Ctrl/⌘ Shift .)">Hidden files</button
      >
    </div>
    {#if notice}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <span class="notice" title={notice} onclick={() => (notice = null)}>{notice}</span>
    {/if}
  </div>

  <div class="middle" class:with-inspector={showInspector}>
    <Sidebar
      {volumes}
      {favorites}
      current={active.location}
      onopen={(loc) => active.open(loc)}
      onremovefavorite={async (loc) => (favorites = await api.removeFavorite(loc))}
      onreorder={async (from, to) => (favorites = await api.reorderFavorites(from, to))}
      {connections}
      onconnect={(c, path) => connectTo(c, path)}
      onaddserver={() => (editing = null)}
      oneditserver={(c) => (editing = c)}
      ondisconnect={disconnect}
      onremoveserver={removeServer}
      onremoveremotefavorite={async (c, path) => {
        await api.removeConnectionFavorite(c.id, path);
        await loadConnections();
      }}
    />
    <main class="panes">
      {#each groups as group, i}
        <Pane {group} active={i === activeIndex} onfocus={() => (activeIndex = i)} />
      {/each}
    </main>
    {#if showInspector}
      <Inspector
        entry={active.current ?? null}
        fs={active.location?.fs ?? "local"}
        onsetmode={(location, mode) => enqueue({ type: "setMode", location, mode })}
        onsaved={() => allPanes().forEach((p) => p.refresh())}
      />
    {/if}
  </div>

  <div class="bottom">
    <EditSessions sessions={editSessions} onstop={(id) => api.stopEditSession(id)} />
    <div class="ops">
      <OperationsPanel
        {operations}
        {activity}
        oncancel={(id) => api.cancel(id)}
        onretry={retry}
        onclear={clearFinished}
        onclearactivity={clearActivity}
      />
    </div>
  </div>
</div>

{#if dialog}
  {@const d = dialog}
  <Dialog
    title={d.title}
    message={d.message}
    input={d.input ?? null}
    confirmLabel={d.confirmLabel}
    danger={d.danger}
    password={d.password}
    checkbox={d.checkbox ?? null}
    oncancel={() => (dialog = null)}
    onconfirm={(v, checked) => {
      // Read the callback before clearing: `d` is derived from `dialog`.
      const confirm = d.onconfirm;
      dialog = null;
      confirm(v, checked);
    }}
  />
{/if}

{#if editing !== undefined}
  <ConnectionDialog connection={editing} onsave={serverSaved} oncancel={() => (editing = undefined)} />
{/if}

<style>
  .app {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    height: 100vh;
  }
  .bottom {
    display: flex;
    flex-direction: column;
  }
  .ops {
    height: 190px;
    display: grid;
    min-height: 0;
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
    min-width: 120px;
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .middle {
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    min-height: 0;
  }
  .middle.with-inspector {
    grid-template-columns: 200px minmax(0, 1fr) 300px;
  }
  .panes {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1px;
    background: var(--border);
    min-height: 0;
  }
</style>
