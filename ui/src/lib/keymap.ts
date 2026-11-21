// Keyboard shortcuts: the default keys of every command, the person's own
// bindings on top of them, and matching key presses to commands.
//
// A key combination is written as modifiers then the key, joined by "+":
// "Mod+Shift+P", "F5", "Shift+Delete", "Alt+Shift+Enter", "Ctrl+Tab".
// "Mod" is ⌘ on macOS and Ctrl elsewhere. "Ctrl" is the Control key itself,
// which only differs from "Mod" on macOS. Keys are KeyboardEvent.key names,
// letters in upper case, " " written as "Space".

/** Default keys of each command id; equal to the shortcuts in the README. */
export const DEFAULT_KEYS: Readonly<Record<string, readonly string[]>> = {
  palette: ["Mod+Shift+P"],
  settings: ["Mod+,"],
  find: ["Mod+Shift+F"],
  filter: ["Mod+F"],
  copy: ["F5"],
  move: ["F6"],
  newFolder: ["F7"],
  rename: ["F2"],
  multiRename: ["Shift+F2"],
  edit: ["F4"],
  terminal: ["Mod+Shift+T"],
  trash: ["F8", "Delete", "Mod+Backspace"],
  delete: ["Shift+F8", "Shift+Delete", "Mod+Alt+Backspace"],
  compareFolders: ["Mod+Shift+Y"],
  compareFiles: ["Mod+Shift+D"],
  linked: ["Mod+Shift+L"],
  selectAll: ["Mod+A"],
  selectPattern: ["+"],
  deselectPattern: ["-"],
  list: ["Mod+1"],
  icons: ["Mod+2"],
  hidden: ["Mod+Shift+."],
  info: ["Mod+I"],
  refresh: ["Mod+R"],
  folderSizes: ["Alt+Shift+Enter"],
  newTab: ["Mod+T"],
  closeTab: ["Mod+W"],
  nextTab: ["Ctrl+Tab"],
  prevTab: ["Ctrl+Shift+Tab"],
  switch: ["Tab", "Shift+Tab"],
  up: ["Mod+ArrowUp", "Backspace"],
};

/** Commands that also work while typing in a text field, when their key
 *  has Ctrl/⌘ or is a function key (so typing a letter never runs them).
 *  "noTextarea": not in the quick editor, where the key edits the text. */
export const FIELD_COMMANDS: Readonly<Record<string, "any" | "noTextarea">> = {
  palette: "any",
  settings: "any",
  find: "any",
  filter: "noTextarea",
};

/** Keys that move the cursor and mark items; they cannot be bound. */
const RESERVED = new Set(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown", "Home", "End", "Enter", "Space", "Insert", "Escape", "ContextMenu"]);

/** Keys that are only modifiers, or that the system uses while composing. */
const NOT_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "OS", "AltGraph", "CapsLock", "Fn", "FnLock", "Hyper", "Super", "NumLock", "ScrollLock", "Dead", "Unidentified", "Process"]);

/** Punctuation by key position, for presses with Ctrl/⌘ or Alt, where
 *  `key` may be shifted (">" for Shift+.) or a special character (Alt on a
 *  Mac). Letters and digits come from `key`, so they follow the layout
 *  (Ctrl+A on AZERTY), and from the key position only when the modifiers
 *  turned them into a symbol ("®" for ⌥R on a Mac). */
const PUNCTUATION: Record<string, string> = {
  Period: ".",
  Comma: ",",
  Slash: "/",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
};

