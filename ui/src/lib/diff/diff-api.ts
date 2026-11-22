// Typed wrappers around the compare view commands in src-tauri/src/diff.rs.
// Types mirror crates/dualpal-core/src/diff.rs.
import { invoke } from "@tauri-apps/api/core";
import type { Location } from "../api";
import type { TextEncoding } from "../viewer/viewer-api";

export interface DiffOptions {
  ignoreWhitespace: boolean;
  ignoreCase: boolean;
  ignoreLineEndings: boolean;
}

export type Side = "left" | "right";
export type Eol = "none" | "lf" | "crLf";
export type Endings = "none" | "lf" | "crLf" | "mixed";

/** Changed lines: `leftLen` lines at `leftStart` stand where the right has `rightLen` at `rightStart` (from 0). */
export interface Hunk {
  leftStart: number;
  leftLen: number;
  rightStart: number;
  rightLen: number;
}

export interface SideInfo {
  encoding: TextEncoding;
  detected: TextEncoding;
  bom: boolean;
  lines: number;
  size: number;
  endings: Endings;
  dirty: boolean;
  /** Saving keeps every byte that was not changed. */
  saveable: boolean;
}

export interface DiffState {
  session: number;
  revision: number;
  options: DiffOptions;
  hunks: Hunk[];
  /** Same lines and line endings, whatever the options. */
  identical: boolean;
  sides: [SideInfo, SideInfo];
}

export interface DiffLine {
  text: string;
  eol: Eol;
  /** Changed parts as [start, end) string indexes. */
  marks: [number, number][];
}

export type DiffOutcome =
  | { kind: "text"; state: DiffState }
  | { kind: "binary"; identical: boolean; sizes: [number, number] }
  | { kind: "tooLarge"; sizes: [number, number]; maxBytes: number; maxLines: number };

export const NO_OPTIONS: DiffOptions = { ignoreWhitespace: false, ignoreCase: false, ignoreLineEndings: false };

export const diffApi = {
  open: (request: number, left: Location, right: Location, options: DiffOptions, encodings: [TextEncoding | null, TextEncoding | null] | null = null) =>
    invoke<DiffOutcome>("diff_open", { request, left, right, options, encodings }),
  lines: (session: number, side: Side, from: number, count: number) => invoke<DiffLine[]>("diff_lines", { session, side, from, count }),
  setOptions: (session: number, options: DiffOptions) => invoke<DiffState>("diff_set_options", { session, options }),
  setEncoding: (session: number, side: Side, encoding: TextEncoding) => invoke<DiffState>("diff_set_encoding", { session, side, encoding }),
  /** Copy change `hunk` to side `to`. */
  apply: (session: number, revision: number, hunk: number, to: Side) => invoke<DiffState>("diff_apply", { session, revision, hunk, to }),
  save: (session: number, side: Side) => invoke<DiffState>("diff_save", { session, side }),
  cancel: (request: number) => invoke<void>("diff_cancel", { request }),
  close: (session: number) => invoke<void>("diff_close", { session }),
};
