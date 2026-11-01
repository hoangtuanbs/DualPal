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
    type DiffTool,
    type Entry,
    type SyncFavorite,
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
  import CompareView from "./lib/CompareView.svelte";
  import MultiRename from "./lib/MultiRename.svelte";
  import CommandPalette, { type Command } from "./lib/CommandPalette.svelte";
  import Insights from "./lib/Insights.svelte";
  import Icon from "./lib/Icon.svelte";

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
    allowEmpty?: boolean;
    onconfirm: (value: string, checked: boolean) => void;
  };
  let dialog = $state<DialogSpec | null>(null);

  let syncs = $state<SyncFavorite[]>([]);
  /** The folder pair in the Compare & Sync window, when it is open. */
  let comparing = $state<{ left: Location; right: Location; favorite: SyncFavorite | null } | null>(null);
  let renaming = $state<{ items: Entry[]; folder: Location; others: string[] } | null>(null);
  let showPalette = $state(false);
  let showInsights = $state(false);
  /** Synchronized browsing: entering or leaving a folder in one pane does the same in the other. */
  let linked = $state(false);
  let diffTools = $state<DiffTool[]>([]);
  const isMac = navigator.userAgent.includes("Mac");
  const MOD = isMac ? "⌘" : "Ctrl+";

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let unlistenEdits: (() => void) | undefined;
    (async () => {
      unlisten = await api.onOperationUpdated(onOperationUpdated);
      unlistenEdits = await api.onEditSessionUpdated(onEditSessionUpdated);
      await loadConnections();
      editSessions = await api.editSessions();
      const home = await api.homeLocation();
      await Promise.all(groups.map((g) => g.current.open(home)));
      operations = await api.operations();
      activity = await api.activity();
      favorites = await api.favorites();
      volumes = await api.volumes();
      syncs = await api.syncFavorites();
      diffTools = await api.diffTools();
    })().catch((e) => (notice = errorMessage(e)));
    PaneState.navigated = followNavigation;
    // Pick up mounted/unmounted drives and changing free space.
    const timer = setInterval(() => api.volumes().then((v) => (volumes = v)), 15000);
    return () => {
      unlisten?.();
      unlistenEdits?.();
      clearInterval(timer);
      PaneState.navigated = null;
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
  function connectTo(
    c: ConnectionView,
    path?: string,
    opts: { secret?: string; remember?: boolean; trust?: string } = {},
    // The pane that was active when the user asked, kept through the prompts.
    pane: PaneState = active,
  ) {
    return connectThen(c, (loc) => pane.open(path ? { fs: loc.fs, path } : loc), opts);
  }

  /** Connect, asking for what is needed, then run `after` with the start folder. */
  async function connectThen(
    c: ConnectionView,
    after: (start: Location) => Promise<void> | void,
    opts: { secret?: string; remember?: boolean; trust?: string } = {},
  ) {
    notice = null;
    try {
      const r = await api.connect(c.id, opts);
      fsNames[c.fsId] = c.name;
      await loadConnections();
      await after(r.location);
    } catch (e) {
      const kind = errorKind(e);
      const fp = (e as { fingerprint?: string }).fingerprint;
      if (kind === "unknownHost" && fp) {
        dialog = {
          title: `Trust ${c.name}?`,
          message: `Dual Pal has not connected to this server before. Check that its key fingerprint is ${fp} before you continue. It will be added to ~/.ssh/known_hosts.`,
          confirmLabel: "Trust and connect",
          onconfirm: () => connectThen(c, after, { ...opts, trust: fp }),
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
          onconfirm: (secret, remember) => connectThen(c, after, { ...opts, secret, remember }),
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

  // ---- compare, sync and synchronized browsing ------------------------------------

  /** A display name for a file system id. */
  const nameOf = (fs: string) => fsNames[fs] ?? (fs === "local" ? (isMac ? "This Mac" : "This PC") : fs);

  /** Open Compare & Sync for the folders in the two panes. */
  function compareFolders() {
    const left = groups[0].current.location;
    const right = groups[1].current.location;
    if (!left || !right) return;
    if (left.fs === right.fs && left.path === right.path) {
      notice = "Both panes show the same folder. Open the other folder in one of them to compare.";
      return;
    }
    comparing = { left, right, favorite: null };
  }

  /** Open a sync favorite, connecting to its servers first. */
  function openSync(f: SyncFavorite) {
    for (const loc of [f.left, f.right]) {
      const c = connectionOf(loc.fs);
      if (loc.fs !== "local" && !c) {
        notice = `The server of “${f.name}” has been removed.`;
        return;
      }
      if (c && !c.connected) {
        connectThen(c, () => openSync(f));
        return;
      }
    }
    comparing = { left: f.left, right: f.right, favorite: f };
  }

  async function saveSync(f: SyncFavorite) {
    try {
      const saved = await api.saveSyncFavorite(f);
      syncs = await api.syncFavorites();
      return saved;
    } catch (e) {
      notice = errorMessage(e);
      return null;
    }
  }

  function removeSync(f: SyncFavorite) {
    dialog = {
      title: `Remove “${f.name}”?`,
      message: "The sync favorite is removed. No files change.",
      confirmLabel: "Remove",
      onconfirm: async () => {
        await api.removeSyncFavorite(f.id);
        syncs = await api.syncFavorites();
      },
    };
  }

  /** Compare two files in the external compare app: two marked files in the
   *  active pane, or the file under the cursor in each pane, or the file of
   *  the same name in the other pane. */
  async function compareFiles() {
    const loc = active.location;
    const oloc = other.location;
    if (!loc) return;
    const files = (es: Entry[]) => es.filter((e) => e.kind === "file");
    let pair: [Location, Location] | null = null;
    const marked = files(active.targets);
    if (active.selected.size === 2 && marked.length === 2) {
      pair = [
        { fs: loc.fs, path: marked[0].path },
        { fs: loc.fs, path: marked[1].path },
      ];
    } else if (active.current?.kind === "file" && oloc) {
      const twin = other.entries.find((e) => e.name === active.current!.name && e.kind === "file");
      const counterpart = twin ?? (other.current?.kind === "file" ? other.current : undefined);
      if (counterpart) pair = [{ fs: loc.fs, path: active.current.path }, { fs: oloc.fs, path: counterpart.path }];
    }
    if (!pair) {
      notice = "Mark two files, or put the cursor on a file in each pane, to compare them.";
      return;
    }
    // Keep the left pane's file on the left.
    if (activeIndex === 1 && active.selected.size !== 2) pair.reverse();
    await api.compareFiles(pair[0], pair[1]).catch((e) => (notice = errorMessage(e)));
  }

  async function chooseDiffTool() {
    const s = await api.settings();
    diffTools = await api.diffTools();
    const found = diffTools.map((t) => t.name).join(", ");
    dialog = {
      title: "Compare app",
      message: diffTools.length
        ? `Found on this computer: ${found}. The command uses {left} and {right} for the two files. Leave it empty to use ${diffTools[0].name}.`
        : "No compare app was found. Enter the command of one, using {left} and {right} for the two files, e.g. meld {left} {right}.",
      input: s.diffTool ?? diffTools[0]?.command ?? "",
      allowEmpty: true,
      confirmLabel: "Save",
      onconfirm: (cmd) => api.saveSettings({ ...s, diffTool: cmd || null }).catch((e) => (notice = errorMessage(e))),
    };
  }

  async function useDiffTool(t: DiffTool) {
    const s = await api.settings();
    await api.saveSettings({ ...s, diffTool: t.command }).catch((e) => (notice = errorMessage(e)));
  }

  /** Path segments, for comparing folder paths on any system. */
  const segments = (p: string) => p.split(/[\/]/).filter(Boolean);
  let following = false;

  /** Synchronized browsing: repeat a step into or out of a folder in the other pane. */
  async function followNavigation(pane: PaneState, from: Location | null, to: Location) {
    if (!linked || following || !from || from.fs !== to.fs) return;
    const gi = groups.findIndex((g) => g.current === pane);
    if (gi < 0) return;
    const target = groups[1 - gi].current;
    const start = target.location;
    if (!start) return;
    const a = segments(from.path);
    const b = segments(to.path);
    const common = a.findIndex((s, i) => s !== b[i]);
    let next: Location | null = start;
    if (b.length > a.length && (common < 0 || common >= a.length)) {
      // Went into a subfolder: go into the same one.
      let path = start.path;
      for (const seg of b.slice(a.length)) path = join({ fs: start.fs, path }, seg);
      next = { fs: start.fs, path };
    } else if (a.length > b.length && b.every((s, i) => s === a[i])) {
      // Went up: go up as many levels.
      for (let i = 0; i < a.length - b.length && next; i++) next = await api.parentLocation(next);
    } else {
      return; // Jumped elsewhere: the other pane stays.
    }
    if (!next || next.path === start.path) return;
    try {
      const entry = await api.stat(next);
      if (entry.kind !== "dir") throw new Error();
    } catch {
      notice = `The other pane has no folder “${segments(next.path).pop() ?? next.path}”.`;
      return;
    }
    following = true;
    try {
      await target.open(next);
    } finally {
      following = false;
    }
  }

  function toggleLinked() {
    linked = !linked;
    notice = null;
  }

  /** Show the same folder in both panes. */
  function sameFolderInOther() {
    if (active.location) other.open(active.location);
  }

  function swapPanes() {
    const a = groups[0].current.location;
    const b = groups[1].current.location;
    if (!a || !b) return;
    following = true;
    Promise.all([groups[0].current.open(b), groups[1].current.open(a)]).finally(() => (following = false));
  }

  function multiRename() {
    const loc = active.location;
    const items = active.targets;
    if (!loc || !items.length) return;
    const names = new Set(items.map((e) => e.name));
    renaming = { items, folder: loc, others: active.entries.filter((e) => !names.has(e.name)).map((e) => e.name) };
  }

  // ---- command palette -----------------------------------------------------------

  const commands = $derived.by((): Command[] => {
    const c: Command[] = [
      { id: "copy", group: "File", title: "Copy to Other Pane", shortcut: "F5", run: () => transfer("copy") },
      { id: "move", group: "File", title: "Move to Other Pane", shortcut: "F6", run: () => transfer("move") },
      { id: "newFolder", group: "File", title: "New Folder", shortcut: "F7", run: newFolder },
      { id: "rename", group: "File", title: "Rename", shortcut: "F2", run: rename },
      { id: "multiRename", group: "File", title: "Multi Rename…", shortcut: "Shift+F2", run: multiRename },
      { id: "edit", group: "File", title: "Edit in Editor", shortcut: "F4", run: editInEditor },
      { id: "compress", group: "File", title: "Compress into ZIP", run: compress },
      { id: "trash", group: "File", title: "Move to Trash", shortcut: "F8", run: trash },
      { id: "delete", group: "File", title: "Delete Permanently", shortcut: "Shift+Delete", run: remove },
      { id: "compareFolders", group: "Compare", title: "Compare & Sync Folders…", shortcut: `${MOD}Shift+Y`, run: compareFolders },
      { id: "compareFiles", group: "Compare", title: "Compare Files", shortcut: `${MOD}Shift+D`, run: compareFiles },
      { id: "linked", group: "Compare", title: linked ? "Stop Synchronized Browsing" : "Synchronized Browsing", shortcut: `${MOD}Shift+L`, run: toggleLinked },
      { id: "chooseDiff", group: "Settings", title: "Choose Compare App…", run: chooseDiffTool },
      { id: "chooseEditor", group: "Settings", title: "Choose Editor…", run: chooseEditor },
      { id: "selectAll", group: "Select", title: "Select All", shortcut: `${MOD}A`, run: () => active.visible.forEach((it) => active.selected.add(it.path)) },
      { id: "selectPattern", group: "Select", title: "Select by Name…", shortcut: "+", run: () => selectByPattern(true) },
      { id: "deselectPattern", group: "Select", title: "Deselect by Name…", shortcut: "−", run: () => selectByPattern(false) },
      { id: "deselect", group: "Select", title: "Deselect All", run: () => active.selected.clear() },
      { id: "list", group: "View", title: "List View", shortcut: `${MOD}1`, run: () => (active.view = "list") },
      { id: "icons", group: "View", title: "Icon View", shortcut: `${MOD}2`, run: () => (active.view = "icons") },
      { id: "hidden", group: "View", title: active.showHidden ? "Hide Hidden Files" : "Show Hidden Files", shortcut: `${MOD}Shift+.`, run: () => (active.showHidden = !active.showHidden) },
      { id: "info", group: "View", title: showInspector ? "Hide Info Panel" : "Show Info Panel", shortcut: `${MOD}I`, run: () => (showInspector = !showInspector) },
      { id: "refresh", group: "View", title: "Refresh", shortcut: `${MOD}R`, run: () => active.refresh() },
      { id: "newTab", group: "View", title: "New Tab", shortcut: `${MOD}T`, run: () => groups[activeIndex].newTab() },
      { id: "closeTab", group: "View", title: "Close Tab", shortcut: `${MOD}W`, run: () => groups[activeIndex].closeTab() },
      { id: "switch", group: "View", title: "Switch to Other Pane", shortcut: "Tab", run: () => (activeIndex = 1 - activeIndex) },
      { id: "swap", group: "View", title: "Swap Panes", run: swapPanes },
      { id: "same", group: "View", title: "Same Folder in Other Pane", run: sameFolderInOther },
      { id: "favorite", group: "Go", title: isFavorite ? "Remove Folder from Favorites" : "Add Folder to Favorites", run: toggleFavorite },
      { id: "back", group: "Go", title: "Back", run: () => active.back() },
      { id: "forward", group: "Go", title: "Forward", run: () => active.forward() },
      { id: "up", group: "Go", title: "Enclosing Folder", shortcut: `${MOD}↑`, run: () => active.up() },
      { id: "insights", group: "Activity", title: "Activity Insights…", run: () => (showInsights = true) },
      { id: "clearOps", group: "Activity", title: "Clear Finished Operations", run: clearFinished },
      { id: "addServer", group: "Servers", title: "Add Server…", run: () => (editing = null) },
    ];
    for (const t of diffTools) c.push({ id: `diff:${t.name}`, group: "Settings", title: `Compare Files with ${t.name}`, run: () => useDiffTool(t) });
    for (const v of volumes) c.push({ id: `vol:${v.location.path}`, group: "Go", title: `Go to ${v.name}`, run: () => active.open(v.location) });
    for (const f of favorites) c.push({ id: `fav:${f.location.fs}:${f.location.path}`, group: "Go", title: `Go to ${f.name}`, run: () => active.open(f.location) });
    for (const s of connections) {
      c.push({ id: `conn:${s.id}`, group: "Servers", title: `Connect to ${s.name}`, run: () => connectTo(s) });
      for (const f of s.favorites) c.push({ id: `conn:${s.id}:${f.path}`, group: "Servers", title: `Go to ${f.name} on ${s.name}`, run: () => connectTo(s, f.path) });
      if (s.connected) c.push({ id: `disc:${s.id}`, group: "Servers", title: `Disconnect ${s.name}`, run: () => disconnect(s) });
    }
    for (const f of syncs) c.push({ id: `sync:${f.id}`, group: "Sync", title: `Sync ${f.name}`, run: () => openSync(f) });
    return c;
  });

  // ---- editing --------------------------------------------------------------

  function stopEditing(id: string) {
    const s = editSessions.find((x) => x.id === id);
    if (s?.status !== "failed") {
      api.stopEditSession(id);
      return;
    }
    dialog = {
      title: "Discard unsent changes?",
      message: `The last save of ${s.remote.path} has not reached the server. Stopping deletes the local copy with those changes.`,
      confirmLabel: "Discard",
      danger: true,
      onconfirm: () => api.stopEditSession(id),
    };
  }

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
      allowEmpty: true,
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
    // A dialog, window or the server form is open: its keys are its own.
    if (dialog || editing !== undefined || comparing || renaming || showPalette || showInsights) return;
    const cmd = e.metaKey || e.ctrlKey;
    if (cmd && e.shiftKey && e.key.toLowerCase() === "p") {
      showPalette = true;
      return e.preventDefault();
    }
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
        if (e.shiftKey) multiRename();
        else rename();
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
    if (mod && e.shiftKey && e.key.toLowerCase() === "y") {
      compareFolders();
      return handled();
    }
    if (mod && e.shiftKey && e.key.toLowerCase() === "d") {
      compareFiles();
      return handled();
    }
    if (mod && e.shiftKey && e.key.toLowerCase() === "l") {
      toggleLinked();
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
      <button onclick={() => transfer("copy")} aria-label="Copy" title="Copy to the other pane (F5)"><Icon name="copy" /></button>
      <button onclick={() => transfer("move")} aria-label="Move" title="Move to the other pane (F6)"><Icon name="move" /></button>
    </div>
    <div class="group">
      <button onclick={newFolder} aria-label="New Folder" title="New folder (F7)"><Icon name="folder-plus" /></button>
      <button onclick={rename} aria-label="Rename" title="Rename (F2)"><Icon name="rename" /></button>
      <button onclick={multiRename} aria-label="Multi Rename" title="Multi Rename: rename the marked items with rules and presets (Shift F2)"
        ><Icon name="multi-rename" /></button
      >
      <button onclick={editInEditor} aria-label="Edit" title="Edit in your preferred editor (F4). Files on servers upload again when you save."
        ><Icon name="edit" /></button
      >
      <button onclick={compress} aria-label="Compress" title="Compress into a ZIP archive"><Icon name="archive" /></button>
      <button onclick={trash} aria-label="Trash" title="Move to Trash (F8 / Delete). Shift+Delete deletes permanently."><Icon name="trash" /></button>
    </div>
    <div class="group">
      <button onclick={compareFolders} aria-label="Compare & Sync" title="Compare & Sync: compare the two panes' folders and sync them ({MOD}Shift Y)"
        ><Icon name="sync" /></button
      >
      <button onclick={compareFiles} aria-label="Compare Files" title="Compare Files: open two files in your compare app ({MOD}Shift D)"
        ><Icon name="compare-files" /></button
      >
      <button
        aria-pressed={linked}
        onclick={toggleLinked}
        aria-label="Linked browsing"
        title="Linked browsing: going into or out of a folder in one pane does the same in the other ({MOD}Shift L)"><Icon name="link" /></button
      >
    </div>
    <div class="group" role="group" aria-label="View">
      <button aria-pressed={active.view === "list"} onclick={() => (active.view = "list")} aria-label="List view" title="List view (Ctrl/⌘ 1)"
        ><Icon name="list" /></button
      >
      <button aria-pressed={active.view === "icons"} onclick={() => (active.view = "icons")} aria-label="Icon view" title="Icon view (Ctrl/⌘ 2)"
        ><Icon name="grid" /></button
      >
    </div>
    <div class="group">
      <button onclick={() => selectByPattern(true)} aria-label="Select" title="Select by name or extension (+). Deselect with −."><Icon name="select" /></button>
      <button
        aria-pressed={isFavorite}
        onclick={toggleFavorite}
        aria-label="Favorite"
        title={isFavorite ? "Remove folder from Favorites" : "Add folder to Favorites"}><Icon name="star" filled={isFavorite} /></button
      >
      <button onclick={() => groups[activeIndex].newTab()} aria-label="New Tab" title="New tab (Ctrl/⌘ T)"><Icon name="new-tab" /></button>
      <button aria-pressed={showInspector} onclick={() => (showInspector = !showInspector)} aria-label="Info" title="Show preview and info (Ctrl/⌘ I)"
        ><Icon name="info" /></button
      >
    </div>
    <div class="group">
      <button onclick={() => active.refresh()} aria-label="Refresh" title="Refresh (Ctrl/⌘ R)"><Icon name="refresh" /></button>
      <button onclick={chooseEditor} aria-label="Editor" title="Choose the editor used by Edit"><Icon name="file-code" /></button>
      <button onclick={() => (showPalette = true)} aria-label="Commands" title="Find any command ({MOD}Shift P)"><Icon name="command" /></button>
      <button
        aria-pressed={active.showHidden}
        onclick={() => (active.showHidden = !active.showHidden)}
        aria-label="Hidden files"
        title="Show hidden files (Ctrl/⌘ Shift .)"><Icon name="eye" /></button
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
      {syncs}
      onopensync={openSync}
      onremovesync={removeSync}
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
    <EditSessions
    sessions={editSessions}
    onstop={stopEditing}
    onupload={(id) => api.uploadEditSession(id).catch((e) => (notice = errorMessage(e)))}
  />
    <div class="ops">
      <OperationsPanel
        {operations}
        {activity}
        oncancel={(id) => api.cancel(id)}
        onretry={retry}
        onclear={clearFinished}
        onclearactivity={clearActivity}
        oninsights={() => (showInsights = true)}
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
    allowEmpty={d.allowEmpty}
    oncancel={() => (dialog = null)}
    onconfirm={(v, checked) => {
      // Read the callback before clearing: `d` is derived from `dialog`.
      const confirm = d.onconfirm;
      dialog = null;
      confirm(v, checked);
    }}
  />
{/if}

{#if comparing}
  <CompareView
    left={comparing.left}
    right={comparing.right}
    favorite={comparing.favorite}
    {nameOf}
    onclose={() => (comparing = null)}
    onsavefavorite={saveSync}
    oncomparefiles={(l, r) => api.compareFiles(l, r)}
  />
{/if}

{#if renaming}
  <MultiRename
    items={renaming.items}
    folder={renaming.folder}
    others={renaming.others}
    onclose={() => (renaming = null)}
    onrename={(items) => {
      renaming = null;
      enqueue({ type: "batchRename", items });
    }}
  />
{/if}

{#if showPalette}
  <CommandPalette {commands} onclose={() => (showPalette = false)} />
{/if}

{#if showInsights}
  <Insights {nameOf} onclose={() => (showInsights = false)} />
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
    /* Narrow windows: groups wrap onto a second row instead of being cut off. */
    flex-wrap: wrap;
    row-gap: 6px;
    padding: 8px 12px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
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
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px 9px;
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