export interface Combo {
  ctrl: boolean;
  mod: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

export type KeyEventLike = Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">;

export const isMacPlatform = () => typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

export function parseCombo(text: string): Combo {
  let key: string;
  let mods: string[];
  if (text === "+") {
    key = "+";
    mods = [];
  } else if (text.endsWith("++")) {
    key = "+";
    mods = text.slice(0, -2).split("+");
  } else {
    const parts = text.split("+");
    key = parts.pop() ?? "";
    mods = parts;
  }
  const has = (m: string) => mods.some((x) => x.toLowerCase() === m);
  return { ctrl: has("ctrl"), mod: has("mod") || has("cmd") || has("meta"), alt: has("alt"), shift: has("shift"), key: normalKey(key) };
}

function normalKey(key: string): string {
  if (key === " ") return "Space";
  if (key.length === 1) return key.toUpperCase();
  if (key === "Esc") return "Escape";
  if (key === "Del") return "Delete";
  return key;
}

export function comboText(c: Combo): string {
  return [c.ctrl && "Ctrl", c.mod && "Mod", c.alt && "Alt", c.shift && "Shift", c.key].filter(Boolean).join("+");
}

/** The combination in its canonical spelling for this platform: off macOS
 *  "Ctrl" and "Mod" are the same key. */
export function normalizeCombo(text: string, mac: boolean): string {
  const c = parseCombo(text);
  if (!mac && c.ctrl) {
    c.ctrl = false;
    c.mod = true;
  }
  return comboText(c);
}

/** The key name of a press, or null for a modifier on its own. */
function eventKey(e: KeyEventLike): string | null {
  if (NOT_KEYS.has(e.key)) return null;
  const withMods = e.ctrlKey || e.metaKey || e.altKey;
  if (withMods) {
    if (/^[a-z0-9]$/i.test(e.key)) return e.key.toUpperCase();
    if (/^Key[A-Z]$/.test(e.code)) return e.code.slice(3);
    if (/^Digit[0-9]$/.test(e.code)) return e.code.slice(5);
    if (e.code in PUNCTUATION) return PUNCTUATION[e.code];
  }
  return normalKey(e.key);
}

/** The combination a key press makes, as recorded for a new shortcut; null
 *  for a modifier pressed on its own. */
export function comboFromEvent(e: KeyEventLike, mac = isMacPlatform()): string | null {
  const key = eventKey(e);
  if (!key) return null;
  const c: Combo = mac
    ? { ctrl: e.ctrlKey, mod: e.metaKey, alt: e.altKey, shift: e.shiftKey, key }
    : { ctrl: false, mod: e.ctrlKey || e.metaKey, alt: e.altKey, shift: e.shiftKey, key };
  return comboText(c);
}

/** The spellings a press may be bound under, best first: the exact one;
 *  on a Mac, Ctrl in place of ⌘ (as the app has always allowed); and for
 *  punctuation such as "+", the key without Shift (it often needs Shift). */
export function eventCombos(e: KeyEventLike, mac = isMacPlatform()): string[] {
  const exact = comboFromEvent(e, mac);
  if (!exact) return [];
  const out = [exact];
  const c = parseCombo(exact);
  if (mac && c.ctrl && !c.mod) out.push(comboText({ ...c, ctrl: false, mod: true }));
  if (c.shift && c.key.length === 1 && !/[A-Z0-9]/.test(c.key)) {
    for (const s of [...out]) out.push(comboText({ ...parseCombo(s), shift: false }));
  }
  return [...new Set(out)];
}

/** The person's bindings over the defaults: command id → key combinations. */
export function effectiveBindings(overrides: Record<string, string[]> | null | undefined): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const [id, keys] of Object.entries(DEFAULT_KEYS)) out[id] = [...keys];
  for (const [id, keys] of Object.entries(overrides ?? {})) out[id] = [...keys];
  return out;
}

const sameKeys = (a: readonly string[], b: readonly string[], mac: boolean) =>
  a.length === b.length && a.every((k, i) => normalizeCombo(k, mac) === normalizeCombo(b[i], mac));

/** Overrides after giving `id` the keys `keys`; a command back on its
 *  defaults is left out, so later default changes reach it. */
export function withBinding(overrides: Record<string, string[]> | null | undefined, id: string, keys: string[], mac = isMacPlatform()): Record<string, string[]> {
  const out = { ...(overrides ?? {}) };
  const unique = keys.filter((k, i) => keys.findIndex((x) => normalizeCombo(x, mac) === normalizeCombo(k, mac)) === i);
  if (sameKeys(unique, DEFAULT_KEYS[id] ?? [], mac)) delete out[id];
  else out[id] = unique;
  return out;
}

