// Typed wrappers around the Tauri commands in src-tauri/src (commands.rs, remote.rs, tools.rs).
// Types mirror the serde shapes in crates/dualpal-core.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface Location {
  fs: string;
  path: string;
}

export type EntryKind = "file" | "dir" | "other";

export interface Entry {
  name: string;
  path: string;
  kind: EntryKind;
  symlink: boolean;
  size: number;
  modifiedMs: number | null;
  createdMs: number | null;
  hidden: boolean;
  readonly: boolean;
  mode: number | null;
}

export type ConflictPolicy = "fail" | "skip" | "overwrite";

export type Operation =
  | { type: "copy"; sources: Location[]; destination: Location; conflict?: ConflictPolicy }
  | { type: "move"; sources: Location[]; destination: Location; conflict?: ConflictPolicy }
  | { type: "trash"; targets: Location[] }
  | { type: "delete"; targets: Location[] }
  | { type: "compress"; sources: Location[]; destination: Location }
  | { type: "setMode"; location: Location; mode: number }
  | { type: "createDir"; location: Location }
  | { type: "rename"; location: Location; newName: string }
  | { type: "batchRename"; items: { location: Location; newName: string }[] };

export type OperationStatus = "queued" | "running" | "completed" | "failed" | "cancelled";

export interface ErrorInfo {
  kind: string;
  message: string;
  /** SSH host key fingerprint, for `unknownHost` and `hostKeyChanged`. */
  fingerprint?: string;
}

export interface Progress {
  filesTotal: number;
  filesDone: number;
  bytesTotal: number;
  bytesDone: number;
  skipped: number;
  current: string | null;
}

export interface OperationInfo {
  id: string;
  kind: string;
  summary: string;
  status: OperationStatus;
  progress: Progress;
  error: ErrorInfo | null;
  queuedMs: number;
  startedMs: number | null;
  finishedMs: number | null;
  places: string[];
}

export interface ActivityRecord {
  id: string;
  kind: string;
  summary: string;
  status: OperationStatus;
  startedMs: number;
  finishedMs: number;
  filesDone: number;
  bytesDone: number;
  skipped: number;
  error: string | null;
  places: string[];
}

export interface TextFile {
  text: string;
  truncated: boolean;
  binary: boolean;
}

export interface FileSystemInfo {
  id: string;
  name: string;
}

export interface Volume {
  name: string;
  location: Location;
  totalBytes: number;
  availableBytes: number;
  removable: boolean;
}

export interface Favorite {
  name: string;
  location: Location;
  icon: string;
}

export type SshAuth = { method: "password" } | { method: "keyFile"; path: string } | { method: "agent" };

export type Endpoint =
  | { type: "sftp"; host: string; port: number; user: string; auth: SshAuth }
  | {
      type: "s3";
      bucket: string;
      region: string;
      endpoint: string | null;
      accessKeyId: string;
      pathStyle: boolean;
    };

export interface RemoteFavorite {
  name: string;
  path: string;
}

export interface Connection {
  id: string;
  name: string;
  endpoint: Endpoint;
  initialPath: string | null;
  favorites: RemoteFavorite[];
}

export interface ConnectionView extends Connection {
  connected: boolean;
  /** File system id once connected, e.g. "conn:…". */
  fsId: string;
}

export interface SshConfigHost {
  alias: string;
  hostName: string;
  user: string | null;
  port: number | null;
  identityFile: string | null;
}

export type EditStatus = "watching" | "uploading" | "uploaded" | "failed" | "stopped";

export interface EditSession {
  id: string;
  remote: Location;
  localPath: string;
  status: EditStatus;
  uploads: number;
  lastUploadMs: number | null;
  error: string | null;
}

export interface Settings {
  editor: string | null;
  diffTool: string | null;
}

// ---- compare and sync ----

export type CompareBy = "sizeAndDate" | "size" | "content";

export interface CompareOptions {
  by: CompareBy;
  toleranceMs: number;
  exclude: string[];
  includeHidden: boolean;
}

export const defaultCompareOptions = (): CompareOptions => ({
  by: "sizeAndDate",
  toleranceMs: 2000,
  exclude: [".DS_Store", "Thumbs.db", "desktop.ini", "._*"],
  includeHidden: true,
});

export type DiffState = "same" | "leftOnly" | "rightOnly" | "leftNewer" | "rightNewer" | "different" | "kindMismatch";

export interface Side {
  dir: boolean;
  size: number;
  modifiedMs: number | null;
}

