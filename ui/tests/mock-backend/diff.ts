// A small fake of the compare view's backend (crates/dualpal-core/src/diff.rs)
// for component tests: an LCS line diff, intra-line marks from the common
// start and end of a changed pair, copying changes and saving.

interface Line {
  text: string;
  eol: "none" | "lf" | "crLf";
}
interface Options {
  ignoreWhitespace: boolean;
  ignoreCase: boolean;
  ignoreLineEndings: boolean;
}
interface Hunk {
  leftStart: number;
  leftLen: number;
  rightStart: number;
  rightLen: number;
}
interface MockSession {
  paths: [{ fs: string; path: string }, { fs: string; path: string }];
  lines: [Line[], Line[]];
  dirty: [boolean, boolean];
  options: Options;
  revision: number;
  hunks: Hunk[];
}

export function splitLines(text: string): Line[] {
  const out: Line[] = [];
  const parts = text.split("\n");
  parts.forEach((p, i) => {
    if (i === parts.length - 1) {
      if (p) out.push({ text: p, eol: "none" });
    } else if (p.endsWith("\r")) out.push({ text: p.slice(0, -1), eol: "crLf" });
    else out.push({ text: p, eol: "lf" });
  });
  return out;
}

const joinLines = (lines: Line[]) => lines.map((l) => l.text + (l.eol === "lf" ? "\n" : l.eol === "crLf" ? "\r\n" : "")).join("");

function key(l: Line, o: Options): string {
  let s = o.ignoreWhitespace ? l.text.replace(/\s+/g, "") : l.text;
  if (o.ignoreCase) s = s.toLowerCase();
  return o.ignoreLineEndings ? s : `${s}\n${l.eol}`;
}

