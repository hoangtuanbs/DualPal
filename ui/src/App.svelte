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
    type LiveStatus,
    type Entry,
    type SyncFavorite,
  } from "./lib/api";
  import Dialog from "./lib/Dialog.svelte";
  import OperationsPanel from "./lib/OperationsPanel.svelte";
  import Pane from "./lib/Pane.svelte";
  import Inspector from "./lib/Inspector.svelte";
  import ContextMenu, { type MenuItem } from "./lib/ContextMenu.svelte";
  import { drag, fromKeyboard, type DropTarget } from "./lib/drag.svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { copyText } from "./lib/about";
  import Sidebar from "./lib/Sidebar.svelte";
  import ConnectionDialog from "./lib/ConnectionDialog.svelte";
  import ImportDialog from "./lib/ImportDialog.svelte";
  import EditSessions from "./lib/EditSessions.svelte";
  import { fsNames } from "./lib/fs-names.svelte";
  import { patternMatcher } from "./lib/fileTypes";
  import { PaneGroup, PaneState } from "./lib/pane-state.svelte";
  import CompareView from "./lib/CompareView.svelte";
  import MultiRename from "./lib/MultiRename.svelte";
  import CommandPalette, { type Command } from "./lib/CommandPalette.svelte";
  import FindDialog from "./lib/FindDialog.svelte";
  import Insights from "./lib/Insights.svelte";
  import Icon from "./lib/Icon.svelte";
  import AboutDialog from "./lib/AboutDialog.svelte";
  import { emailSupport, openProjectPage } from "./lib/about";
  import UpdateBar from "./lib/UpdateBar.svelte";
  import { updates } from "./lib/updates.svelte";
  import { UPDATE_CHECK_DELAY_MS, UPDATE_RECHECK_POLL_MS, activeOperationCount } from "./lib/updates";
  import { SPLIT_DEFAULT, clampSplit, loadSplit, saveSplit, splitAt, splitForKey } from "./lib/split";

  const groups = [new PaneGroup(new PaneState()), new PaneGroup(new PaneState())];
  let activeIndex = $state(0);
  const active = $derived(groups[activeIndex].current);
  const other = $derived(groups[1 - activeIndex].current);
  /** Every open tab, for refreshing after operations. */
  const allPanes = () => groups.flatMap((g) => g.tabs);

  let showInspector = $state(true);
  /** The left pane's share of the width; the divider between the panes moves it. */
  let split = $state(loadSplit());
  let panesEl = $state<HTMLElement>();
  let splitterEl = $state<HTMLElement>();
  let panesWidth = $state(0);
  /** The split as drawn: the saved one, kept within the minimum pane width as the window or Info panel changes. */
  const shownSplit = $derived(clampSplit(split, panesWidth));
  let resizing = $state(false);

  function splitDown(e: PointerEvent) {
    if (e.button !== 0 || !panesEl) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    resizing = true;
  }
  function splitMove(e: PointerEvent) {
    if (!resizing || !panesEl) return;
    const box = panesEl.getBoundingClientRect();
    split = splitAt(e.clientX, box.left, box.width);
  }
  function splitUp(e: PointerEvent) {
    if (!resizing) return;
    resizing = false;
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    saveSplit(split);
  }
  function splitKey(e: KeyboardEvent) {
    // Tab and Esc hand the keyboard back to the panes. Tab switches panes
    // everywhere else, so without this focus would stay on the divider.
    if (e.key === "Tab" || e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      splitterEl?.blur();
      return;
    }
    const next = splitForKey(shownSplit, e.key, e.shiftKey, panesWidth);
    if (next === null) return;
    // The divider's own keys: the panes' arrow-key handling must not see them.
    e.preventDefault();
    e.stopPropagation();
    split = next;
    saveSplit(split);
  }
  function splitReset() {
    split = SPLIT_DEFAULT;
    saveSplit(split);
  }
  /** The open right-click menu. */
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let volumes = $state<Volume[]>([]);
  let favorites = $state<Favorite[]>([]);
  let connections = $state<ConnectionView[]>([]);
  let editSessions = $state<EditSession[]>([]);
  /** The connection being added (null) or edited; undefined when closed. */
  let editing = $state<Connection | null | undefined>(undefined);
  /** The "Import servers" dialog, opened from the server form. */
  let importing = $state(false);

  let operations = $state<OperationInfo[]>([]);
  // An update waits for queued and running file operations, and for saves
  // of files edited on a server that are still uploading.
  updates.setBusyCheck(
    () => activeOperationCount(operations) + editSessions.filter((s) => s.status === "uploading").length,
  );
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
  let showFind = $state(false);
  let showInsights = $state(false);
  let aboutPage = $state<"about" | "donate" | null>(null);
  /** Synchronized browsing: entering or leaving a folder in one pane does the same in the other. */
  let linked = $state(false);
  let diffTools = $state<DiffTool[]>([]);
  /** Auto-sync, while it runs. The panes are frozen meanwhile. */
  let live = $state<LiveStatus | null>(null);
  let liveKind = $state<LiveKind | null>(null);
  const frozen = $derived(live !== null);
  const isMac = navigator.userAgent.includes("Mac");
  const MOD = isMac ? "⌘" : "Ctrl+";

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let unlistenEdits: (() => void) | undefined;
    let unlistenLive: (() => void) | undefined;
    // PREP-06: a deterministic "app ready" signal for E2E tests, after both
    // panes finish their first listing.
    let resolveReady: (() => void) | undefined;
    if (import.meta.env.MODE === "test" || (typeof localStorage !== "undefined" && localStorage.getItem("dualpalTest") === "1")) {
      window.__DUALPAL_TEST__ = { ready: new Promise<void>((r) => (resolveReady = r)) };
    }
    (async () => {
      unlisten = await api.onOperationUpdated(onOperationUpdated);
      unlistenLive = await api.onLiveSyncUpdated(onLiveSyncUpdated);
      unlistenEdits = await api.onEditSessionUpdated(onEditSessionUpdated);
      await loadConnections();
      editSessions = await api.editSessions();
      const home = await api.homeLocation();
      // Auto-sync left running when the window reloaded: show its folders,
      // the source on the left, before the panes are locked.
      const running = await api.liveSyncStatus();
      const start = running ? [running.from, running.to] : [home, home];
      await Promise.all(
        groups.map(async (g, i) => {
          await g.current.open(start[i]);
          if (!g.current.location) await g.current.open(home);
        }),
      );
      operations = await api.operations();
      activity = await api.activity();
      favorites = await api.favorites();
      volumes = await api.volumes();
      syncs = await api.syncFavorites();
      diffTools = await api.diffTools();
      if (running) onLiveSyncUpdated(await api.liveSyncStatus());
    })().catch((e) => (notice = errorMessage(e))).finally(() => resolveReady?.());
    PaneState.navigated = followNavigation;
    // Pick up mounted/unmounted drives and changing free space.
    const timer = setInterval(() => api.volumes().then((v) => (volumes = v)), 15000);
    // The automatic-check switch, so the palette names it correctly from the start.
    updates.loadPreferences().catch(() => {});
    // Look for a new release shortly after start, then hourly whether a daily
    // check is due, so a window left open still hears about one (not in E2E tests).
    const updateTimer = window.__DUALPAL_TEST__ ? undefined : setTimeout(() => updates.checkAutomatically(), UPDATE_CHECK_DELAY_MS);
    const updatePoll = window.__DUALPAL_TEST__ ? undefined : setInterval(() => updates.checkAutomatically(), UPDATE_RECHECK_POLL_MS);
    return () => {
      unlisten?.();
      unlistenEdits?.();
      unlistenLive?.();
      clearInterval(timer);
      clearTimeout(updateTimer);
      clearInterval(updatePoll);
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

  // ---- auto-sync ------------------------------------------------------------------

  type LiveKind = "right" | "left" | "both";

  /** The folders auto-sync copies from and to: → copies the left pane to
   *  the right, ← the right pane to the left. */
  function liveSides(kind: LiveKind): { from: Location; to: Location } | null {
    const l = groups[0].current.location;
    const r = groups[1].current.location;
    if (!l || !r) return null;
    return kind === "left" ? { from: r, to: l } : { from: l, to: r };
  }

  async function toggleLive(kind: LiveKind) {
    if (liveKind === kind) {
      await api.liveSyncStop().catch((e) => (notice = errorMessage(e)));
      return;
    }
    const sides = liveSides(kind);
    if (!sides) return;
    const { from, to } = sides;
    const arrow = kind === "both" ? "⇄" : kind === "left" ? "←" : "→";
    const [l, r] = kind === "left" ? [to, from] : [from, to];
    const full = (loc: Location) => (loc.fs === "local" ? loc.path : `${loc.path} on ${nameOf(loc.fs)}`);
    dialog = {
      title: `Start auto-sync ${arrow}?`,
      message:
        `${placeName(l)} ${arrow} ${placeName(r)}\n\n` +
        (kind === "both"
          ? `Changes in ${full(l)} and ${full(r)} are copied to the other folder, and files deleted in one are deleted in the other. `
          : `New and changed files in ${full(from)} are copied to ${full(to)}, replacing older versions there. Nothing is deleted. `) +
        "The panes stay locked until you stop it.",
      confirmLabel: "Start",
      danger: kind === "both",
      onconfirm: () => startLive(kind, from, to),
    };
  }

  async function startLive(kind: LiveKind, from: Location, to: Location) {
    const sides = { from, to };
    try {
      notice = null;
      const status = await api.liveSyncStart(sides.from, sides.to, kind === "both" ? "both" : "oneWay");
      liveKind = kind;
      onLiveSyncUpdated(status);
    } catch (e) {
      notice = errorMessage(e);
    }
  }

  let lastSyncs = 0;
  function onLiveSyncUpdated(s: LiveStatus | null) {
    live = s;
    if (!s) {
      liveKind = null;
      lastSyncs = 0;
      return;
    }
    if (!liveKind) {
      const l = groups[0].current.location;
      liveKind = s.direction === "both" ? "both" : l && sameLoc(l, s.to) ? "left" : "right";
    }
    // Show what a sync changed.
    if (s.syncs !== lastSyncs) {
      lastSyncs = s.syncs;
      allPanes().forEach((p) => p.refresh());
    }
  }

  const sameLoc = (a: Location, b: Location) => a.fs === b.fs && a.path === b.path;

  /** A folder's name for the auto-sync bar, with its server's name. */
  function placeName(loc: Location) {
    const name = loc.path.split(/[\\/]/).filter(Boolean).pop() ?? loc.path;
    return loc.fs === "local" ? name || loc.path : `${name || "/"} on ${nameOf(loc.fs)}`;
  }

  /** The synced folders as they sit in the panes, and the arrow between them. */
  const liveLeft = $derived(live ? (liveKind === "left" ? live.to : live.from) : null);
  const liveRight = $derived(live ? (liveKind === "left" ? live.from : live.to) : null);
  const liveArrow = $derived(liveKind === "both" ? "⇄" : liveKind === "left" ? "←" : "→");

  const liveText = $derived.by(() => {
    if (!live) return "";
    if (live.state === "syncing") return "Syncing…";
    if (live.state === "failed") return `Failed: ${live.error ?? "unknown error"}. Trying again shortly.`;
    const time = live.lastSyncMs ? new Date(live.lastSyncMs).toLocaleTimeString() : "";
    const how =
      live.fromWatched && live.toWatched ? "watching for changes" : "watching here, checking the server every 10 s";
    const parts = [`Up to date at ${time}`, how];
    if (live.itemsSynced) parts.push(`${live.itemsSynced} ${live.itemsSynced === 1 ? "item" : "items"} synced`);
    if (live.conflicts) parts.push(`${live.conflicts} changed on both sides, left alone`);
    return parts.join(" · ");
  });

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

  /** Servers with a connection attempt in flight, shown as "Connecting…". */
  let connecting = $state<Record<string, boolean>>({});
  /** The connection attempt under way for each server, resolving to its start folder. */
  const inFlight = new Map<string, Promise<Location | null>>();

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
    // Already connecting: a second handshake would stack up password prompts,
    // so wait for the one under way and then run this request's own step.
    const waiting = inFlight.get(c.id);
    if (waiting) {
      const start = await waiting;
      if (start) await Promise.resolve(after(start)).catch((e) => (notice = errorMessage(e)));
      return;
    }
    notice = null;
    connecting[c.id] = true;
    let settle: (start: Location | null) => void = () => {};
    inFlight.set(c.id, new Promise((resolve) => (settle = resolve)));
    try {
      const r = await api.connect(c.id, opts);
      fsNames[c.fsId] = c.name;
      await loadConnections();
      settle(r.location);
      await after(r.location);
    } catch (e) {
      const kind = errorKind(e);
      const fp = (e as { fingerprint?: string }).fingerprint;
      if (kind === "unknownHost" && fp) {
        dialog = {
          title: `Trust ${c.name}?`,
          message:
            c.endpoint.type === "ftp"
              ? `This server's certificate is not signed by an authority your computer trusts, which is normal for routers and NAS boxes. Check that its fingerprint is\n\n${fp}\n\nbefore you continue. Dual Pal will remember it for this server.`
              : `Dual Pal has not connected to this server before. Check that its key fingerprint is ${fp} before you continue. It will be added to ~/.ssh/known_hosts.`,
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
      } else if (kind === "authFailed" && (c.endpoint.type === "ftp" || c.endpoint.type === "webDav") && !c.endpoint.user.trim()) {
        // No user name: a password alone cannot sign in, so ask for both.
        dialog = {
          title: `Sign in to ${c.name}`,
          message:
            c.endpoint.type === "ftp"
              ? "This server does not allow anonymous sign-in. Add your user name and password to the server."
              : "This server needs a user name and password. Add them to the server.",
          confirmLabel: "Edit Server",
          onconfirm: () => (editing = c),
        };
      } else if (kind === "authFailed" && (c.endpoint.type === "ftp" || c.endpoint.type === "webDav")) {
        const who = c.endpoint.type === "ftp" ? `${c.endpoint.user}@${c.endpoint.host}` : `${c.endpoint.user} at ${c.endpoint.url}`;
        dialog = {
          title: `Sign in to ${c.name}`,
          message: `${opts.secret ? "That password did not work. " : ""}Enter the password for ${who}.`,
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
    } finally {
      delete connecting[c.id];
      inFlight.delete(c.id);
      // Requests that waited on a failed attempt do nothing; this one shows the error.
      settle(null);
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
      { id: "terminal", group: "File", title: "Open Terminal Here", shortcut: `${MOD}Shift+T`, run: openTerminal },
      { id: "compress", group: "File", title: "Compress into ZIP", run: compress },
      { id: "find", group: "File", title: "Find files by name…", shortcut: `${MOD}Shift+F`, run: () => (showFind = true) },
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
      { id: "filter", group: "View", title: "Filter Items by Name", shortcut: `${MOD}F`, run: () => active.openFilter() },
      { id: "refresh", group: "View", title: "Refresh", shortcut: `${MOD}R`, run: () => active.refresh() },
      { id: "folderSizes", group: "View", title: "Calculate Folder Sizes", shortcut: "Alt+Shift+Enter", run: () => active.calculateSizes() },
      { id: "newTab", group: "View", title: "New Tab", shortcut: `${MOD}T`, run: () => groups[activeIndex].newTab() },
      { id: "closeTab", group: "View", title: "Close Tab", shortcut: `${MOD}W`, run: () => groups[activeIndex].closeTab() },
      { id: "switch", group: "View", title: "Switch to Other Pane", shortcut: "Tab", run: () => (activeIndex = 1 - activeIndex) },
      { id: "swap", group: "View", title: "Swap Panes", run: swapPanes },
      { id: "resizePanes", group: "View", title: "Resize Panes (← → to move, Tab when done)", run: () => splitterEl?.focus() },
      { id: "evenPanes", group: "View", title: "Make Panes Equal Width", run: splitReset },
      { id: "same", group: "View", title: "Same Folder in Other Pane", run: sameFolderInOther },
      { id: "favorite", group: "Go", title: isFavorite ? "Remove Folder from Favorites" : "Add Folder to Favorites", run: toggleFavorite },
      { id: "back", group: "Go", title: "Back", run: () => active.back() },
      { id: "forward", group: "Go", title: "Forward", run: () => active.forward() },
      { id: "up", group: "Go", title: "Enclosing Folder", shortcut: `${MOD}↑`, run: () => active.up() },
      { id: "insights", group: "Activity", title: "Activity Insights…", run: () => (showInsights = true) },
      { id: "about", group: "Help", title: "About Dual Pal", run: () => (aboutPage = "about") },
      { id: "donate", group: "Help", title: "Donate…", run: () => (aboutPage = "donate") },
      { id: "email", group: "Help", title: "Email Us…", run: () => emailSupport().catch((e) => (notice = errorMessage(e))) },
      { id: "github", group: "Help", title: "Dual Pal on GitHub", run: () => openProjectPage().catch((e) => (notice = errorMessage(e))) },
      { id: "updates", group: "Help", title: "Check for Updates…", run: () => updates.checkNow().catch(() => {}) },
      {
        id: "autoUpdates",
        group: "Settings",
        title: updates.autoCheck ? "Stop Checking for Updates Automatically" : "Check for Updates Automatically",
        run: () => updates.setAutoCheck(!updates.autoCheck).catch((e) => (notice = errorMessage(e))),
      },
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

  /** Open the system terminal in the active pane's folder. */
  async function openTerminal() {
    const loc = active.location;
    if (!loc) return;
    if (loc.fs !== "local") {
      notice = "Terminal opens in local folders only. Open a folder on this computer first.";
      return;
    }
    await api.openTerminal(loc).catch((e) => (notice = errorMessage(e)));
  }

  /** Open the marked files, or the file under the cursor, in the preferred editor. */
  async function editInEditor() {
    await openFiles(true);
  }

  /** Open the marked files (or the one under the cursor) in their apps, or
   *  in the preferred editor. */
  async function openFiles(editor: boolean) {
    const loc = active.location;
    const files = active.targets.filter((e) => e.kind === "file");
    if (!loc || !files.length) return;
    if (files.length > 20) {
      notice = `Open at most 20 files at once (${files.length} are marked).`;
      return;
    }
    for (const f of files) {
      try {
        await api.openFile({ fs: loc.fs, path: f.path }, editor);
      } catch (e) {
        notice = errorMessage(e);
      }
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
    // Servers, buckets and Windows network or WSL folders have no Trash: offer to delete instead.
    if (active.location?.fs !== "local") remove();
    else if (isWindowsShare(active.location.path)) remove("This location has no Recycle Bin.");
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

  /** \\server\share\ paths, including WSL's \\wsl.localhost\<distro>\. */
  function isWindowsShare(path: string) {
    return path.startsWith("\\\\");
  }

  function remove(why?: string) {
    const targets = locationsOf(active);
    if (!targets.length) return;
    dialog = {
      title: `Delete ${describe(active)}?`,
      message: `${why ? why + " " : ""}This permanently deletes the selected items. It cannot be undone.`,
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
    // Several marked items: rename them together.
    if (active.selected.size > 1) return multiRename();
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

  // ---- right-click menu and drag and drop -------------------------------------------

  function openMenu(x: number, y: number, entry: Entry | null) {
    menu = { x, y, items: entry ? entryMenu(entry) : folderMenu() };
  }

  /** The menu for the marked items, or the one right-clicked. */
  function entryMenu(entry: Entry): MenuItem[] {
    const t = active.targets;
    const n = t.length;
    const files = t.filter((e) => e.kind === "file");
    const dest = other.location ? `“${other.title || other.location.path}”` : "the other pane";
    const what = n === 1 ? "" : ` ${n} Items`;
    const single = n === 1 ? t[0] : null;
    return [
      single
        ? {
            label: "Open",
            icon: single.kind === "dir" ? "folder-open" : undefined,
            shortcut: "Enter",
            run: () => active.activate(single),
          }
        : { label: `Open ${files.length} Files`, disabled: !files.length, run: () => openFiles(false) },
      ...(single?.kind === "dir" && active.location
        ? [{ label: "Open in New Tab", icon: "new-tab", run: () => openInNewTab(single) }]
        : []),
      {
        label: files.length > 1 ? `Edit ${files.length} Files` : "Edit in Editor",
        icon: "edit",
        shortcut: "F4",
        disabled: !files.length,
        run: editInEditor,
      },
      "separator",
      { label: `Copy${what} to ${dest}`, icon: "copy", shortcut: "F5", disabled: !other.location, run: () => transfer("copy") },
      { label: `Move${what} to ${dest}`, icon: "move", shortcut: "F6", disabled: !other.location, run: () => transfer("move") },
      "separator",
      single
        ? { label: "Rename…", icon: "rename", shortcut: "F2", run: rename }
        : { label: `Rename${what}…`, icon: "multi-rename", shortcut: "Shift+F2", run: multiRename },
      { label: "Compress into ZIP", icon: "archive", run: compress },
      {
        label: n === 1 ? "Copy Path" : `Copy ${n} Paths`,
        icon: "clipboard",
        run: async () => {
          if (!(await copyText(t.map((e) => e.path).join("\n")))) notice = "Could not copy to the clipboard.";
        },
      },
      ...(n === 2 && files.length === 2
        ? [{ label: "Compare Files", icon: "compare-files", shortcut: `${MOD}Shift+D`, run: compareFiles }]
        : []),
      ...(single ? [{ label: "Show Info", icon: "info", shortcut: `${MOD}I`, run: () => (showInspector = true) }] : []),
      "separator",
      { label: `Move${what} to Trash`, icon: "trash", shortcut: "F8", run: trash },
      { label: `Delete${what} Permanently…`, shortcut: "Shift+Del", danger: true, run: () => remove() },
    ];
  }

  /** The menu for empty space in a pane. */
  function folderMenu(): MenuItem[] {
    return [
      { label: "New Folder…", icon: "folder-plus", shortcut: "F7", run: newFolder },
      { label: "Refresh", icon: "refresh", shortcut: `${MOD}R`, run: () => active.refresh() },
      "separator",
      { label: "Select All", icon: "select", shortcut: `${MOD}A`, run: () => active.visible.forEach((it) => active.selected.add(it.path)) },
      { label: "Select by Name…", shortcut: "+", run: () => selectByPattern(true) },
      {
        label: active.showHidden ? "Hide Hidden Files" : "Show Hidden Files",
        icon: "eye",
        shortcut: `${MOD}Shift+.`,
        run: () => (active.showHidden = !active.showHidden),
      },
      "separator",
      { label: "Compare & Sync Folders…", icon: "sync", shortcut: `${MOD}Shift+Y`, run: compareFolders },
      { label: isFavorite ? "Remove from Favorites" : "Add to Favorites", icon: "star", run: toggleFavorite },
    ];
  }

  function openInNewTab(entry: Entry) {
    const loc = active.location;
    if (!loc) return;
    // Only the chosen folder loads, so no slower listing can replace it.
    groups[activeIndex].newTab(active.clone({ fs: loc.fs, path: entry.path }));
  }

  /** The keyboard's menu key, or Shift+F10: the menu for the cursor row. */
  function menuFromKeyboard() {
    const row = document.querySelector<HTMLElement>(`[data-pane="${activeIndex}"] [data-index="${active.cursor}"]`);
    const r = row?.getBoundingClientRect();
    if (!r || !active.current) return;
    if (!active.selected.has(active.current.path)) active.selected.clear();
    openMenu(r.left + 32, r.bottom, active.current);
  }

  /** The folder under a point: a folder row in a pane, or the pane's folder. */
  function folderAt(x: number, y: number): DropTarget | null {
    const el = document.elementFromPoint(x, y);
    const paneEl = el?.closest<HTMLElement>("[data-pane]");
    const fs = paneEl?.dataset.fs;
    const path = paneEl?.dataset.path;
    if (!paneEl || !fs || path == null) return null;
    const pane = Number(paneEl.dataset.pane);
    const row = el?.closest<HTMLElement>("[data-dir]");
    if (row?.dataset.dir) return { pane, folder: { fs, path: row.dataset.dir }, name: row.dataset.name ?? "", row: row.dataset.dir };
    return { pane, folder: { fs, path }, name: paneEl.dataset.title || path, row: null };
  }

  // Files dragged in from Explorer or Finder: copy them where they land.
  $effect(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    const at = (p: { x: number; y: number }) => folderAt(p.x / devicePixelRatio, p.y / devicePixelRatio);
    getCurrentWebview()
      .onDragDropEvent(({ payload }) => {
        if (payload.type === "enter" || payload.type === "over") {
          drag.target = at(payload.position);
        } else if (payload.type === "drop") {
          const target = at(payload.position);
          drag.target = null;
          if (target && payload.paths.length) {
            enqueue({
              type: "copy",
              sources: payload.paths.map((path) => ({ fs: "local", path })),
              destination: target.folder,
              conflict: "fail",
            });
          }
        } else {
          drag.target = null;
        }
      })
      .then((u) => (gone ? u() : (unlisten = u)))
      .catch(() => {}); // not running inside the app (e.g. a plain browser)
    return () => {
      gone = true;
      unlisten?.();
    };
  });

  /** Items dragged from a pane and let go over a folder. */
  function dropped(items: Entry[], from: Location, target: DropTarget, move: boolean) {
    if (!items.length) return;
    enqueue({
      type: move ? "move" : "copy",
      sources: items.map((e) => ({ fs: from.fs, path: e.path })),
      destination: target.folder,
      conflict: "fail",
    });
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

  /** Shift+↑/↓: mark the items the cursor passes over. */
  function extendSelection(delta: number) {
    if (active.current) active.selected.add(active.current.path);
    active.moveCursor(delta);
    if (active.current) active.selected.add(active.current.path);
  }

  function onkeydown(e: KeyboardEvent) {
    // A dialog, window or the server form is open: its keys are its own.
    if (frozen || menu || dialog || editing !== undefined || importing || comparing || renaming || showPalette || showFind || showInsights || aboutPage) return;
    const cmd = e.metaKey || e.ctrlKey;
    if (cmd && e.shiftKey && e.key.toLowerCase() === "p") {
      showPalette = true;
      return e.preventDefault();
    }
    // Find files by name below the active folder.
    if (cmd && e.shiftKey && e.key.toLowerCase() === "f") {
      if (active.location) showFind = true;
      return e.preventDefault();
    }
    // Not from the quick editor, where the key belongs to the text being edited.
    if (cmd && !e.shiftKey && e.key.toLowerCase() === "f" && (e.target as HTMLElement | null)?.tagName !== "TEXTAREA") {
      active.openFilter();
      return e.preventDefault();
    }
    // Typing in the quick editor or another field: leave keys alone.
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
    const mod = e.metaKey || e.ctrlKey;
    const handled = () => e.preventDefault();
    const group = groups[activeIndex];
    const icons = active.view === "icons";
    if (e.key === "Escape" && active.filterOpen) {
      active.closeFilter();
      return handled();
    }
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
    if (e.altKey && e.shiftKey && e.key === "Enter") {
      active.calculateSizes();
      return handled();
    }
    switch (e.key) {
      case "Tab":
        activeIndex = 1 - activeIndex;
        return handled();
      case "ArrowDown":
        if (e.shiftKey) extendSelection(icons ? active.columns : 1);
        else active.moveCursor(icons ? active.columns : 1);
        return handled();
      case "ArrowUp":
        if (mod) active.up();
        else if (e.shiftKey) extendSelection(icons ? -active.columns : -1);
        else active.moveCursor(icons ? -active.columns : -1);
        return handled();
      case "ContextMenu":
        menuFromKeyboard();
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
        if (active.current) {
          active.toggle(active.current);
          // Space also works out the size of a folder, as in Total Commander.
          if (e.key === " ") active.calculateSizes([active.current]);
        }
        active.moveCursor(1);
        return handled();
      case "F2":
        if (e.shiftKey) multiRename();
        else rename();
        return handled();
      case "F10":
        if (!e.shiftKey) return;
        menuFromKeyboard();
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
    if (mod && e.shiftKey && e.key.toLowerCase() === "t") {
      openTerminal();
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

<svelte:window
  {onkeydown}
  oncontextmenu={(e) => {
    // No browser menu (Reload, Inspect…) outside text fields.
    const t = e.target as HTMLElement | null;
    if (t?.closest("input, textarea, [contenteditable]")) return;
    e.preventDefault();
    // The Menu key, which WebView2 delivers only as this event.
    if (fromKeyboard(e) && !menu && !dialog && !frozen) menuFromKeyboard();
  }}
/>

<div class="app">
  <div class="toolbar" data-testid="toolbar">
    <div class="group" inert={frozen}>
      <button onclick={() => transfer("copy")} aria-label="Copy" title="Copy to the other pane (F5)"><Icon name="copy" /></button>
      <button onclick={() => transfer("move")} aria-label="Move" title="Move to the other pane (F6)"><Icon name="move" /></button>
    </div>
    <div class="group" inert={frozen}>
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
    <div class="group" inert={frozen}>
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
    <div class="group live" role="group" aria-label="Auto-sync" data-testid="live-controls">
      <button aria-pressed={liveKind === "right"} onclick={() => toggleLive("right")} aria-label="Auto-sync left to right"
        title="Auto-sync → : copy changes from left to right"><Icon name="arrow-right" /></button
      >
      <button aria-pressed={liveKind === "left"} onclick={() => toggleLive("left")} aria-label="Auto-sync right to left"
        title="Auto-sync ← : copy changes from right to left"><Icon name="arrow-left" /></button
      >
      <button aria-pressed={liveKind === "both"} onclick={() => toggleLive("both")} aria-label="Auto-sync both ways"
        title="Auto-sync ⇄ : copy changes both ways"><Icon name="arrow-left-right" /></button
      >
    </div>
    <!-- Pushes the view and pane groups to the right; errors show in the gap. -->
    <div class="spacer">
      {#if notice}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <span class="notice" title={notice} onclick={() => (notice = null)}>{notice}</span>
      {/if}
    </div>
    <div class="group" role="group" aria-label="View" inert={frozen}>
      <button aria-pressed={active.view === "list"} onclick={() => (active.view = "list")} aria-label="List view" title="List view (Ctrl/⌘ 1)"
        ><Icon name="list" /></button
      >
      <button aria-pressed={active.view === "icons"} onclick={() => (active.view = "icons")} aria-label="Icon view" title="Icon view (Ctrl/⌘ 2)"
        ><Icon name="grid" /></button
      >
    </div>
    <div class="group" inert={frozen}>
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
    <div class="group" inert={frozen}>
      <button onclick={() => active.refresh()} aria-label="Refresh" title="Refresh (Ctrl/⌘ R)"><Icon name="refresh" /></button>
      <button onclick={chooseEditor} aria-label="Editor" title="Choose the editor used by Edit"><Icon name="file-code" /></button>
      <button onclick={() => (showPalette = true)} aria-label="Command Palette" title="Command Palette: find and run any command ({MOD}Shift P)"
        ><Icon name="command" /></button
      >
      <button onclick={openTerminal} aria-label="Open Terminal" title="Open a terminal in this folder ({MOD}Shift T)"><Icon name="terminal" /></button>
      <button
        aria-pressed={active.showHidden}
        onclick={() => (active.showHidden = !active.showHidden)}
        aria-label="Hidden files"
        title="Show hidden files (Ctrl/⌘ Shift .)"><Icon name="eye" /></button
      >
    </div>
    <div class="group">
      <button class="heart" onclick={() => (aboutPage = "donate")} aria-label="Donate" title="Support Dual Pal with a donation"
        ><Icon name="heart" /></button
      >
      <button onclick={() => (aboutPage = "about")} aria-label="About" title="About Dual Pal, help and support"><Icon name="help" /></button>
    </div>
  </div>

  <div class="live-bar" class:on={live} class:failed={live?.state === "failed"} role="status" data-testid="live-bar">
    {#if live && liveLeft && liveRight}
      <strong>Auto-sync</strong>
      <span class="live-places" title="{liveLeft.path} {liveArrow} {liveRight.path}"
        >{placeName(liveLeft)} {liveArrow} {placeName(liveRight)}</span
      >
      <span class="live-state">{liveText}</span>
      <span class="live-note">Panes are locked while this runs.</span>
      <button data-testid="live-stop" onclick={() => liveKind && toggleLive(liveKind)}>Stop</button>
    {/if}
  </div>

  <div class="middle" class:with-inspector={showInspector} class:frozen inert={frozen}>
    <Sidebar
      {volumes}
      {favorites}
      current={active.location}
      onopen={(loc) => active.open(loc)}
      onremovefavorite={async (loc) => (favorites = await api.removeFavorite(loc))}
      onreorder={async (from, to) => (favorites = await api.reorderFavorites(from, to))}
      {connections}
      {connecting}
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
    <main
      class="panes"
      class:resizing
      bind:this={panesEl}
      bind:clientWidth={panesWidth}
      style:grid-template-columns={`minmax(0, ${shownSplit}fr) 1px minmax(0, ${1 - shownSplit}fr)`}
    >
      {#each groups as group, i}
        {#if i === 1}
          <!-- A focusable separator is interactive under ARIA; the lint does not know that. -->
          <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
          <div
            class="splitter"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize panes"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(shownSplit * 100)}
            tabindex="0"
            title="Drag to resize the panes, double-click to make them equal"
            data-testid="pane-splitter"
            bind:this={splitterEl}
            onpointerdown={splitDown}
            onpointermove={splitMove}
            onpointerup={splitUp}
            onpointercancel={splitUp}
            ondblclick={splitReset}
            onkeydown={splitKey}
          ></div>
        {/if}
        <Pane
          {group}
          index={i}
          active={i === activeIndex}
          onfocus={() => (activeIndex = i)}
          oncontext={openMenu}
          ondrop={dropped}
        />
      {/each}
    </main>
    {#if showInspector}
      <Inspector
        entry={active.current ?? null}
        fs={active.location?.fs ?? "local"}
        folderSize={active.current?.kind === "dir" ? (active.sizes.get(active.current.path) ?? null) : null}
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

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />
{/if}

{#if drag.items.length}
  <div class="drag-ghost" style:left="{drag.x + 14}px" style:top="{drag.y + 10}px">
    {drag.items.length === 1 ? drag.items[0].name : `${drag.items.length} items`}
    {#if drag.target}
      <span class="drag-action">{drag.move ? "Move" : "Copy"} to “{drag.target.name}”</span>
    {/if}
  </div>
{/if}

{#if showPalette}
  <CommandPalette {commands} onclose={() => (showPalette = false)} />
{/if}
{#if showFind && active.location}
  <FindDialog
    location={active.location}
    onclose={() => (showFind = false)}
    onopen={async (hit) => {
      showFind = false;
      const parent = await api.parentLocation({ fs: active.location!.fs, path: hit.path });
      if (parent) await active.open(parent, { record: true, focusName: hit.name });
    }}
  />
{/if}

{#if aboutPage}
  <AboutDialog start={aboutPage} onclose={() => (aboutPage = null)} />
{/if}

<UpdateBar />


{#if showInsights}
  <Insights {nameOf} onclose={() => (showInsights = false)} />
{/if}

{#if editing !== undefined}
  <ConnectionDialog
    connection={editing}
    onsave={serverSaved}
    oncancel={() => (editing = undefined)}
    onimport={() => {
      editing = undefined;
      importing = true;
    }}
  />
{/if}

{#if importing}
  <ImportDialog onsaved={() => loadConnections()} onclose={() => (importing = false)} />
{/if}

<style>
  .drag-ghost {
    position: fixed;
    z-index: 30;
    pointer-events: none;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: 320px;
    padding: 5px 9px;
    border-radius: 7px;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.25);
    font-size: 12.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .drag-action {
    color: var(--accent);
    font-size: 11.5px;
  }
  .live-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 12px;
    font-size: 12.5px;
    background: color-mix(in srgb, var(--accent) 12%, var(--pane-bg));
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  /* Hidden but kept in the grid, so the rows below keep their places. */
  .live-bar:not(.on) {
    height: 0;
    overflow: hidden;
    border-bottom: 0;
  }
  .live-bar.on {
    height: 34px;
  }
  .live-bar.failed {
    background: color-mix(in srgb, var(--danger) 12%, var(--pane-bg));
  }
  .live-bar :global(.icon) {
    color: var(--accent);
  }
  .live-places {
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 40%;
  }
  .live-state {
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    min-width: 0;
  }
  .live-note {
    color: var(--muted);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .live-bar button {
    padding: 3px 12px;
    border-radius: 6px;
    background: var(--control);
  }
  .live-bar button:hover {
    background: var(--control-hover);
  }
  .middle.frozen {
    opacity: 0.7;
  }
  .toolbar .group[inert] {
    opacity: 0.4;
  }
  .app {
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto;
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
  .group button.heart {
    color: #e0245e;
  }
  .group button[aria-pressed="true"] {
    background: var(--accent);
    color: white;
  }
  .spacer {
    flex: 1;
    display: flex;
    min-width: 0;
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
    grid-template-columns: minmax(0, 1fr) 1px minmax(0, 1fr);
    min-height: 0;
  }
  .panes.resizing {
    cursor: col-resize;
    user-select: none;
  }
  .splitter {
    position: relative;
    background: var(--border);
    cursor: col-resize;
    touch-action: none;
    outline: none;
  }
  /* A wider grab area than the 1px line it draws. */
  .splitter::before {
    content: "";
    position: absolute;
    inset: 0 -4px;
    z-index: 2;
  }
  .splitter:hover,
  .splitter:focus-visible,
  .panes.resizing .splitter {
    background: var(--accent);
  }
</style>