export interface DiffItem {
  rel: string;
  name: string;
  depth: number;
  dir: boolean;
  left: Side | null;
  right: Side | null;
  state: DiffState;
}

export interface Comparison {
  left: Location;
  right: Location;
  options: CompareOptions;
  items: DiffItem[];
  summary: Record<"same" | "leftOnly" | "rightOnly" | "leftNewer" | "rightNewer" | "different" | "kindMismatch", number>;
}

export type SyncMode = "update" | "mirror" | "twoWay";
export type SyncAction = "copyToRight" | "copyToLeft" | "deleteLeft" | "deleteRight" | "skip";
export type SyncReason =
  | "new"
  | "changed"
  | "deleted"
  | "extra"
  | "onlyOnTarget"
  | "targetChanged"
  | "conflict"
  | "changedAndDeleted"
  | "kindMismatch";

export interface Totals {
  files: number;
  bytes: number;
}

export interface SyncStep {
  rel: string;
  dir: boolean;
  action: SyncAction;
  reason: SyncReason;
  conflict: boolean;
  left: Totals | null;
  right: Totals | null;
}

export interface SyncPreview {
  id: string;
  mode: SyncMode;
  comparison: Comparison;
  steps: SyncStep[];
  hasHistory: boolean;
}

export interface SyncFavorite {
  id: string;
  name: string;
  left: Location;
  right: Location;
  mode: SyncMode;
  options: CompareOptions;
}

// ---- Multi Rename ----

export type Scope = "name" | "extension" | "whole";
export type Position = "start" | "end";
export type CaseKind = "lower" | "upper" | "title" | "sentence";

export type RenameRule =
  | { type: "replace"; find: string; replace: string; regex: boolean; caseSensitive: boolean; scope: Scope }
  | { type: "case"; case: CaseKind; scope: Scope }
  | { type: "insert"; text: string; at: Position; scope: Scope }
  | { type: "number"; start: number; step: number; pad: number; at: Position; separator: string }
  | { type: "remove"; count: number; from: Position; offset: number; scope: Scope }
  | { type: "extension"; ext: string };

export interface RenameInput {
  name: string;
  dir: boolean;
  modifiedMs: number | null;
  parent: string;
}

export interface RenamePreview {
  name: string;
  newName: string;
  issue: string | null;
}

export interface RenamePreset {
  name: string;
  rules: RenameRule[];
  builtin: boolean;
}

// ---- tools and insights ----

export interface DiffTool {
  name: string;
  command: string;
  folders: boolean;
}

export interface DayStat {
  startMs: number;
  operations: number;
  failed: number;
  files: number;
  bytes: number;
}

export interface GroupStat {
  key: string;
  operations: number;
  failed: number;
  files: number;
  bytes: number;
}

export interface Insights {
  days: DayStat[];
  operations: number;
  completed: number;
  failed: number;
  cancelled: number;
  files: number;
  bytes: number;
  busyMs: number;
  throughput: number | null;
  byKind: GroupStat[];
  byPlace: GroupStat[];
  largest: ActivityRecord[];
  failures: ActivityRecord[];
}

export function errorKind(e: unknown): string | null {
  return e && typeof e === "object" && "kind" in e ? String((e as ErrorInfo).kind) : null;
}

/** Errors from commands arrive as `{ kind, message }`. */
export function errorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as ErrorInfo).message);
  return String(e);
}