export function diffLines(a: Line[], b: Line[], o: Options): Hunk[] {
  const ka = a.map((l) => key(l, o));
  const kb = b.map((l) => key(l, o));
  const n = ka.length;
  const m = kb.length;
  const dp = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = ka[i] === kb[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const hunks: Hunk[] = [];
  let i = 0;
  let j = 0;
  let open: Hunk | null = null;
  const take = (dl: number, dr: number) => {
    if (!open) open = { leftStart: i, leftLen: 0, rightStart: j, rightLen: 0 };
    open.leftLen += dl;
    open.rightLen += dr;
  };
  while (i < n || j < m) {
    if (i < n && j < m && ka[i] === kb[j]) {
      if (open) hunks.push(open);
      open = null;
      i++;
      j++;
    } else if (j < m && (i >= n || dp[i][j + 1] >= dp[i + 1][j])) {
      take(0, 1);
      j++;
    } else {
      take(1, 0);
      i++;
    }
  }
  if (open) hunks.push(open);
  return hunks;
}

function marks(a: string, b: string): [[number, number][], [number, number][]] {
  if (a === b) return [[], []];
  let p = 0;
  while (p < a.length && p < b.length && a[p] === b[p]) p++;
  let s = 0;
  while (s < a.length - p && s < b.length - p && a[a.length - 1 - s] === b[b.length - 1 - s]) s++;
  const m = (t: string): [number, number][] => (t.length - s > p ? [[p, t.length - s]] : []);
  return [m(a), m(b)];
}

type Tree = Map<string, { kind: string; content?: string; bytes?: Uint8Array; size: number }>;

/** Handlers for the `diff_*` commands over `treeOf(fs)`. */
export function diffHandlers(treeOf: (fs: string) => Tree) {
  const sessions: MockSession[] = [];
  const get = (session: number) => {
    const s = sessions[session - 1];
    if (!s) throw { kind: "invalid", message: `the compare ${session} is not open` };
    return s;
  };
  const sideIndex = (side: string) => (side === "left" ? 0 : 1);
  const state = (s: MockSession, session: number) => ({
    session,
    revision: s.revision,
    options: s.options,
    hunks: s.hunks,
    identical: joinLines(s.lines[0]) === joinLines(s.lines[1]),
    sides: s.lines.map((lines, i) => ({
      encoding: "utf-8",
      detected: "utf-8",
      bom: false,
      lines: lines.length,
      size: joinLines(lines).length,
      endings: lines.some((l) => l.eol === "crLf") ? (lines.some((l) => l.eol === "lf") ? "mixed" : "crLf") : lines.some((l) => l.eol === "lf") ? "lf" : "none",
      dirty: s.dirty[i],
      saveable: true,
    })),
  });
  const recompute = (s: MockSession) => {
    s.hunks = diffLines(s.lines[0], s.lines[1], s.options);
    s.revision++;
  };
  const read = (loc: { fs: string; path: string }) => {
    const e = treeOf(loc.fs).get(loc.path);
    if (!e || e.kind !== "file") throw { kind: "notFound", message: `not found: ${loc.path}` };
    return e;
  };
  return {
    diff_open: (a: any) => {
      const l = read(a.left);
      const r = read(a.right);
      if (l.bytes || r.bytes) {
        const bl = l.bytes ?? new TextEncoder().encode(l.content ?? "");
        const br = r.bytes ?? new TextEncoder().encode(r.content ?? "");
        return { kind: "binary", identical: bl.length === br.length && bl.every((x, i) => x === br[i]), sizes: [bl.length, br.length] };
      }
      const options = { ignoreWhitespace: false, ignoreCase: false, ignoreLineEndings: false, ...(a.options ?? {}) };
      const s: MockSession = {
        paths: [a.left, a.right],
        lines: [splitLines(l.content ?? ""), splitLines(r.content ?? "")],
        dirty: [false, false],
        options,
        revision: 0,
        hunks: [],
      };
      recompute(s);
      sessions.push(s);
      return { kind: "text", state: state(s, sessions.length) };
    },
    diff_lines: (a: any) => {
      const s = get(a.session);
      const i = sideIndex(a.side);
      const lines = s.lines[i];
      return lines.slice(a.from, a.from + a.count).map((l, k) => {
        const n = a.from + k;
        const h = s.hunks.find((h) => (i ? h.rightStart : h.leftStart) <= n && n < (i ? h.rightStart + h.rightLen : h.leftStart + h.leftLen));
        let m: [number, number][] = [];
        if (h) {
          const pos = n - (i ? h.rightStart : h.leftStart);
          if (pos < (i ? h.leftLen : h.rightLen)) {
            const other = s.lines[1 - i][(i ? h.leftStart : h.rightStart) + pos];
            const [ml, mr] = i ? marks(other.text, l.text) : marks(l.text, other.text);
            m = i ? mr : ml;
          }
        }
        return { text: l.text, eol: l.eol, marks: m };
      });
    },
    diff_set_options: (a: any) => {
      const s = get(a.session);
      s.options = a.options;
      recompute(s);
      return state(s, a.session);
    },
    diff_set_encoding: (a: any) => state(get(a.session), a.session),
    diff_apply: (a: any) => {
      const s = get(a.session);
      if (a.revision !== s.revision) throw { kind: "invalid", message: "the compare view is out of date; try again" };
      const h = s.hunks[a.hunk];
      if (!h) throw { kind: "invalid", message: `there is no change ${a.hunk}` };
      const to = sideIndex(a.to);
      const src = to ? s.lines[0].slice(h.leftStart, h.leftStart + h.leftLen) : s.lines[1].slice(h.rightStart, h.rightStart + h.rightLen);
      s.lines[to].splice(to ? h.rightStart : h.leftStart, to ? h.rightLen : h.leftLen, ...src);
      s.dirty[to] = true;
      recompute(s);
      return state(s, a.session);
    },
    diff_save: (a: any) => {
      const s = get(a.session);
      const i = sideIndex(a.side);
      const e = read(s.paths[i]);
      e.content = joinLines(s.lines[i]);
      e.size = e.content.length;
      s.dirty[i] = false;
      return state(s, a.session);
    },
    diff_cancel: () => null,
    diff_close: () => null,
  };
}