/** Overrides with `id` back on its default keys. */
export function resetBinding(overrides: Record<string, string[]> | null | undefined, id: string): Record<string, string[]> {
  const out = { ...(overrides ?? {}) };
  delete out[id];
  return out;
}

/** The commands other than `except` that `combo` already runs. */
export function conflicts(bindings: Record<string, string[]>, combo: string, except: string | null, mac = isMacPlatform()): string[] {
  const want = normalizeCombo(combo, mac);
  return Object.entries(bindings)
    .filter(([id, keys]) => id !== except && keys.some((k) => normalizeCombo(k, mac) === want))
    .map(([id]) => id);
}

/** Overrides after binding `combo` to `id` (in place of `replacing`, or
 *  added), taking it away from any command that had it. */
export function assignCombo(
  overrides: Record<string, string[]> | null | undefined,
  id: string,
  combo: string,
  replacing: string | null = null,
  mac = isMacPlatform(),
): Record<string, string[]> {
  let out = { ...(overrides ?? {}) };
  const bindings = effectiveBindings(out);
  const want = normalizeCombo(combo, mac);
  for (const other of conflicts(bindings, combo, id, mac)) {
    out = withBinding(out, other, bindings[other].filter((k) => normalizeCombo(k, mac) !== want), mac);
  }
  const mine = bindings[id] ?? [];
  const at = replacing === null ? -1 : mine.findIndex((k) => normalizeCombo(k, mac) === normalizeCombo(replacing, mac));
  const next = at >= 0 ? mine.map((k, i) => (i === at ? combo : k)) : [...mine, combo];
  return withBinding(out, id, next, mac);
}

/** Why `combo` cannot be a shortcut, or null when it can. */
export function unbindable(combo: string): string | null {
  const c = parseCombo(combo);
  if (!c.ctrl && !c.mod && !c.alt && RESERVED.has(c.key)) return `${formatCombo(combo)} moves the cursor or marks items, so it cannot be changed.`;
  if (!c.ctrl && !c.mod && !c.alt && c.shift && c.key === "F10") return `${formatCombo(combo)} opens the item menu, so it cannot be changed.`;
  return null;
}

/** Whether the combination may run a command while typing in a text field. */
export function worksInFields(combo: string): boolean {
  const c = parseCombo(combo);
  return c.ctrl || c.mod || /^F\d+$/.test(c.key);
}

/** Match a press against the bindings: the command id and the combination
 *  that matched, or null. */
export function lookup(bindings: Record<string, string[]>, e: KeyEventLike, mac = isMacPlatform()): { id: string; combo: string } | null {
  const index = new Map<string, { id: string; combo: string }>();
  for (const [id, keys] of Object.entries(bindings)) {
    for (const k of keys) {
      const n = normalizeCombo(k, mac);
      if (!index.has(n)) index.set(n, { id, combo: k });
    }
  }
  for (const c of eventCombos(e, mac)) {
    const hit = index.get(c);
    if (hit) return hit;
  }
  return null;
}

const KEY_NAMES: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  "-": "−",
  Escape: "Esc",
  PageUp: "Page Up",
  PageDown: "Page Down",
};

/** A combination as shown to people: "Ctrl+Shift+P", or "⇧⌘P" on a Mac. */
export function formatCombo(text: string, mac = isMacPlatform()): string {
  const c = parseCombo(text);
  const key = KEY_NAMES[c.key] ?? c.key;
  if (mac) return `${c.ctrl ? "⌃" : ""}${c.alt ? "⌥" : ""}${c.shift ? "⇧" : ""}${c.mod ? "⌘" : ""}${key}`;
  const mods = [(c.ctrl || c.mod) && "Ctrl", c.alt && "Alt", c.shift && "Shift"].filter(Boolean);
  return [...mods, key].join("+");
}

/** All keys of a command as shown to people, e.g. "F8, Delete". */
export function formatKeys(keys: readonly string[] | undefined, mac = isMacPlatform()): string | undefined {
  return keys?.length ? keys.map((k) => formatCombo(k, mac)).join(", ") : undefined;
}
