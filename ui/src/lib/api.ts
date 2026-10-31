// Typed wrappers around the Tauri commands in src-tauri/src/commands.rs.
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
  | { type: "rename"; location: Location; newName: string };

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
  previewPath: (location: Location) => invoke<string>("preview_path", { location }),
  settings: () => invoke<Settings>("settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  onEditSessionUpdated: (cb: (s: EditSession) => void): Promise<UnlistenFn> =>
    listen<EditSession>("edit-session-updated", (e) => cb(e.payload)),
  onOperationUpdated: (cb: (info: OperationInfo) => void): Promise<UnlistenFn> =>
    listen<OperationInfo>("operation-updated", (e) => cb(e.payload)),
};