export const api = {
  fileSystems: () => invoke<FileSystemInfo[]>("file_systems"),
  homeLocation: () => invoke<Location>("home_location"),
  listDir: (location: Location) => invoke<Entry[]>("list_dir", { location }),
  parentLocation: (location: Location) => invoke<Location | null>("parent_location", { location }),
  enqueue: (operation: Operation) => invoke<string>("enqueue_operation", { operation }),
  cancel: (id: string) => invoke<boolean>("cancel_operation", { id }),
  operations: () => invoke<OperationInfo[]>("operations"),
  clearFinished: () => invoke<void>("clear_finished_operations"),
  activity: () => invoke<ActivityRecord[]>("activity"),
  clearActivity: () => invoke<void>("clear_activity"),
  volumes: () => invoke<Volume[]>("volumes"),
  availableSpace: (location: Location) => invoke<number | null>("available_space", { location }),
  favorites: () => invoke<Favorite[]>("favorites"),
  addFavorite: (favorite: Favorite) => invoke<Favorite[]>("add_favorite", { favorite }),
  removeFavorite: (location: Location) => invoke<Favorite[]>("remove_favorite", { location }),
  reorderFavorites: (from: number, to: number) => invoke<Favorite[]>("reorder_favorites", { from, to }),
  openPath: (location: Location) => invoke<void>("open_path", { location }),
  stat: (location: Location) => invoke<Entry>("stat", { location }),
  readText: (location: Location, maxBytes: number) => invoke<TextFile>("read_text", { location, maxBytes }),
  writeText: (location: Location, text: string) => invoke<void>("write_text", { location, text }),
  connections: () => invoke<ConnectionView[]>("connections"),
  saveConnection: (connection: Connection, secret: string | null) =>
    invoke<ConnectionView>("save_connection", { connection, secret }),
  removeConnection: (id: string) => invoke<void>("remove_connection", { id }),
  reorderConnections: (ids: string[]) => invoke<void>("reorder_connections", { ids }),
  connect: (id: string, opts: { secret?: string | null; remember?: boolean; trust?: string | null } = {}) =>
    invoke<{ location: Location; name: string }>("connect", {
      id,
      secret: opts.secret ?? null,
      remember: opts.remember ?? false,
      trust: opts.trust ?? null,
    }),
  disconnect: (id: string) => invoke<void>("disconnect", { id }),
  addConnectionFavorite: (id: string, favorite: RemoteFavorite) =>
    invoke<ConnectionView>("add_connection_favorite", { id, favorite }),
  removeConnectionFavorite: (id: string, path: string) =>
    invoke<ConnectionView>("remove_connection_favorite", { id, path }),
  sshConfigHosts: () => invoke<SshConfigHost[]>("ssh_config_hosts"),
  /** Open a file in its default app, or the preferred editor. Remote files upload on save. */
  openFile: (location: Location, editor = false) => invoke<EditSession | null>("open_file", { location, editor }),
  editSessions: () => invoke<EditSession[]>("edit_sessions"),
  stopEditSession: (id: string) => invoke<boolean>("stop_edit_session", { id }),
  uploadEditSession: (id: string) => invoke<EditSession>("upload_edit_session", { id }),
  previewPath: (location: Location) => invoke<string>("preview_path", { location }),
  settings: () => invoke<Settings>("settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  syncPlan: (left: Location, right: Location, mode: SyncMode, options: CompareOptions) =>
    invoke<SyncPreview>("sync_plan", { left, right, mode, options }),
  cancelCompare: () => invoke<void>("cancel_compare"),
  runSync: (plan: string, overrides: Record<string, SyncAction>) => invoke<string>("run_sync", { plan, overrides }),
  forgetSyncHistory: (left: Location, right: Location) => invoke<void>("forget_sync_history", { left, right }),
  syncFavorites: () => invoke<SyncFavorite[]>("sync_favorites"),
  saveSyncFavorite: (favorite: SyncFavorite) => invoke<SyncFavorite>("save_sync_favorite", { favorite }),
  removeSyncFavorite: (id: string) => invoke<void>("remove_sync_favorite", { id }),
  onCompareProgress: (cb: (p: { folders: number; current: string }) => void): Promise<UnlistenFn> =>
    listen<{ folders: number; current: string }>("compare-progress", (e) => cb(e.payload)),
  renamePreview: (rules: RenameRule[], inputs: RenameInput[], others: string[]) =>
    invoke<RenamePreview[]>("rename_preview", { rules, inputs, others }),
  renamePresets: () => invoke<RenamePreset[]>("rename_presets"),
  saveRenamePreset: (preset: RenamePreset) => invoke<RenamePreset[]>("save_rename_preset", { preset }),
  removeRenamePreset: (name: string) => invoke<RenamePreset[]>("remove_rename_preset", { name }),
  diffTools: () => invoke<DiffTool[]>("diff_tools"),
  compareFiles: (left: Location, right: Location) => invoke<void>("compare_files", { left, right }),
  activityInsights: (days: number) =>
    invoke<Insights>("activity_insights", { days, utcOffsetMinutes: -new Date().getTimezoneOffset() }),
  onEditSessionUpdated: (cb: (s: EditSession) => void): Promise<UnlistenFn> =>
    listen<EditSession>("edit-session-updated", (e) => cb(e.payload)),
  onOperationUpdated: (cb: (info: OperationInfo) => void): Promise<UnlistenFn> =>
    listen<OperationInfo>("operation-updated", (e) => cb(e.payload)),
};
