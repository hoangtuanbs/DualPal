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
  | { type: "createDir"; location: Location }
  | { type: "rename"; location: Location; newName: string };

export type OperationStatus = "queued" | "running" | "completed" | "failed" | "cancelled";

export interface ErrorInfo {
  kind: string;
  message: string;
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
  onOperationUpdated: (cb: (info: OperationInfo) => void): Promise<UnlistenFn> =>
    listen<OperationInfo>("operation-updated", (e) => cb(e.payload)),
};
